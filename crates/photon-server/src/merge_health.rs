//! Merge visibility: the shared state behind [`photon_api::CompactionStatus`].
//!
//! # Why this exists
//!
//! `merge_once` consolidates small Parquet files into bigger ones. It is the only thing standing
//! between a node and an unbounded file count, and it used to fail **silently**: the compaction
//! loops logged `merge_once failed: {e}` to stderr and moved on, so a merge that started failing
//! kept failing on every pass forever with nothing to see anywhere in the product.
//!
//! That is not hypothetical. A production node stopped merging on 2026-08-21 at 10:26 and nobody
//! noticed for 12 days: the last merged file across all three signals carries that timestamp, and
//! from there the hot tier grew by exactly 1,440 unmerged files per day per signal (one per closed
//! WAL segment, never consolidated). By the time it was found there were 48,979 files where a
//! healthy tier would have had a few hundred, and a 7-day RED query took 14.9 s instead of 1.8 s.
//! The stderr line was there the whole time. Nobody reads stderr on a healthy-looking node.
//!
//! Two things fix that, and this module carries both:
//!
//! 1. **The number is in the product.** `GET /api/data/storage` reports, per signal, when merging
//!    last succeeded and how many passes have failed in a row. A stalled merge is now a fact the
//!    `/data` page can show, not something you have to go read a log to learn.
//! 2. **The log escalates instead of repeating.** A merge always re-selects the OLDEST small files,
//!    so one bad input fails identically every 10 s until someone intervenes. Printing all of those
//!    is the same as printing none — the line stops being read. So the first three failures print,
//!    and after that only powers of two (4th, 8th, 16th, …), each carrying how long merging has
//!    been down. The stall is loud when it starts, still audible after three hours, and never spam.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

use photon_api::{CompactionHealth, CompactionStatus};

/// Per-signal merge state. All three signals merge independently, so one stalling says nothing
/// about the other two — a bad span file must not be reported as "compaction is down".
#[derive(Debug)]
struct SignalHealth {
    signal: &'static str,
    /// Epoch-ms of the last successful pass; `-1` = none since start-up.
    last_ok_ms: AtomicI64,
    consecutive_failures: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl SignalHealth {
    fn new(signal: &'static str) -> Self {
        Self {
            signal,
            last_ok_ms: AtomicI64::new(-1),
            consecutive_failures: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    fn snapshot(&self) -> CompactionHealth {
        let last_ok = self.last_ok_ms.load(Ordering::Relaxed);
        CompactionHealth {
            signal: self.signal,
            last_ok_ms: (last_ok >= 0).then_some(last_ok),
            consecutive_failures: self.consecutive_failures.load(Ordering::Relaxed),
            last_error: self.last_error.lock().unwrap().clone(),
        }
    }
}

/// The three compaction loops' merge state, shared with the API.
#[derive(Debug)]
pub struct MergeHealth {
    logs: SignalHealth,
    traces: SignalHealth,
    metrics: SignalHealth,
}

impl Default for MergeHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl MergeHealth {
    pub fn new() -> Self {
        Self {
            logs: SignalHealth::new("logs"),
            traces: SignalHealth::new("traces"),
            metrics: SignalHealth::new("metrics"),
        }
    }

    fn signal(&self, signal: &str) -> &SignalHealth {
        match signal {
            "traces" => &self.traces,
            "metrics" => &self.metrics,
            _ => &self.logs,
        }
    }

    /// Record one `merge_once` pass. `label` is the loop's own log prefix (`"compactor"`,
    /// `"span-compactor"`, `"metric-compactor"`) so the escalating line reads the same as the
    /// lines around it.
    ///
    /// A pass that merged nothing counts as a success: "there was nothing small enough to merge"
    /// and "merging works" are the same fact here, and treating an idle node as unhealthy would
    /// make the signal useless on exactly the nodes that are fine.
    pub fn record<E: std::fmt::Display>(&self, signal: &str, label: &str, result: Result<(), E>) {
        let s = self.signal(signal);
        match result {
            Ok(()) => {
                let recovered = s.consecutive_failures.swap(0, Ordering::Relaxed);
                s.last_ok_ms.store(now_ms(), Ordering::Relaxed);
                if recovered > 0 {
                    *s.last_error.lock().unwrap() = None;
                    eprintln!("{label}: merge_once recovered after {recovered} failed passes");
                }
            }
            Err(e) => {
                let n = s.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
                let msg = e.to_string();
                *s.last_error.lock().unwrap() = Some(msg.clone());
                if should_log(n) {
                    let since = stalled_for(s.last_ok_ms.load(Ordering::Relaxed));
                    eprintln!("{label}: merge_once failed ({n} in a row{since}): {msg}");
                }
            }
        }
    }
}

impl CompactionStatus for MergeHealth {
    fn snapshot(&self) -> Vec<CompactionHealth> {
        vec![
            self.logs.snapshot(),
            self.traces.snapshot(),
            self.metrics.snapshot(),
        ]
    }
}

/// Print the first three failures, then only powers of two. At one pass per 10 s that is loud for
/// the first half-minute, then 40 s, 80 s, 2.8 min, … 2.8 h, 5.7 h — a stall that lasts days still
/// prints, and a flapping one cannot drown the log.
fn should_log(n: u64) -> bool {
    n <= 3 || n.is_power_of_two()
}

/// `", stalled 3h12m"` for the log line, or `""` when nothing has ever succeeded (start-up
/// failures have no "since" to report, and claiming one would be a made-up duration).
fn stalled_for(last_ok_ms: i64) -> String {
    if last_ok_ms < 0 {
        return String::new();
    }
    let secs = ((now_ms() - last_ok_ms).max(0) / 1000) as u64;
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h > 0 {
        format!(", stalled {h}h{m}m")
    } else {
        format!(", stalled {m}m")
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_node_reports_all_three_signals_with_no_pass_yet() {
        let h = MergeHealth::new();
        let snap = h.snapshot();
        assert_eq!(
            snap.iter().map(|s| s.signal).collect::<Vec<_>>(),
            ["logs", "traces", "metrics"]
        );
        for s in &snap {
            assert_eq!(s.last_ok_ms, None);
            assert_eq!(s.consecutive_failures, 0);
            assert_eq!(s.last_error, None);
        }
    }

    #[test]
    fn a_successful_pass_stamps_the_time_and_leaves_the_others_alone() {
        let h = MergeHealth::new();
        h.record("logs", "compactor", Ok::<(), String>(()));
        let snap = h.snapshot();
        assert!(snap[0].last_ok_ms.is_some());
        assert_eq!(snap[1].last_ok_ms, None, "traces must not move");
        assert_eq!(snap[2].last_ok_ms, None, "metrics must not move");
    }

    #[test]
    fn failures_accumulate_and_keep_the_last_message() {
        let h = MergeHealth::new();
        h.record("traces", "span-compactor", Err::<(), _>("first"));
        h.record("traces", "span-compactor", Err::<(), _>("second"));
        let s = h.snapshot()[1].clone();
        assert_eq!(s.consecutive_failures, 2);
        assert_eq!(s.last_error.as_deref(), Some("second"));
        assert_eq!(s.last_ok_ms, None);
    }

    #[test]
    fn a_success_clears_the_streak_and_the_stale_error() {
        let h = MergeHealth::new();
        h.record("metrics", "metric-compactor", Err::<(), _>("boom"));
        h.record("metrics", "metric-compactor", Ok::<(), String>(()));
        let s = h.snapshot()[2].clone();
        assert_eq!(s.consecutive_failures, 0);
        assert_eq!(
            s.last_error, None,
            "a recovered merge must not keep showing the old error"
        );
        assert!(s.last_ok_ms.is_some());
    }

    #[test]
    fn the_three_signals_do_not_share_state() {
        let h = MergeHealth::new();
        h.record("logs", "compactor", Err::<(), _>("logs are broken"));
        h.record("traces", "span-compactor", Ok::<(), String>(()));
        let snap = h.snapshot();
        assert_eq!(snap[0].consecutive_failures, 1);
        assert_eq!(snap[1].consecutive_failures, 0);
        assert_eq!(snap[2].consecutive_failures, 0);
    }

    #[test]
    fn the_escalation_prints_early_then_thins_out() {
        // The point of the ladder: never silent, never one line per 10 s forever.
        let printed: Vec<u64> = (1..=64).filter(|n| should_log(*n)).collect();
        assert_eq!(printed, [1, 2, 3, 4, 8, 16, 32, 64]);
        assert!(!should_log(1_000), "a day-long stall must not spam");
        assert!(should_log(1_024));
    }

    #[test]
    fn a_stall_before_any_success_reports_no_duration() {
        assert_eq!(stalled_for(-1), "", "there is no 'since' to report yet");
        assert!(stalled_for(now_ms() - 7_200_000).starts_with(", stalled 2h"));
        assert_eq!(stalled_for(now_ms()), ", stalled 0m");
    }
}

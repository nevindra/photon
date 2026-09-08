//! Container stats arithmetic, shared by the inventory (one `stats?one-shot=true` per running
//! container per sync) and the live stats stream. Docker reports cumulative counters — CPU
//! nanoseconds, bytes on the wire, bytes to disk — so every rate is a delta against the
//! previous reading of the SAME container, which `RateTracker` remembers across syncs. Pure
//! numbers in, so it needs no daemon to test.
use std::collections::HashMap;

/// One CPU reading: the container's cumulative CPU time, the host's cumulative CPU time, and
/// how many CPUs it may use — Docker's own `docker stats` formula inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSample {
    pub total_ns: u64,
    pub system_ns: u64,
    pub online_cpus: u32,
}

/// `docker stats`' CPU %: the container's share of the host's CPU time over the interval,
/// scaled by the CPU count so a container saturating 2 of 8 cores reads 200%, not 25%.
pub fn cpu_pct(cur: &CpuSample, prev: &CpuSample) -> Option<f64> {
    let cpu_delta = cur.total_ns.checked_sub(prev.total_ns)? as f64;
    let sys_delta = cur.system_ns.checked_sub(prev.system_ns)? as f64;
    if sys_delta <= 0.0 {
        return None;
    }
    let cpus = if cur.online_cpus > 0 {
        cur.online_cpus as f64
    } else {
        1.0
    };
    Some((cpu_delta / sys_delta * cpus * 100.0 * 100.0).round() / 100.0)
}

/// Cumulative byte counters at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Counters {
    pub ts_ms: i64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub block_read: u64,
    pub block_write: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rates {
    pub cpu_pct: Option<f64>,
    pub net_rx_bps: Option<f64>,
    pub net_tx_bps: Option<f64>,
    pub block_read_bps: Option<f64>,
    pub block_write_bps: Option<f64>,
}

fn bps(cur: u64, prev: u64, dt_ms: i64) -> Option<f64> {
    if dt_ms <= 0 {
        return None;
    }
    let delta = cur.checked_sub(prev)? as f64;
    Some((delta * 1000.0 / dt_ms as f64 * 10.0).round() / 10.0)
}

/// Remembers the previous reading per container id so the next one yields rates. The first
/// reading of a container produces `None`s — there is nothing to diff against yet — which the
/// UI shows as "—" rather than a made-up zero.
#[derive(Debug, Default)]
pub struct RateTracker {
    last: HashMap<String, (Option<CpuSample>, Counters)>,
}

impl RateTracker {
    pub fn update(&mut self, id: &str, cpu: Option<CpuSample>, counters: Counters) -> Rates {
        let rates = match self.last.get(id) {
            Some((prev_cpu, prev)) => Rates {
                cpu_pct: match (cpu.as_ref(), prev_cpu.as_ref()) {
                    (Some(c), Some(p)) => cpu_pct(c, p),
                    _ => None,
                },
                net_rx_bps: bps(counters.net_rx, prev.net_rx, counters.ts_ms - prev.ts_ms),
                net_tx_bps: bps(counters.net_tx, prev.net_tx, counters.ts_ms - prev.ts_ms),
                block_read_bps: bps(
                    counters.block_read,
                    prev.block_read,
                    counters.ts_ms - prev.ts_ms,
                ),
                block_write_bps: bps(
                    counters.block_write,
                    prev.block_write,
                    counters.ts_ms - prev.ts_ms,
                ),
            },
            None => Rates::default(),
        };
        self.last.insert(id.to_string(), (cpu, counters));
        rates
    }

    /// Drop containers that no longer exist so the map does not grow with every churned id.
    pub fn retain_only(&mut self, live: &[String]) {
        self.last.retain(|id, _| live.iter().any(|l| l == id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_matches_docker_stats_formula() {
        let prev = CpuSample {
            total_ns: 1_000,
            system_ns: 100_000,
            online_cpus: 4,
        };
        let cur = CpuSample {
            total_ns: 11_000,
            system_ns: 200_000,
            online_cpus: 4,
        };
        // 10_000 / 100_000 * 4 cpus = 40%
        assert_eq!(cpu_pct(&cur, &prev), Some(40.0));
        // A counter reset (container restarted) is not a negative percentage.
        assert_eq!(cpu_pct(&prev, &cur), None);
        // No host progress → undefined, not infinity.
        assert_eq!(
            cpu_pct(
                &cur,
                &CpuSample {
                    system_ns: 200_000,
                    ..prev
                }
            ),
            None
        );
    }

    #[test]
    fn tracker_yields_rates_from_the_second_reading() {
        let mut t = RateTracker::default();
        let c0 = Counters {
            ts_ms: 0,
            net_rx: 1000,
            net_tx: 0,
            block_read: 0,
            block_write: 0,
        };
        let first = t.update("c1", None, c0);
        assert_eq!(first, Rates::default());
        let c1 = Counters {
            ts_ms: 2000,
            net_rx: 5000,
            net_tx: 200,
            block_read: 4096,
            block_write: 0,
        };
        let r = t.update("c1", None, c1);
        assert_eq!(r.net_rx_bps, Some(2000.0));
        assert_eq!(r.net_tx_bps, Some(100.0));
        assert_eq!(r.block_read_bps, Some(2048.0));
        assert_eq!(r.block_write_bps, Some(0.0));
        assert_eq!(r.cpu_pct, None);
        t.retain_only(&[]);
        assert_eq!(t.update("c1", None, c1), Rates::default());
    }
}

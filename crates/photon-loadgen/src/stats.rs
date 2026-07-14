//! Thread-safe measurement: atomic counters plus an hdrhistogram of ack latency. Workers
//! record into it from many tasks; the reporter takes a [`Snapshot`] each interval to compute
//! rates and percentiles. These numbers are the deliverable — they answer "can Photon take it?"
//!
//! The counters are signal-neutral: `units` is the logical/rate unit (log records for logs,
//! whole traces for traces) and `spans` is a secondary detail (0 for logs, total spans for
//! traces). The driver attaches display labels; the stats themselves stay generic.

use hdrhistogram::Histogram;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Duration;

pub struct Stats {
    units: AtomicU64,
    spans: AtomicU64,
    requests: AtomicU64,
    bytes: AtomicU64,
    ok: AtomicU64,
    http_errors: AtomicU64,
    transport_errors: AtomicU64,
    latency_us: Mutex<Histogram<u64>>,
    status: Mutex<BTreeMap<u16, u64>>,
}

/// A consistent-enough point-in-time read of [`Stats`] for reporting.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub units: u64,
    pub spans: u64,
    pub requests: u64,
    pub bytes: u64,
    pub ok: u64,
    pub http_errors: u64,
    pub transport_errors: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub status: BTreeMap<u16, u64>,
}

impl Stats {
    pub fn new() -> Stats {
        let mut hist = Histogram::<u64>::new(3).expect("valid sigfig");
        hist.auto(true); // grow bounds instead of erroring on large latencies
        Stats {
            units: AtomicU64::new(0),
            spans: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            ok: AtomicU64::new(0),
            http_errors: AtomicU64::new(0),
            transport_errors: AtomicU64::new(0),
            latency_us: Mutex::new(hist),
            status: Mutex::new(BTreeMap::new()),
        }
    }

    /// Record a completed request. `units`/`spans` count as accepted only on a 2xx response — a
    /// rejected batch never persisted, so it must not inflate throughput.
    pub fn record(&self, status: u16, units: u64, spans: u64, bytes: u64, latency: Duration) {
        self.requests.fetch_add(1, Relaxed);
        self.bytes.fetch_add(bytes, Relaxed);

        let micros = (latency.as_micros() as u64).max(1);
        let _ = self.latency_us.lock().unwrap().record(micros);
        *self.status.lock().unwrap().entry(status).or_insert(0) += 1;

        if (200..300).contains(&status) {
            self.ok.fetch_add(1, Relaxed);
            self.units.fetch_add(units, Relaxed);
            self.spans.fetch_add(spans, Relaxed);
        } else {
            self.http_errors.fetch_add(1, Relaxed);
        }
    }

    /// Record a request that never got an HTTP response (connection reset, timeout, refused).
    pub fn record_transport_error(&self) {
        self.requests.fetch_add(1, Relaxed);
        self.transport_errors.fetch_add(1, Relaxed);
    }

    /// Logical units accepted so far — used by the shutdown coordinator for the `--total` cap.
    pub fn units(&self) -> u64 {
        self.units.load(Relaxed)
    }

    pub fn snapshot(&self) -> Snapshot {
        let (p50, p95, p99) = {
            let h = self.latency_us.lock().unwrap();
            if h.is_empty() {
                (0, 0, 0)
            } else {
                (
                    h.value_at_quantile(0.5),
                    h.value_at_quantile(0.95),
                    h.value_at_quantile(0.99),
                )
            }
        };
        Snapshot {
            units: self.units.load(Relaxed),
            spans: self.spans.load(Relaxed),
            requests: self.requests.load(Relaxed),
            bytes: self.bytes.load(Relaxed),
            ok: self.ok.load(Relaxed),
            http_errors: self.http_errors.load(Relaxed),
            transport_errors: self.transport_errors.load(Relaxed),
            p50_us: p50,
            p95_us: p95,
            p99_us: p99,
            status: self.status.lock().unwrap().clone(),
        }
    }
}

impl Default for Stats {
    fn default() -> Self {
        Stats::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_accepted_units_and_categorizes_status() {
        let s = Stats::new();
        s.record(200, 500, 0, 1000, Duration::from_millis(2));
        s.record(200, 500, 0, 1000, Duration::from_millis(4));
        s.record(503, 500, 0, 1000, Duration::from_millis(50));
        s.record_transport_error();

        let snap = s.snapshot();
        assert_eq!(snap.units, 1000, "only 2xx batches count as accepted");
        assert_eq!(snap.requests, 4);
        assert_eq!(snap.ok, 2);
        assert_eq!(snap.http_errors, 1);
        assert_eq!(snap.transport_errors, 1);
        assert_eq!(snap.status.get(&200), Some(&2));
        assert_eq!(snap.status.get(&503), Some(&1));
    }

    #[test]
    fn tracks_spans_secondary_counter_independently() {
        let s = Stats::new();
        s.record(200, 20, 240, 4096, Duration::from_millis(3));
        s.record(500, 20, 240, 4096, Duration::from_millis(9));

        let snap = s.snapshot();
        assert_eq!(snap.units, 20, "only the accepted request's traces count");
        assert_eq!(snap.spans, 240, "only the accepted request's spans count");
    }

    #[test]
    fn percentiles_are_within_recorded_range() {
        let s = Stats::new();
        for ms in [1u64, 2, 3, 4, 5, 10, 20, 50, 100] {
            s.record(200, 1, 0, 1, Duration::from_millis(ms));
        }
        let snap = s.snapshot();
        // p50 ~ 5ms, p99 ~ 100ms — assert ordering and rough range in micros.
        assert!(
            snap.p50_us >= 3_000 && snap.p50_us <= 10_000,
            "p50={}",
            snap.p50_us
        );
        assert!(snap.p99_us >= snap.p95_us && snap.p95_us >= snap.p50_us);
        assert!(snap.p99_us <= 200_000);
    }
}

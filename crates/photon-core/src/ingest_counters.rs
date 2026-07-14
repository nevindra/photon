use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic per-signal ingest tallies. Reset to 0 on process restart; the usage sampler
/// snapshots the cumulative values and the API differences them into per-bucket rates.
#[derive(Default)]
pub struct SignalCounter {
    pub rows: AtomicU64,
    pub bytes: AtomicU64,
}
impl SignalCounter {
    pub fn add(&self, rows: u64, bytes: u64) {
        self.rows.fetch_add(rows, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    /// `(rows, bytes)` cumulative snapshot.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.rows.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
        )
    }
}

#[derive(Default)]
pub struct IngestCounters {
    pub logs: SignalCounter,
    pub traces: SignalCounter,
    pub metrics: SignalCounter,
}
impl IngestCounters {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn add_accumulates_and_snapshots_per_signal() {
        let c = IngestCounters::new();
        c.logs.add(10, 100);
        c.logs.add(5, 50);
        c.traces.add(7, 70);
        assert_eq!(c.logs.snapshot(), (15, 150));
        assert_eq!(c.traces.snapshot(), (7, 70));
        assert_eq!(c.metrics.snapshot(), (0, 0));
    }
}

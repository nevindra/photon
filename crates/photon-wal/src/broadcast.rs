//! A `Wal` decorator that fans each successfully-appended `RecordBatch` into a
//! `tokio::broadcast` channel for the live-tail SSE path. Broadcasting is best-effort
//! and happens ONLY after the inner (durable) append succeeds — the ack boundary is
//! never moved onto the broadcast.

use std::sync::Arc;

use arrow::record_batch::RecordBatch;
use tokio::sync::broadcast;

use crate::Wal;
use photon_core::segment::SegmentId;
use photon_core::PhotonError;

/// Decorates a `Wal` implementation, fanning each durably-appended batch out to
/// subscribers of a `tokio::broadcast` channel. Used to feed the live-tail SSE path
/// without moving the ack boundary: subscribers only ever see batches that already
/// made it through `inner.append`.
pub struct BroadcastingWal<W> {
    inner: W,
    tx: broadcast::Sender<Arc<RecordBatch>>,
}

impl<W> BroadcastingWal<W> {
    pub fn new(inner: W, capacity: usize) -> Self {
        // tokio's `broadcast::channel` panics on capacity 0; clamp so a misconfigured
        // `[live].broadcast_capacity = 0` can't take down startup.
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self { inner, tx }
    }

    /// A sender handle for the live hub. `subscribe()` on it yields a receiver.
    pub fn sender(&self) -> broadcast::Sender<Arc<RecordBatch>> {
        self.tx.clone()
    }
}

impl<W: Wal + Send + Sync> Wal for BroadcastingWal<W> {
    async fn append(&self, batch: RecordBatch) -> Result<(), PhotonError> {
        // Clone is O(columns) Arc-bumps. Append the clone so the original can be
        // broadcast without waiting on (or being affected by) subscriber backpressure.
        self.inner.append(batch.clone()).await?;
        // Best-effort: `send` errors only when there are zero receivers — ignore.
        let _ = self.tx.send(Arc::new(batch));
        Ok(())
    }

    async fn sync(&self) -> Result<(), PhotonError> {
        self.inner.sync().await
    }

    fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
        self.inner.list_closed_segments()
    }

    async fn read_segment(&self, id: SegmentId) -> Result<Vec<RecordBatch>, PhotonError> {
        self.inner.read_segment(id).await
    }

    fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError> {
        self.inner.remove_segment(id)
    }
}

#[cfg(test)]
mod tests {
    use crate::Wal;
    use arrow::array::Int32Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use photon_core::segment::SegmentId;
    use photon_core::PhotonError;
    use std::sync::Arc;

    fn one_row_batch(v: i32) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int32, false)]));
        RecordBatch::try_new(schema, vec![Arc::new(Int32Array::from(vec![v]))]).unwrap()
    }

    // A fake Wal that succeeds or fails on append per a flag, recording appends.
    #[derive(Default)]
    struct FakeWal {
        fail: bool,
        appended: std::sync::Mutex<usize>,
    }

    impl Wal for FakeWal {
        async fn append(&self, _batch: RecordBatch) -> Result<(), PhotonError> {
            if self.fail {
                return Err(PhotonError::Wal("boom".into()));
            }
            *self.appended.lock().unwrap() += 1;
            Ok(())
        }

        async fn sync(&self) -> Result<(), PhotonError> {
            Ok(())
        }

        fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
            Ok(Vec::new())
        }

        async fn read_segment(&self, _id: SegmentId) -> Result<Vec<RecordBatch>, PhotonError> {
            Ok(Vec::new())
        }

        fn remove_segment(&self, _id: SegmentId) -> Result<(), PhotonError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn broadcasts_after_successful_append() {
        let w = super::BroadcastingWal::new(FakeWal::default(), 8);
        let mut rx = w.sender().subscribe();
        w.append(one_row_batch(7)).await.unwrap();
        let got = rx.recv().await.unwrap();
        assert_eq!(got.num_rows(), 1);
    }

    #[tokio::test]
    async fn zero_capacity_is_clamped_not_panicking() {
        // capacity 0 would panic tokio's `broadcast::channel`; `new` clamps it to 1 so a
        // misconfigured `[live].broadcast_capacity = 0` can't crash startup.
        let w = super::BroadcastingWal::new(FakeWal::default(), 0);
        let mut rx = w.sender().subscribe();
        w.append(one_row_batch(1)).await.unwrap();
        assert_eq!(rx.recv().await.unwrap().num_rows(), 1);
    }

    #[tokio::test]
    async fn does_not_broadcast_on_append_error() {
        let inner = FakeWal {
            fail: true,
            ..Default::default()
        };
        let w = super::BroadcastingWal::new(inner, 8);
        let mut rx = w.sender().subscribe();
        assert!(w.append(one_row_batch(7)).await.is_err());
        // Nothing was published.
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
}

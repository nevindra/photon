//! photon-wal: durable write-ahead log with group commit.
//!
//! Implemented per the `photon-wal` section of
//! `docs/superpowers/plans/2026-07-01-photon-interface-contracts.md`.
//!
//! A [`DiskWal`] persists [`RecordBatch`](arrow::record_batch::RecordBatch)es as a sequence
//! of self-describing, crc32-checked frames in rotating segment files. Concurrent
//! [`Wal::append`] calls are group-committed: coalesced into a single `fsync` whose
//! completion is the acknowledgement boundary — an append never resolves before the fsync
//! covering its bytes is durable. Rotated ("closed") segments are handed to the compactor
//! via [`Wal::list_closed_segments`]/[`Wal::read_segment`]; a torn tail left by a crash is
//! dropped on recovery. See [`disk`] for the design in detail.

mod broadcast;
mod disk;
mod frame;

pub use broadcast::BroadcastingWal;
pub use disk::DiskWal;

use arrow::record_batch::RecordBatch;
use photon_core::segment::SegmentId;
use photon_core::PhotonError;

/// A durable write-ahead log.
///
/// Consumers (ingest, compact) are generic over `W: Wal` so they can be tested against an
/// in-memory fake; [`DiskWal`] is the production implementation.
pub trait Wal {
    /// Append one batch; resolves once durably fsync'd (group-committed). Ack boundary.
    fn append(
        &self,
        batch: RecordBatch,
    ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send;
    /// Force-flush the current open segment (used on shutdown).
    fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send;
    /// Segment IDs that are closed (rotated) and ready for compaction, ascending.
    fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError>;
    /// Read all recovered batches from a closed segment (torn tail dropped). Async: a
    /// segment can be up to `segment_max_bytes`, and this is driven from the async
    /// compactor loop, so the read must not block a runtime worker thread.
    fn read_segment(
        &self,
        id: SegmentId,
    ) -> impl std::future::Future<Output = Result<Vec<RecordBatch>, PhotonError>> + Send;
    /// Delete a segment after its data is durably compacted. Idempotent.
    fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError>;
}

//! photon-storage: hot + durable object stores and the background replicator.
//!
//! Its place in the crate graph is described in `docs/architecture.md`.
//!
//! - [`Storage`] wraps a hot (local disk) `object_store::ObjectStore` and an optional
//!   durable (S3-compatible) one, plus the fixed object-path scheme for a segment's
//!   compacted Parquet file and its skip index.
//! - [`Replicator`] is a background hot -> durable copier used once a segment has been
//!   compacted, so the manifest entry can be flipped `durable = true` after a successful
//!   upload.

mod replicator;
mod storage;

pub use replicator::Replicator;
pub use storage::Storage;

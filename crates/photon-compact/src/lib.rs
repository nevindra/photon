//! photon-compact: drains closed WAL segments into sorted Parquet + skip index, updates
//! the manifest, and enqueues replication.
//!
//! Implemented per the `photon-compact` section of
//! `docs/superpowers/plans/2026-07-01-photon-interface-contracts.md`.

mod compactor;
mod metrics_compactor;
mod span_compactor;
mod stream;

pub use compactor::Compactor;
pub use metrics_compactor::MetricsCompactor;
pub use span_compactor::SpanCompactor;

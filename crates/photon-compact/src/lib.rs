//! photon-compact: drains closed WAL segments into sorted Parquet + skip index, updates
//! the manifest, and enqueues replication.
//!
//! Its place in the crate graph is described in `docs/architecture.md`.

mod compactor;
mod metrics_compactor;
mod span_compactor;
mod stream;

pub use compactor::Compactor;
pub use metrics_compactor::MetricsCompactor;
pub use span_compactor::SpanCompactor;

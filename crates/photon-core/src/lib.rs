//! photon-core: shared domain types for Photon. No I/O.

use thiserror::Error;

/// Crate-wide error type. Variants are pre-declared for every crate so that
/// downstream crates never need to edit this enum (which would race under
/// parallel development). Each crate uses the variant matching its domain.
#[derive(Debug, Error)]
pub enum PhotonError {
    #[error("invalid config: {0}")]
    Config(String),
    #[error("arrow error: {0}")]
    Arrow(String),
    #[error("serialization error: {0}")]
    Serde(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("wal error: {0}")]
    Wal(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("index error: {0}")]
    Index(String),
    #[error("query error: {0}")]
    Query(String),
    #[error("uptime error: {0}")]
    Uptime(String),
}

pub mod config;
pub mod ingest_counters;
pub mod manifest;
pub mod metric_agg;
pub mod metric_record;
pub mod metric_schema;
pub mod query;
pub mod record;
pub mod retention;
pub mod rum;
pub mod schema;
pub mod segment;
pub mod span_record;
pub mod span_schema;

#[cfg(test)]
mod smoke_tests {
    use super::*;

    #[test]
    fn error_displays() {
        let e = PhotonError::Config("bad".into());
        assert_eq!(e.to_string(), "invalid config: bad");
    }
}

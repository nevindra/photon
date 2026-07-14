//! photon-loadgen: a high-throughput OTLP/HTTP load generator for stress-testing Photon.
//!
//! It POSTs prost-encoded OTLP batches to Photon's `/v1/logs`, `/v1/traces`, and `/v1/metrics`
//! receivers, via a `logs` / `traces` / `metrics` subcommand triple, either at a steady target
//! rate (soak testing) or as fast as concurrency allows (ceiling testing), while continuously
//! reporting achieved throughput, ack latency, and error rates.
//!
//! Exposed as a library so the modules are unit- and integration-testable; the `photon-loadgen`
//! binary (`src/main.rs`) is a thin wiring layer over this API.

pub mod config;
pub mod driver;
pub mod logs;
pub mod metrics;
pub mod payload;
pub mod ratelimit;
pub mod stats;
pub mod traces;
pub mod worker;

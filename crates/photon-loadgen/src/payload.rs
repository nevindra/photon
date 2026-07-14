//! The signal-agnostic payload abstraction the driver + worker run against. Each signal (logs,
//! traces) provides a [`Payload`] that knows how to build one request body and report how many
//! logical units (logs / traces) and spans it carries. This is the only seam between the shared
//! load-driving machinery and the per-signal OTLP encoding.

use rand::rngs::SmallRng;

/// One built request, ready to POST, plus the accepted-unit counts it represents.
pub struct Built {
    /// Prost-encoded OTLP request body.
    pub body: Vec<u8>,
    /// Logical units in this request: log records for logs, whole traces for traces. This is the
    /// unit `--rate` and `--total` are expressed in.
    pub units: u64,
    /// Spans in this request. `0` for logs; the total span count for traces.
    pub spans: u64,
}

/// A source of request bodies for one signal. `cost` is the number of rate-limiter tokens a
/// single request consumes (== the `units` it will report), so pacing is always in the logical
/// unit. Implementations must be cheap to clone the shared state of and safe to call from many
/// worker tasks concurrently (each worker owns its own `rng`).
pub trait Payload: Send + Sync {
    /// Tokens to acquire from the rate limiter before sending one request. Equals the `units`
    /// each [`build`](Payload::build) will report.
    fn cost(&self) -> f64;

    /// Build one request body plus the accepted-unit counts it represents.
    fn build(&self, rng: &mut SmallRng) -> Built;
}

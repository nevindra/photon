//! The sender loop. Each worker owns a seeded RNG and shares one pooled `reqwest::Client`, the
//! rate limiter, the payload source, and the stats sink. It builds a request via the shared
//! [`Payload`], POSTs it with the bearer token, times the round-trip, records the outcome, and
//! repeats until shutdown — awaiting each response before the next send, so `concurrency` is the
//! true in-flight cap and server backpressure naturally slows the client.
//!
//! The loop is signal-agnostic: logs vs traces differ only in the `payload` it drives.

use crate::config::RunConfig;
use crate::payload::Payload;
use crate::ratelimit::RateLimiter;
use crate::stats::Stats;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use reqwest::Client;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Instant;

/// Everything a worker needs, shared across all workers via `Arc`.
pub struct WorkerCtx {
    pub client: Client,
    pub run: Arc<RunConfig>,
    pub payload: Arc<dyn Payload>,
    pub limiter: Arc<RateLimiter>,
    pub stats: Arc<Stats>,
    pub shutdown: Arc<AtomicBool>,
}

pub async fn run_worker(id: u64, ctx: Arc<WorkerCtx>) {
    // Distinct, deterministic seed per worker so payloads vary but runs are reproducible.
    let mut rng = SmallRng::seed_from_u64(0x9E37_79B9_7F4A_7C15 ^ id);
    let cost = ctx.payload.cost();

    while !ctx.shutdown.load(Relaxed) {
        ctx.limiter.acquire(cost).await;
        if ctx.shutdown.load(Relaxed) {
            break;
        }

        let built = ctx.payload.build(&mut rng);
        let bytes = built.body.len() as u64;

        let start = Instant::now();
        let result = ctx
            .client
            .post(&ctx.run.endpoint)
            .bearer_auth(&ctx.run.token)
            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
            .body(built.body)
            .send()
            .await;
        let latency = start.elapsed();

        match result {
            Ok(resp) => ctx.stats.record(
                resp.status().as_u16(),
                built.units,
                built.spans,
                bytes,
                latency,
            ),
            Err(_) => ctx.stats.record_transport_error(),
        }
    }
}

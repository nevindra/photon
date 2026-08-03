//! Tenant-side federation: pushes synthetic health metrics to a central Photon over OTLP
//! (`summary` mode, always on when `[federation]` is present) and, in `full` mode, tees raw
//! ingest batches to central (Task 6, not yet wired here). Turned on iff `Config.federation` is
//! `Some` — a non-federated node runs none of this.
mod otlp;

pub use otlp::build_summary;

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use photon_alerts::store::AlertStore;
use photon_core::config::FederationConfig;
use photon_core::ingest_counters::IngestCounters;
use photon_query::{MetricsQueryEngine, QueryEngine, SpanQueryEngine};
use prost::Message;
use tokio::task::JoinHandle;

/// Shared, lock-free-ish push telemetry read by the `/api/federation/status` seam (Task 7).
#[derive(Default)]
pub struct FederationStats {
    pub last_push_ms: AtomicI64, // 0 = never
    pub last_error: Mutex<Option<String>>,
    pub pushed: AtomicU64,
    pub dropped: AtomicU64, // full-mode tee drops (Task 6 writes it)
    pub queued: AtomicU64,  // full-mode tee queue depth (Task 6)
}

/// Everything the summary pusher reads each tick to build a [`SummarySnapshot`].
#[derive(Clone)]
pub struct SummaryDeps {
    pub counters: Arc<IngestCounters>,
    pub query: QueryEngine,
    pub span_query: SpanQueryEngine,
    pub metrics_query: MetricsQueryEngine,
    pub alerts: Arc<dyn AlertStore>,
}

/// A single tick's synthetic-health snapshot: per-signal cumulative ingest rows/bytes, per-signal
/// hot-tier bytes, and the count of currently-open alert incidents. Pure input to `build_summary`.
pub struct SummarySnapshot {
    pub rows: [(String, u64); 3],
    pub bytes: [(String, u64); 3],
    pub open_incidents: u64,
    pub hot_bytes: [(String, u64); 3],
}

impl SummaryDeps {
    async fn snapshot(&self) -> SummarySnapshot {
        let (lr, lb) = self.counters.logs.snapshot();
        let (tr, tb) = self.counters.traces.snapshot();
        let (mr, mb) = self.counters.metrics.snapshot();
        let open_incidents = self
            .alerts
            .list_open_incidents()
            .await
            .map(|v| v.len() as u64)
            .unwrap_or(0); // never crash the pusher on a store error
        SummarySnapshot {
            rows: [
                ("logs".into(), lr),
                ("traces".into(), tr),
                ("metrics".into(), mr),
            ],
            bytes: [
                ("logs".into(), lb),
                ("traces".into(), tb),
                ("metrics".into(), mb),
            ],
            open_incidents,
            hot_bytes: [
                (
                    "logs".into(),
                    self.query.storage_stats().map(|s| s.bytes).unwrap_or(0),
                ),
                (
                    "traces".into(),
                    self.span_query
                        .storage_stats()
                        .map(|s| s.bytes)
                        .unwrap_or(0),
                ),
                (
                    "metrics".into(),
                    self.metrics_query
                        .storage_stats()
                        .map(|s| s.bytes)
                        .unwrap_or(0),
                ),
            ],
        }
    }
}

/// Spawn the tenant-side summary pusher: every `cfg.interval_secs`, snapshot ingest counters +
/// hot-tier footprint + open incident count, build an OTLP metrics payload (`build_summary`), and
/// POST it to `{cfg.endpoint}/v1/metrics` with the tenant bearer token. Never blocks or fails
/// local ingest: request/build errors only update `stats.last_error` and the loop runs forever —
/// a slow or unreachable central just means skipped pushes.
pub fn spawn_summary_pusher(
    cfg: FederationConfig,
    deps: SummaryDeps,
    stats: Arc<FederationStats>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                *stats.last_error.lock().unwrap() = Some(e.to_string());
                return;
            }
        };
        let mut tick = tokio::time::interval(Duration::from_secs(cfg.interval_secs.max(1)));
        loop {
            tick.tick().await;
            let snapshot = deps.snapshot().await;
            let body = build_summary(&snapshot, cfg.mode).encode_to_vec();
            let res = client
                .post(format!("{}/v1/metrics", cfg.endpoint))
                .bearer_auth(&cfg.token)
                .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
                .body(body)
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {
                    stats.pushed.fetch_add(1, Ordering::Relaxed);
                    stats
                        .last_push_ms
                        .store(photon_alerts::model::now_ms(), Ordering::Relaxed);
                    *stats.last_error.lock().unwrap() = None;
                }
                Ok(r) => {
                    *stats.last_error.lock().unwrap() =
                        Some(format!("central returned {}", r.status()));
                }
                Err(e) => {
                    *stats.last_error.lock().unwrap() = Some(e.to_string());
                }
            }
        }
    })
}

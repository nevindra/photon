//! Tenant-side federation: pushes synthetic health metrics to a central Photon over OTLP
//! (`summary` mode, always on when `[federation]` is present) and, in `full` mode, tees raw
//! ingest batches to central (the [`spawn_forwarder`] drain of `photon-ingest`'s `FederationTee`).
//! Turned on iff `Config.federation` is `Some` — a non-federated node runs none of this.
mod otlp;

pub use otlp::{build_summary, mode_label};

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
    /// Full-mode payloads that never reached central: tee drops (queue full) plus forwarder
    /// give-ups. `Arc` so the tee — which owns its own counter — can share this exact cell.
    pub dropped: Arc<AtomicU64>,
    pub queued: AtomicU64, // full-mode tee queue depth
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
        // Default Burst would fire every tick missed while a push outran the interval (request
        // timeout 10s vs interval_secs min 5) back-to-back the moment central recovers.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let snapshot = deps.snapshot().await;
            let body = build_summary(&snapshot, &mode_label(&cfg)).encode_to_vec();
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

/// Attempt backoffs for one teed payload. Three attempts total; after the last failure the
/// payload is dropped (`stats.dropped`) — federation is a best-effort mirror, never a queue we
/// grow without bound.
const FORWARD_BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];

/// Spawn the tenant-side `full`-mode forwarder: drain the ingest tee and re-POST each raw OTLP
/// payload to `{endpoint}/v1/{logs|traces|metrics}` with the tenant bearer token. HTTP-status
/// failures (central reachable but erroring) are retried up to three times with 250ms/1s backoff
/// and then dropped. Connection-level failures (connect refused / timeout) drop the payload after
/// the FIRST attempt — retrying an unreachable central serializes ~30s of timeouts per payload and
/// head-of-line-blocks the whole queue into overflow; the single attempt per payload doubles as
/// the recovery probe. The loop itself never exits.
pub fn spawn_forwarder(
    cfg: FederationConfig,
    mut rx: tokio::sync::mpsc::Receiver<(photon_ingest::TeeSignal, bytes::Bytes)>,
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
        while let Some((signal, payload)) = rx.recv().await {
            stats.queued.store(rx.len() as u64, Ordering::Relaxed);
            let url = format!("{}/v1/{}", cfg.endpoint, signal.path());
            let mut delivered = false;
            for attempt in 0..=FORWARD_BACKOFF.len() {
                let res = client
                    .post(&url)
                    .bearer_auth(&cfg.token)
                    .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
                    .body(payload.clone())
                    .send()
                    .await;
                match res {
                    Ok(r) if r.status().is_success() => {
                        *stats.last_error.lock().unwrap() = None;
                        delivered = true;
                    }
                    Ok(r) => {
                        *stats.last_error.lock().unwrap() = Some(format!(
                            "central returned {} for {}",
                            r.status(),
                            signal.path()
                        ))
                    }
                    Err(e) => {
                        *stats.last_error.lock().unwrap() = Some(e.to_string());
                        // Central unreachable — give up on this payload immediately (see the
                        // doc comment); the next payload is the probe.
                        break;
                    }
                }
                if delivered {
                    break;
                }
                if let Some(backoff) = FORWARD_BACKOFF.get(attempt) {
                    tokio::time::sleep(*backoff).await;
                }
            }
            if !delivered {
                stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    })
}

#[cfg(test)]
mod forwarder_tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, Uri};
    use bytes::Bytes;
    use photon_core::config::FederationMode;
    use photon_ingest::TeeSignal;
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicBool;

    type Captured = Arc<Mutex<Vec<(String, String, Vec<u8>)>>>;

    #[derive(Clone)]
    struct Stub {
        captured: Captured,
        fail: Arc<AtomicBool>,
    }

    async fn capture(
        State(stub): State<Stub>,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        let auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        stub.captured
            .lock()
            .unwrap()
            .push((uri.path().to_string(), auth, body.to_vec()));
        if stub.fail.load(Ordering::Relaxed) {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::OK
        }
    }

    /// Boot an in-process axum stub that records every POST it receives.
    async fn stub_central() -> (SocketAddr, Captured, Arc<AtomicBool>) {
        let stub = Stub {
            captured: Arc::new(Mutex::new(Vec::new())),
            fail: Arc::new(AtomicBool::new(false)),
        };
        let app = axum::Router::new()
            .fallback(axum::routing::post(capture))
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (addr, stub.captured, stub.fail)
    }

    fn cfg(addr: SocketAddr) -> FederationConfig {
        FederationConfig {
            endpoint: format!("http://{addr}"),
            token: "tk_tenant_x".to_string(),
            mode: FederationMode::Full,
            signals: None,
            interval_secs: 30,
            queue_batches: 16,
        }
    }

    /// Wait until `f` holds, or fail the test after ~5s.
    async fn eventually(mut f: impl FnMut() -> bool) {
        for _ in 0..500 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition never became true");
    }

    #[tokio::test]
    async fn forwards_each_signal_to_its_path_with_the_tenant_token() {
        let (addr, captured, _fail) = stub_central().await;
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let stats = Arc::new(FederationStats::default());
        spawn_forwarder(cfg(addr), rx, stats.clone());

        tx.send((TeeSignal::Traces, Bytes::from_static(b"span-payload")))
            .await
            .unwrap();
        eventually(|| captured.lock().unwrap().len() == 1).await;

        let got = captured.lock().unwrap()[0].clone();
        assert_eq!(got.0, "/v1/traces");
        assert_eq!(got.1, "Bearer tk_tenant_x");
        assert_eq!(got.2, b"span-payload");
        assert_eq!(stats.dropped.load(Ordering::Relaxed), 0);
    }

    /// An unreachable central (connect refused) costs each payload a single attempt, not three
    /// timeout-bound retries — the queue must not head-of-line block while central is down.
    #[tokio::test]
    async fn connection_failure_drops_immediately_without_retries() {
        // Bind then drop a listener: a port that actively refuses connections.
        let addr = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap()
        };
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let stats = Arc::new(FederationStats::default());
        spawn_forwarder(cfg(addr), rx, stats.clone());

        let started = std::time::Instant::now();
        tx.send((TeeSignal::Logs, Bytes::from_static(b"a")))
            .await
            .unwrap();
        tx.send((TeeSignal::Logs, Bytes::from_static(b"b")))
            .await
            .unwrap();
        eventually(|| stats.dropped.load(Ordering::Relaxed) == 2).await;
        // With the old retry path this takes >= 2 * (250ms + 1s) of backoff alone.
        assert!(
            started.elapsed() < Duration::from_millis(1250),
            "connect failures must not burn the backoff schedule: {:?}",
            started.elapsed()
        );
    }

    /// A central that keeps 500ing costs the payload after three attempts — but never the loop:
    /// the next payload is delivered once the stub recovers.
    #[tokio::test]
    async fn gives_up_after_three_attempts_and_stays_alive() {
        let (addr, captured, fail) = stub_central().await;
        fail.store(true, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let stats = Arc::new(FederationStats::default());
        spawn_forwarder(cfg(addr), rx, stats.clone());

        tx.send((TeeSignal::Logs, Bytes::from_static(b"doomed")))
            .await
            .unwrap();
        eventually(|| stats.dropped.load(Ordering::Relaxed) == 1).await;
        assert_eq!(
            captured.lock().unwrap().len(),
            3,
            "three attempts, then drop"
        );

        fail.store(false, Ordering::Relaxed);
        tx.send((TeeSignal::Metrics, Bytes::from_static(b"recovered")))
            .await
            .unwrap();
        eventually(|| captured.lock().unwrap().len() == 4).await;
        let got = captured.lock().unwrap()[3].clone();
        assert_eq!(got.0, "/v1/metrics");
        assert_eq!(got.2, b"recovered");
        assert_eq!(stats.dropped.load(Ordering::Relaxed), 1);
    }
}

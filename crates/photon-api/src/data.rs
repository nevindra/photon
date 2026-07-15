//! Data & retention REST surface: storage overview, editable per-signal retention, and manual
//! purge. Parquet-signal deletes are routed to the compactors (sole manifest writers) over an
//! mpsc channel; uptime deletes go through the uptime store. Retention lives in `AtomicU32`s
//! (live values the compactor loops read) backed by the `settings` table (persistence).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::settings::SettingsStore;
use crate::AppState;
use photon_core::config::MAX_RETENTION_DAYS;
use photon_core::retention::PurgeReport;
use photon_core::PhotonError;

/// A purge request sent to a compactor loop; it runs `purge_before(cutoff_nanos)` and replies.
pub struct PurgeCommand {
    pub cutoff_nanos: i64,
    pub reply: oneshot::Sender<Result<PurgeReport, PhotonError>>,
}
pub type PurgeSender = mpsc::Sender<PurgeCommand>;

/// Live per-signal retention (days). Seeded at startup; the compactor loops read these on the
/// hourly purge tick; `PUT /retention` updates them.
#[derive(Default)]
pub struct RetentionAtomics {
    pub logs: AtomicU32,
    pub traces: AtomicU32,
    pub metrics: AtomicU32,
    pub uptime: AtomicU32,
}

/// The data-admin handle attached to `AppState`. `None` disables the routes' mutating paths.
#[derive(Clone)]
pub struct DataAdmin {
    pub purge_logs: PurgeSender,
    pub purge_traces: PurgeSender,
    pub purge_metrics: PurgeSender,
    pub retention: Arc<RetentionAtomics>,
    pub settings: Arc<dyn SettingsStore>,
    /// Whether uptime is enabled (controls whether the `uptime` key appears / is mutable).
    pub uptime_enabled: bool,
    /// Global Apdex threshold T (ms) for services without a per-service override.
    pub apdex_default_ms: u32,
}

fn err_json(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": msg.into() })))
}

// ---- GET /storage -------------------------------------------------------------------------

/// `st.usage`'s reported durable footprint for `signal`, or `0` when usage tracking is disabled
/// or the store errors (durable accounting is best-effort, never fatal to `/storage`).
async fn durable_bytes_for(st: &AppState, signal: &str) -> u64 {
    match st.usage.as_ref() {
        Some(u) => u.durable_bytes(signal).await.unwrap_or(0),
        None => 0,
    }
}

pub(crate) async fn storage(
    State(st): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let logs = st
        .query
        .storage_stats()
        .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let traces = st
        .span_query
        .storage_stats()
        .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let metrics = st
        .metrics_query
        .storage_stats()
        .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut logs_v = serde_json::to_value(logs).unwrap();
    logs_v["durable_bytes"] = serde_json::json!(durable_bytes_for(&st, "logs").await);
    let mut traces_v = serde_json::to_value(traces).unwrap();
    traces_v["durable_bytes"] = serde_json::json!(durable_bytes_for(&st, "traces").await);
    let mut metrics_v = serde_json::to_value(metrics).unwrap();
    metrics_v["durable_bytes"] = serde_json::json!(durable_bytes_for(&st, "metrics").await);

    let mut signals =
        serde_json::json!({ "logs": logs_v, "traces": traces_v, "metrics": metrics_v });
    if let Some(up) = st.uptime.as_ref() {
        let s = up
            .store
            .stats()
            .await
            .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        signals["uptime"] = serde_json::to_value(s).unwrap();
    }

    let (configured, pending) = match st.replication.as_ref() {
        Some(r) => (r.configured(), r.pending() as u64),
        None => (false, 0),
    };
    let last_replicated_ms = match st.usage.as_ref() {
        Some(u) => u.last_replicated_ms().await.unwrap_or(None),
        None => None,
    };

    Ok(Json(serde_json::json!({
        "signals": signals,
        "durable": { "configured": configured, "pending": pending, "last_replicated_ms": last_replicated_ms },
    })))
}

// ---- GET/PUT /retention -------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct RetentionView {
    logs: u32,
    traces: u32,
    metrics: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    uptime: Option<u32>,
}

fn view(data: &DataAdmin) -> RetentionView {
    RetentionView {
        logs: data.retention.logs.load(Ordering::Relaxed),
        traces: data.retention.traces.load(Ordering::Relaxed),
        metrics: data.retention.metrics.load(Ordering::Relaxed),
        uptime: data
            .uptime_enabled
            .then(|| data.retention.uptime.load(Ordering::Relaxed)),
    }
}

pub(crate) async fn get_retention(
    State(st): State<AppState>,
) -> Result<Json<RetentionView>, (StatusCode, Json<serde_json::Value>)> {
    let data = st
        .data
        .as_ref()
        .ok_or_else(|| err_json(StatusCode::NOT_FOUND, "data admin disabled"))?;
    Ok(Json(view(data)))
}

#[derive(Deserialize)]
pub(crate) struct RetentionPatch {
    logs: Option<u32>,
    traces: Option<u32>,
    metrics: Option<u32>,
    uptime: Option<u32>,
}

pub(crate) async fn put_retention(
    State(st): State<AppState>,
    Json(patch): Json<RetentionPatch>,
) -> Result<Json<RetentionView>, (StatusCode, Json<serde_json::Value>)> {
    let data = st
        .data
        .as_ref()
        .ok_or_else(|| err_json(StatusCode::NOT_FOUND, "data admin disabled"))?;
    // Validate then apply each provided field.
    for (signal, val, atomic) in [
        ("logs", patch.logs, &data.retention.logs),
        ("traces", patch.traces, &data.retention.traces),
        ("metrics", patch.metrics, &data.retention.metrics),
        ("uptime", patch.uptime, &data.retention.uptime),
    ] {
        if let Some(days) = val {
            if days == 0 {
                return Err(err_json(
                    StatusCode::BAD_REQUEST,
                    "retention days must be > 0",
                ));
            }
            if days > MAX_RETENTION_DAYS {
                return Err(err_json(
                    StatusCode::BAD_REQUEST,
                    format!("retention days must be <= {MAX_RETENTION_DAYS} (100 years)"),
                ));
            }
            if signal == "uptime" && !data.uptime_enabled {
                return Err(err_json(StatusCode::BAD_REQUEST, "uptime is disabled"));
            }
            data.settings
                .set_retention(signal, days)
                .await
                .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            atomic.store(days, Ordering::Relaxed);
        }
    }
    Ok(Json(view(data)))
}

// ---- POST /data/purge ---------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct PurgeReq {
    signal: String,
    mode: String,
    before_ms: Option<i64>,
}

pub(crate) async fn purge(
    State(st): State<AppState>,
    Json(req): Json<PurgeReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let data = st
        .data
        .as_ref()
        .ok_or_else(|| err_json(StatusCode::NOT_FOUND, "data admin disabled"))?;
    // Resolve the cutoff (ms). "all" ⇒ everything; "before" ⇒ the provided timestamp.
    let cutoff_ms: i64 = match req.mode.as_str() {
        "all" => i64::MAX,
        "before" => req
            .before_ms
            .ok_or_else(|| err_json(StatusCode::BAD_REQUEST, "before_ms required"))?,
        _ => {
            return Err(err_json(
                StatusCode::BAD_REQUEST,
                "mode must be 'all' or 'before'",
            ))
        }
    };

    if req.signal == "uptime" {
        let up = st
            .uptime
            .as_ref()
            .ok_or_else(|| err_json(StatusCode::NOT_FOUND, "uptime disabled"))?;
        let hb = up
            .store
            .prune_heartbeats(cutoff_ms)
            .await
            .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let inc = up
            .store
            .prune_incidents(cutoff_ms)
            .await
            .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(
            serde_json::json!({ "heartbeats_removed": hb, "incidents_removed": inc }),
        ));
    }

    let sender = match req.signal.as_str() {
        "logs" => &data.purge_logs,
        "traces" => &data.purge_traces,
        "metrics" => &data.purge_metrics,
        _ => return Err(err_json(StatusCode::BAD_REQUEST, "unknown signal")),
    };
    // ms → nanos (saturating so i64::MAX stays "delete all").
    let cutoff_nanos = cutoff_ms.saturating_mul(1_000_000);
    let (tx, rx) = oneshot::channel();
    sender
        .send(PurgeCommand {
            cutoff_nanos,
            reply: tx,
        })
        .await
        .map_err(|_| err_json(StatusCode::INTERNAL_SERVER_ERROR, "compactor unavailable"))?;
    let report = rx
        .await
        .map_err(|_| err_json(StatusCode::INTERNAL_SERVER_ERROR, "purge cancelled"))?
        .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        serde_json::json!({ "files_removed": report.files_removed, "rows_removed": report.rows_removed }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt; // for `oneshot`

    use crate::{ReplicationStatus, UsageSampleRow, UsageStore};

    /// A `UsageStore` fake that reports a fixed durable footprint for `logs` (0 for every other
    /// signal) and a fixed last-replicated timestamp; every other method is a no-op success.
    struct FakeUsage;

    #[async_trait::async_trait]
    impl UsageStore for FakeUsage {
        async fn insert_sample(&self, _s: &UsageSampleRow) -> Result<(), PhotonError> {
            Ok(())
        }
        async fn prune_samples(&self, _before_ms: i64) -> Result<(), PhotonError> {
            Ok(())
        }
        async fn series(
            &self,
            _start_ms: i64,
            _end_ms: i64,
        ) -> Result<Vec<UsageSampleRow>, PhotonError> {
            Ok(Vec::new())
        }
        async fn record_replicated(
            &self,
            _path: &str,
            _signal: &str,
            _bytes: u64,
            _ts_ms: i64,
        ) -> Result<(), PhotonError> {
            Ok(())
        }
        async fn durable_bytes(&self, signal: &str) -> Result<u64, PhotonError> {
            Ok(match signal {
                "logs" => 176_000_000,
                _ => 0,
            })
        }
        async fn last_replicated_ms(&self) -> Result<Option<i64>, PhotonError> {
            Ok(Some(1_751_000_000_000))
        }
    }

    /// A `ReplicationStatus` fake reporting "configured, 3 pending".
    struct FakeRepl;

    impl ReplicationStatus for FakeRepl {
        fn configured(&self) -> bool {
            true
        }
        fn pending(&self) -> usize {
            3
        }
    }

    /// Seeded server with a fake usage store + replication status attached, routed.
    fn router_with_usage() -> axum::Router {
        crate::test_server()
            .with_usage(Arc::new(FakeUsage), Arc::new(FakeRepl))
            .into_router()
    }

    /// Build a `DataAdmin` whose three purge channels are each drained by a stub task that
    /// replies `PurgeReport { files_removed: 1, rows_removed: 5 }` — mimicking a compactor loop.
    fn stub_data_admin() -> DataAdmin {
        fn drain(mut rx: mpsc::Receiver<PurgeCommand>) {
            tokio::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    let _ = cmd.reply.send(Ok(PurgeReport {
                        files_removed: 1,
                        rows_removed: 5,
                    }));
                }
            });
        }
        let (logs_tx, logs_rx) = mpsc::channel(8);
        let (traces_tx, traces_rx) = mpsc::channel(8);
        let (metrics_tx, metrics_rx) = mpsc::channel(8);
        drain(logs_rx);
        drain(traces_rx);
        drain(metrics_rx);
        DataAdmin {
            purge_logs: logs_tx,
            purge_traces: traces_tx,
            purge_metrics: metrics_tx,
            retention: Arc::new(RetentionAtomics {
                logs: AtomicU32::new(30),
                traces: AtomicU32::new(14),
                metrics: AtomicU32::new(7),
                uptime: AtomicU32::new(30),
            }),
            settings: Arc::new(crate::settings::SqliteSettingsStore::open_in_memory().unwrap()),
            uptime_enabled: false,
            apdex_default_ms: photon_core::config::DEFAULT_APDEX_THRESHOLD_MS,
        }
    }

    /// Seeded server + a `DataAdmin` attached, as a routed axum `Router`.
    fn router_with_data_admin() -> axum::Router {
        crate::test_server()
            .with_data_admin(Some(stub_data_admin()))
            .into_router()
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn retention_get_put_and_purge_flow() {
        let app = router_with_data_admin();
        let cookie = crate::session_cookie(&app).await;

        // PUT /retention { "logs": 10 } → 200, body reflects logs: 10.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/retention")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"logs":10}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await["logs"], 10);

        // GET /retention reflects the updated value.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/retention")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await["logs"], 10);

        // PUT /retention { "logs": 0 } → 400.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/retention")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"logs":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // PUT /retention { "logs": 999999999 } → 400 (above the 100-year cap; a user typing this
        // to mean "forever" must not overflow the i64 cutoff arithmetic in the retention loops).
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/retention")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"logs":999999999}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // POST /data/purge { "signal": "logs", "mode": "all" } → report from the stub compactor.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/data/purge")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"signal":"logs","mode":"all"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_json(resp).await;
        assert_eq!(out["files_removed"], 1);
        assert_eq!(out["rows_removed"], 5);
    }

    #[tokio::test]
    async fn storage_reshape() {
        let app = router_with_usage();
        let cookie = crate::session_cookie(&app).await;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/storage")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;

        assert_eq!(v["signals"]["logs"]["durable_bytes"], json!(176_000_000));
        assert_eq!(v["signals"]["traces"]["durable_bytes"], json!(0));
        assert_eq!(v["signals"]["metrics"]["durable_bytes"], json!(0));
        assert_eq!(v["durable"]["configured"], json!(true));
        assert_eq!(v["durable"]["pending"], json!(3));
        assert_eq!(
            v["durable"]["last_replicated_ms"],
            json!(1_751_000_000_000i64)
        );
    }
}

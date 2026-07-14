//! `GET /api/services/:service/{timeseries,dependencies,settings}` — the per-service Services
//! (APM) detail-page endpoints: a bucketed RED+Apdex time series, downstream dependency rows
//! (database/external), and the per-service Apdex-threshold override (read/write/clear).
//!
//! Thin handlers mirroring `red.rs`/`traces_agg.rs`: they build a `SpanQueryRequest` scoped to
//! one service via `build_span_query_request`, delegate the heavy lifting to
//! `SpanQueryEngine::{red_timeseries, dependencies}`, and derive window-relative `rate`/
//! `error_rate` here (the engine stays window-agnostic beyond pruning) — same division of labor
//! as `/api/red` and `/api/traces/dependencies`'s sibling `/api/red`.
//!
//! **Service-name query escaping.** The per-service filter is built as the bare, unquoted grammar
//! term `service.name:<service>` (e.g. `service.name:payment-service`), never
//! `service.name:"<service>"`. Verified against `photon_core::query::parser`: `classify()` only
//! treats whitespace, a leading `-` (negation), a leading `"` (whole-token quoted phrase), `:`
//! (field/value split), and `,` (OR-list split) specially — dots and dashes in a *value* have no
//! special meaning, so `service.name:payment-service` already parses as a single-value `Match`
//! term with no quoting needed. Quoting would in fact be *wrong* here: the tokenizer only
//! recognizes a quoted span when the `"` is the token's own leading character (i.e. a bare
//! quoted phrase, which classifies as `FreeText`, not a field `Match`); inside `field:"value"` the
//! quotes are never stripped by `classify` (it just calls `body.split_once(':')` on the whole
//! token), so the resolved `Match` value would literally contain the quote characters and never
//! match real attribute values. See `photon_core/src/query/parser.rs`'s `tokenize`/`classify`.
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::DepRow;

use crate::query_params::build_span_query_request;
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct TimeseriesParams {
    start: String,
    end: String,
    #[serde(default = "default_buckets")]
    buckets: usize,
}

fn default_buckets() -> usize {
    48
}

/// `GET /api/services/:service/timeseries?start&end&buckets` — `buckets` equal-width RED+Apdex
/// buckets over `[start, end]` for one service, using that service's resolved Apdex threshold
/// (per-service override, else the global default).
pub(crate) async fn timeseries(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Query(p): Query<TimeseriesParams>,
) -> Response {
    let req = match build_span_query_request(
        &service_query(&service),
        &p.start,
        &p.end,
        "recent",
        0,
        0,
        state.span_query.promoted_attributes(),
    ) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    // Capture the window before `req` is moved into `red_timeseries` — it powers the per-bucket
    // rate denominator.
    let (start_ns, end_ns) = (req.start_ts_nanos, req.end_ts_nanos);
    let buckets = p.buckets.max(1);
    let secs_per_bucket = bucket_seconds(start_ns, end_ns, buckets);
    let (threshold_ms, _is_default) = resolve_threshold(&state, &service).await;

    match state
        .span_query
        .red_timeseries(req, start_ns, end_ns, buckets, threshold_ms)
        .await
    {
        Ok(rows) => Json(
            rows.iter()
                .map(|b| {
                    json!({
                        "ts": b.ts.to_string(),
                        "rate": b.count as f64 / secs_per_bucket,
                        "error_rate": if b.count > 0 { b.error_count as f64 / b.count as f64 } else { 0.0 },
                        "count": b.count,
                        "error_count": b.error_count,
                        "p50": b.p50.to_string(),
                        "p90": b.p90.to_string(),
                        "p99": b.p99.to_string(),
                        "apdex": apdex_of(b.satisfied, b.tolerating, b.frustrated),
                        "satisfied": b.satisfied,
                        "tolerating": b.tolerating,
                        "frustrated": b.frustrated,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct DependenciesParams {
    start: String,
    end: String,
}

/// `GET /api/services/:service/dependencies?start&end` — one service's downstream database and
/// external call dependencies, with `rate`/`error_rate` derived here from the raw counts and the
/// query window (the engine itself stays window-agnostic).
pub(crate) async fn dependencies(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Query(p): Query<DependenciesParams>,
) -> Response {
    let req = match build_span_query_request(
        &service_query(&service),
        &p.start,
        &p.end,
        "recent",
        0,
        0,
        state.span_query.promoted_attributes(),
    ) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let window_secs = window_seconds(req.start_ts_nanos, req.end_ts_nanos);

    match state.span_query.dependencies(req).await {
        Ok(deps) => Json(json!({
            "database": deps.database.iter().map(|r| dep_row_to_json(r, window_secs)).collect::<Vec<_>>(),
            "external": deps.external.iter().map(|r| dep_row_to_json(r, window_secs)).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

fn dep_row_to_json(r: &DepRow, window_secs: f64) -> Value {
    let error_rate = if r.count > 0 {
        r.error_count as f64 / r.count as f64
    } else {
        0.0
    };
    json!({
        "name": r.name,
        "system": r.system,
        "count": r.count,
        "rate": r.count as f64 / window_secs,
        "error_count": r.error_count,
        "error_rate": error_rate,
        "p50": r.p50.to_string(),
        "p95": r.p95.to_string(),
        "p99": r.p99.to_string(),
    })
}

#[derive(Deserialize)]
pub(crate) struct SettingsBody {
    apdex_threshold_ms: u32,
}

/// `GET /api/services/:service/settings` — the resolved Apdex threshold (per-service override,
/// else the global default) + whether it is the default. Works even when `state.data` is `None`
/// (falls back to `photon_core::config::DEFAULT_APDEX_THRESHOLD_MS`, `is_default: true`).
pub(crate) async fn get_settings(
    State(state): State<AppState>,
    Path(service): Path<String>,
) -> Response {
    let (apdex_threshold_ms, is_default) = resolve_threshold(&state, &service).await;
    Json(json!({ "apdex_threshold_ms": apdex_threshold_ms, "is_default": is_default }))
        .into_response()
}

/// `PUT /api/services/:service/settings` — set a per-service Apdex override. `400` if
/// `apdex_threshold_ms == 0`; `404` if the data-admin layer (and thus the settings store) is
/// disabled.
pub(crate) async fn put_settings(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Json(body): Json<SettingsBody>,
) -> Response {
    if body.apdex_threshold_ms == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "apdex_threshold_ms must be > 0" })),
        )
            .into_response();
    }
    let Some(data) = state.data.as_ref() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "data admin disabled" })),
        )
            .into_response();
    };
    match data
        .settings
        .set_apdex_threshold(&service, body.apdex_threshold_ms)
        .await
    {
        Ok(()) => Json(json!({
            "apdex_threshold_ms": body.apdex_threshold_ms,
            "is_default": false,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `DELETE /api/services/:service/settings` — clear a per-service Apdex override, reverting to
/// the global default. `404` if the data-admin layer is disabled.
pub(crate) async fn delete_settings(
    State(state): State<AppState>,
    Path(service): Path<String>,
) -> Response {
    let Some(data) = state.data.as_ref() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "data admin disabled" })),
        )
            .into_response();
    };
    match data.settings.clear_apdex_threshold(&service).await {
        Ok(()) => Json(json!({
            "apdex_threshold_ms": data.apdex_default_ms,
            "is_default": true,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// The bare, unquoted `service.name:<service>` grammar term (see the module-level doc comment for
/// why quoting would be wrong here).
fn service_query(service: &str) -> String {
    format!("service.name:{service}")
}

/// Resolve the effective Apdex threshold (milliseconds) for `service`: a per-service override
/// from the settings store if one exists, else the configured (or hard-coded) default. The
/// second element of the tuple is `true` when no per-service override is set (i.e. the returned
/// value is a default, not an explicit override) — surfaced to the UI as `is_default`.
pub(crate) async fn resolve_threshold(state: &AppState, service: &str) -> (u32, bool) {
    match &state.data {
        Some(d) => match d
            .settings
            .service_apdex_threshold(service)
            .await
            .ok()
            .flatten()
        {
            Some(ms) => (ms, false),
            None => (d.apdex_default_ms, true),
        },
        None => (photon_core::config::DEFAULT_APDEX_THRESHOLD_MS, true),
    }
}

/// Apdex score from Satisfied/Tolerating/Frustrated band counts: `(sat + tol/2) / (sat+tol+fru)`.
/// `None` when the denominator is zero (no banded spans in the window) — the same formula as
/// `red::red_row_to_json`'s inline apdex, factored here so the timeseries/settings handlers in
/// this file share one implementation.
pub(crate) fn apdex_of(satisfied: u64, tolerating: u64, frustrated: u64) -> Option<f64> {
    let banded = satisfied + tolerating + frustrated;
    if banded == 0 {
        None
    } else {
        Some((satisfied as f64 + tolerating as f64 / 2.0) / banded as f64)
    }
}

/// Window length in seconds, floored at 1ns, matching `red.rs`'s `window_seconds` (kept file-local
/// here — it's a trivial one-liner, not worth a shared module for).
fn window_seconds(start_ns: i64, end_ns: i64) -> f64 {
    let span_ns = (end_ns - start_ns).max(1) as f64;
    span_ns / 1_000_000_000.0
}

/// The width, in seconds, of one of `buckets` equal-width buckets spanning `[start_ns, end_ns]`
/// (mirrors `red_timeseries::bucket_width`, expressed in seconds for the rate denominator).
fn bucket_seconds(start_ns: i64, end_ns: i64, buckets: usize) -> f64 {
    window_seconds(start_ns, end_ns) / buckets.max(1) as f64
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;

    use tokio::sync::mpsc;
    use tower::ServiceExt;

    use crate::{DataAdmin, RetentionAtomics};

    /// A `DataAdmin` with an in-memory `SettingsStore` and un-drained purge channels (these tests
    /// never hit `/api/data/purge`), mirroring `data.rs`'s `stub_data_admin`.
    fn stub_data_admin() -> DataAdmin {
        let (purge_logs, _rx1) = mpsc::channel(1);
        let (purge_traces, _rx2) = mpsc::channel(1);
        let (purge_metrics, _rx3) = mpsc::channel(1);
        DataAdmin {
            purge_logs,
            purge_traces,
            purge_metrics,
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

    /// A seeded server with a `DataAdmin` attached, as a routed axum `Router`. Built fresh per
    /// test (not per request) — callers `.clone()` it across the several requests in one test so
    /// the in-memory settings store's state persists between them (`Router`/`AppState` clone is a
    /// cheap `Arc` clone, same underlying `SqliteSettingsStore` connection).
    fn router_with_data_admin() -> axum::Router {
        crate::test_server()
            .with_data_admin(Some(stub_data_admin()))
            .into_router()
    }

    async fn get(router: &axum::Router, uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
        let cookie = crate::session_cookie(router).await;
        let resp = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn put_json(
        router: &axum::Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let cookie = crate::session_cookie(router).await;
        let resp = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn delete(
        router: &axum::Router,
        uri: &str,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let cookie = crate::session_cookie(router).await;
        let resp = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(uri)
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn settings_round_trip_and_validation() {
        let app = router_with_data_admin();

        // GET before set -> default + is_default true.
        let (s, v) = get(&app, "/api/services/checkout/settings").await;
        assert_eq!(s, axum::http::StatusCode::OK);
        assert_eq!(v["is_default"], serde_json::json!(true));
        assert_eq!(v["apdex_threshold_ms"], serde_json::json!(500));

        // PUT 0 -> 400.
        let (s, _) = put_json(
            &app,
            "/api/services/checkout/settings",
            serde_json::json!({"apdex_threshold_ms":0}),
        )
        .await;
        assert_eq!(s, axum::http::StatusCode::BAD_REQUEST);

        // PUT 750 -> reflected.
        let (s, _) = put_json(
            &app,
            "/api/services/checkout/settings",
            serde_json::json!({"apdex_threshold_ms":750}),
        )
        .await;
        assert_eq!(s, axum::http::StatusCode::OK);
        let (_, v) = get(&app, "/api/services/checkout/settings").await;
        assert_eq!(v["apdex_threshold_ms"], serde_json::json!(750));
        assert_eq!(v["is_default"], serde_json::json!(false));

        // DELETE -> back to default.
        let (s, _) = delete(&app, "/api/services/checkout/settings").await;
        assert_eq!(s, axum::http::StatusCode::OK);
        let (_, v) = get(&app, "/api/services/checkout/settings").await;
        assert_eq!(v["is_default"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn settings_get_and_put_404_when_data_admin_disabled() {
        // No `with_data_admin` call -> `state.data` is `None`. GET still 200s with a default;
        // PUT/DELETE 404 (there is no settings store to write to).
        let app = crate::test_router();
        let (s, v) = get(&app, "/api/services/checkout/settings").await;
        assert_eq!(s, axum::http::StatusCode::OK);
        assert_eq!(v["is_default"], serde_json::json!(true));

        let (s, _) = put_json(
            &app,
            "/api/services/checkout/settings",
            serde_json::json!({"apdex_threshold_ms":750}),
        )
        .await;
        assert_eq!(s, axum::http::StatusCode::NOT_FOUND);

        let (s, _) = delete(&app, "/api/services/checkout/settings").await;
        assert_eq!(s, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn timeseries_and_dependencies_shapes() {
        let app = router_with_data_admin();
        let (s, v) = get(
            &app,
            "/api/services/web/timeseries?start=0&end=100&buckets=2",
        )
        .await;
        assert_eq!(s, axum::http::StatusCode::OK);
        assert!(v.is_array());
        assert_eq!(v.as_array().unwrap().len(), 2);

        let b0 = &v.as_array().unwrap()[0];
        for k in ["ts", "rate", "error_rate", "p50", "p90", "p99", "apdex"] {
            assert!(b0.get(k).is_some(), "missing existing key {k}");
        }
        // New: absolute volume + Apdex band counts (already computed on RedBucket).
        for k in [
            "count",
            "error_count",
            "satisfied",
            "tolerating",
            "frustrated",
        ] {
            assert!(b0.get(k).is_some(), "missing new key {k}");
            assert!(b0[k].is_number(), "{k} should be a number");
        }

        let (s, v) = get(&app, "/api/services/web/dependencies?start=0&end=100").await;
        assert_eq!(s, axum::http::StatusCode::OK);
        assert!(v["database"].is_array() && v["external"].is_array());
    }

    #[tokio::test]
    async fn services_routes_require_session() {
        let app = crate::test_router();
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/services/web/settings")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }
}

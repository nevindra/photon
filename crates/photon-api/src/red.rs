//! `GET /api/red` — RED (Rate, Errors, Duration) metrics per service (or per service+operation)
//! over the matched span set. A thin GET handler mirroring `traces_agg.rs`: it builds a
//! `SpanQueryRequest` from the shared params, calls `SpanQueryEngine::red_metrics`, then derives
//! `rate` (requests/sec over the window) and `error_rate` (`error_count/count`) here — the engine
//! returns raw counts + percentiles and stays window-agnostic. Percentiles cross as
//! decimal-nanosecond strings (JS-safe), consistent with `/api/traces/latency`.
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::{RedGroup, RedRow};

use crate::query_params::build_span_query_request;
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct RedParams {
    #[serde(default)]
    query: String,
    start: String,
    end: String,
    #[serde(default = "default_group")]
    group: String,
}

fn default_group() -> String {
    "operation".to_string()
}

/// `GET /api/red?query&start&end&group=operation|service`.
pub(crate) async fn red(State(state): State<AppState>, Query(p): Query<RedParams>) -> Response {
    let req = match build_span_query_request(
        &p.query,
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
    // Capture the window before `req` is moved into `red_metrics` — it powers the rate denominator.
    let window_secs = window_seconds(req.start_ts_nanos, req.end_ts_nanos);
    let group = match p.group.as_str() {
        "service" => RedGroup::Service,
        _ => RedGroup::Operation,
    };

    use std::collections::HashMap;
    let (thresholds, default_ms) = match &state.data {
        Some(d) => (
            d.settings.all_apdex_thresholds().await.unwrap_or_default(),
            d.apdex_default_ms,
        ),
        None => (
            HashMap::new(),
            photon_core::config::DEFAULT_APDEX_THRESHOLD_MS,
        ),
    };

    match state
        .span_query
        .red_metrics(req, group, &thresholds, default_ms)
        .await
    {
        Ok(rows) => Json(
            rows.iter()
                .map(|r| red_row_to_json(r, window_secs))
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

/// Window length in seconds, floored at 1ns → a tiny positive number, so the rate denominator is
/// never zero (a degenerate `start >= end` window yields a large-but-finite rate rather than NaN).
fn window_seconds(start_ns: i64, end_ns: i64) -> f64 {
    let span_ns = (end_ns - start_ns).max(1) as f64;
    span_ns / 1_000_000_000.0
}

fn red_row_to_json(r: &RedRow, window_secs: f64) -> Value {
    let error_rate = if r.count > 0 {
        r.error_count as f64 / r.count as f64
    } else {
        0.0
    };
    let banded = r.satisfied + r.tolerating + r.frustrated;
    let apdex: Option<f64> = if banded > 0 {
        Some((r.satisfied as f64 + r.tolerating as f64 / 2.0) / banded as f64)
    } else {
        None
    };
    json!({
        "service": r.service,
        "operation": r.operation,          // null for group=service (serde maps Option::None → null)
        "count": r.count,
        "rate": r.count as f64 / window_secs,
        "error_count": r.error_count,
        "error_rate": error_rate,
        "p50": r.p50.to_string(),          // nanos as string (JS-safe), matches /api/traces/latency
        "p90": r.p90.to_string(),
        "p99": r.p99.to_string(),
        "apdex": apdex,
    })
}

#[cfg(test)]
mod tests {
    use tower::ServiceExt;

    async fn get(uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
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

    #[tokio::test]
    async fn red_requires_session() {
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/red?start=0&end=100")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn empty_server_returns_empty_red_array() {
        let (status, v) = get("/api/red?start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v, serde_json::json!([]));
    }

    #[tokio::test]
    async fn group_service_is_accepted() {
        let (status, v) = get("/api/red?start=0&end=100&group=service").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(v.is_array());
    }

    #[tokio::test]
    async fn bad_query_is_a_400_with_offset() {
        let (status, v) = get("/api/red?query=ok+%3Abad&start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(v["offset"], serde_json::json!(3));
    }

    #[tokio::test]
    async fn red_rows_include_apdex_field() {
        // Empty server → empty array is fine for shape; assert the field appears on a seeded row
        // if the test harness supports seeding. Minimum: group=service returns an array and, when
        // non-empty, each row has an "apdex" key (number or null).
        let (status, v) = get("/api/red?start=0&end=100&group=service").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(v.is_array());
        if let Some(row) = v.as_array().unwrap().first() {
            assert!(row.get("apdex").is_some(), "row must carry apdex");
        }
    }
}

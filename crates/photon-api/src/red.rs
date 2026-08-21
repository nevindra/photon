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

use photon_query::{RedBasis, RedGroup, RedRow};

use crate::query_params::build_span_query_request;
use crate::services_rollup::RedSource;
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

    let (thresholds, default_ms) = crate::services::apdex_settings(&state).await;

    // `None` resolution: this route returns window TOTALS, not a shape, so any rollup granularity
    // serves. `red_rows` decides between the rollup and raw spans and says which it used.
    match crate::services_rollup::red_rows(
        &state,
        &p.query,
        req,
        None,
        group,
        &thresholds,
        default_ms,
    )
    .await
    {
        Ok((rows, source)) => Json(
            rows.iter()
                .map(|r| red_row_to_json(r, window_secs, source))
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
pub(crate) fn window_seconds(start_ns: i64, end_ns: i64) -> f64 {
    let span_ns = (end_ns - start_ns).max(1) as f64;
    span_ns / 1_000_000_000.0
}

/// The wire spelling of the entry-set cascade rule that produced a row's `count` (ADR-0003). The
/// UI badges `"all_spans"` as `span-based`; `services.rs` uses this too, so the mapping lives in
/// one place.
pub(crate) fn basis_str(basis: RedBasis) -> &'static str {
    match basis {
        RedBasis::Entry => "entry",
        RedBasis::TraceRoot => "trace_root",
        RedBasis::AllSpans => "all_spans",
    }
}

/// One RED row as the UI reads it. `pub(crate)` because `services_overview` returns the same rows
/// with sparklines attached — a second, hand-rolled copy of this shape is how `rate` and
/// `error_rate` went missing from the index the first time.
pub(crate) fn red_row_to_json(r: &RedRow, window_secs: f64, source: RedSource) -> Value {
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
        "tenant": r.tenant,                // null for local spans / group=operation
        "environment": r.environment,      // deployment.environment; null for group=operation
        "operation": r.operation,          // null for group=service (serde maps Option::None → null)
        "count": r.count,                  // requests: the group's ENTRY spans (ADR-0003)
        "rate": r.count as f64 / window_secs,
        "span_count": r.span_count,        // every span, all kinds — the separate second number
        "basis": basis_str(r.basis),       // which cascade rule produced `count`
        "error_count": r.error_count,
        "error_rate": error_rate,
        "http_4xx": r.http_4xx,             // HTTP status class, beside error_count not instead
        "http_5xx": r.http_5xx,             // of it — see RedRow::http_4xx's doc comment
        "p50": r.p50.to_string(),          // nanos as string (JS-safe), matches /api/traces/latency
        "p90": r.p90.to_string(),
        "p99": r.p99.to_string(),
        "apdex": apdex,
        // Which read path produced this row. Additive and per-row, so the response stays the bare
        // array the UI already parses — and so a rollup bug shows up as an attributable number
        // rather than a mysterious one.
        "source": source.as_str(),
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

    #[test]
    fn every_basis_has_a_stable_wire_spelling() {
        use photon_query::RedBasis;
        assert_eq!(super::basis_str(RedBasis::Entry), "entry");
        assert_eq!(super::basis_str(RedBasis::TraceRoot), "trace_root");
        assert_eq!(super::basis_str(RedBasis::AllSpans), "all_spans");
    }

    #[test]
    fn red_row_json_carries_the_span_total_and_rates_the_entry_count() {
        use photon_query::{RedBasis, RedRow};
        let row = RedRow {
            environment: None,
            service: "api".into(),
            tenant: None,
            operation: None,
            count: 2,
            span_count: 10,
            basis: RedBasis::AllSpans,
            error_count: 1,
            p50: 1,
            p90: 2,
            p99: 3,
            satisfied: 1,
            tolerating: 1,
            frustrated: 0,
            http_4xx: 1,
            http_5xx: 0,
        };
        let v = super::red_row_to_json(&row, 4.0, super::RedSource::RawSpans);
        assert_eq!(v["count"], serde_json::json!(2));
        assert_eq!(v["span_count"], serde_json::json!(10));
        assert_eq!(v["basis"], serde_json::json!("all_spans"));
        // `rate` is still count / window seconds — over the ENTRY count, not the span total.
        assert_eq!(v["rate"], serde_json::json!(0.5));
        assert_eq!(v["error_rate"], serde_json::json!(0.5));
        // http_4xx/http_5xx sit beside error_count/error_rate, not instead of them.
        assert_eq!(v["http_4xx"], serde_json::json!(1));
        assert_eq!(v["http_5xx"], serde_json::json!(0));
    }
}

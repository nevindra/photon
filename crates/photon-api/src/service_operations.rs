//! `GET /api/services/:service/operations` and
//! `GET /api/services/:service/operations/:operation/heatmap` — the operations tab.
//!
//! The list route exists rather than the UI calling `/api/red?q=service.name:…&group=operation`
//! for two reasons the shared route cannot give it: a per-row **sparkline** (one bucketed read per
//! row, concurrently, capped), and the rollup read path, so a wide window does not re-scan raw
//! spans. The row shape is `/api/red`'s, built by the same function — one shape, one place.
//!
//! The heatmap route is the operation detail's one genuinely new chart. See
//! `photon_query::span_heatmap` for why the latency ladder is fixed rather than derived.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::RedGroup;

use crate::query_params::{build_span_query_request, clamp_buckets, grammar_term};
use crate::red::{red_row_to_json, window_seconds};
use crate::services_rollup::{red_buckets, red_rows};
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct OperationsParams {
    #[serde(default)]
    q: String,
    start: String,
    end: String,
    #[serde(default = "default_buckets")]
    buckets: usize,
}

fn default_buckets() -> usize {
    30
}

/// How many rows get a sparkline. Same reasoning (and same number) as `services_overview`: each
/// one is its own bucketed read, so this is the difference between a page load and a thundering
/// herd. Rows past the cap render without a sparkline rather than being dropped.
const MAX_SPARKLINES: usize = 40;

/// The caller's query, narrowed to one service, and to one operation when given.
///
/// Terms go through `grammar_term`, which quotes a value containing whitespace or a comma and
/// drops one the grammar cannot express. Operation names (`GET /v1/charges`) contain spaces as a
/// matter of course, so an unquoted term here would parse as a name plus a stray free-text word
/// and silently describe a different set of spans.
fn scoped_query(base: &str, service: &str, operation: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !base.trim().is_empty() {
        parts.push(base.trim().to_string());
    }
    parts.extend(grammar_term("service.name", service));
    if let Some(op) = operation {
        parts.extend(grammar_term("name", op));
    }
    parts.join(" ")
}

/// `GET /api/services/:service/operations?start&end&buckets&q`.
pub(crate) async fn operations(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Query(p): Query<OperationsParams>,
) -> Response {
    let q = scoped_query(&p.q, &service, None);
    let req = match build_span_query_request(
        &q,
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
    let (start_ns, end_ns) = (req.start_ts_nanos, req.end_ts_nanos);
    let buckets = clamp_buckets(p.buckets);
    let (thresholds, default_ms) = crate::services::apdex_settings(&state).await;

    let (rows, source) = match red_rows(
        &state,
        &q,
        req,
        None,
        RedGroup::Operation,
        &thresholds,
        default_ms,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    let sparked = rows.len().min(MAX_SPARKLINES);
    let sparks = futures::future::join_all(rows.iter().take(sparked).map(|r| {
        let state = state.clone();
        let q = scoped_query(&p.q, &service, r.operation.as_deref());
        let (start, end) = (p.start.clone(), p.end.clone());
        async move {
            let Ok(req) = build_span_query_request(
                &q,
                &start,
                &end,
                "recent",
                0,
                0,
                state.span_query.promoted_attributes(),
            ) else {
                return Vec::new();
            };
            red_buckets(&state, &q, req, buckets, default_ms)
                .await
                .map(|(b, _)| b)
                .unwrap_or_default()
        }
    }))
    .await;

    let window_secs = window_seconds(start_ns, end_ns);
    let out: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut v = red_row_to_json(r, window_secs, source);
            let series = sparks.get(i);
            if let Some(obj) = v.as_object_mut() {
                // `[]` past the cap — an empty sparkline, not a flat one. A flat line would claim
                // the operation had no traffic.
                obj.insert(
                    "spark".into(),
                    Value::Array(series.map(|b| points(b, |x| x.count)).unwrap_or_default()),
                );
                obj.insert(
                    "error_spark".into(),
                    Value::Array(
                        series
                            .map(|b| points(b, |x| x.error_count))
                            .unwrap_or_default(),
                    ),
                );
            }
            v
        })
        .collect();

    Json(json!({
        "source": source.as_str(),
        "window": { "start": start_ns.to_string(), "end": end_ns.to_string(), "buckets": buckets },
        "rows": out,
    }))
    .into_response()
}

fn points(
    buckets: &[photon_query::RedBucket],
    f: impl Fn(&photon_query::RedBucket) -> u64,
) -> Vec<Value> {
    buckets
        .iter()
        .map(|b| json!([b.ts / 1_000_000, f(b)]))
        .collect()
}

#[derive(Deserialize)]
pub(crate) struct HeatmapParams {
    #[serde(default)]
    q: String,
    start: String,
    end: String,
    #[serde(default = "default_heat_buckets")]
    buckets: usize,
}

fn default_heat_buckets() -> usize {
    48
}

/// `GET /api/services/:service/operations/:operation/heatmap?start&end&buckets&q`.
///
/// Raw spans only — deliberately. The rollup stores one histogram per 60s bucket per operation,
/// which is the right shape, but reassembling a heatmap from it is a different read path with
/// different failure modes; until that exists, answering from anything but the spans themselves
/// would mean two charts on one page disagreeing about the same operation.
pub(crate) async fn heatmap(
    State(state): State<AppState>,
    Path((service, operation)): Path<(String, String)>,
    Query(p): Query<HeatmapParams>,
) -> Response {
    let q = scoped_query(&p.q, &service, Some(&operation));
    let req = match build_span_query_request(
        &q,
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
    let (start_ns, end_ns) = (req.start_ts_nanos, req.end_ts_nanos);
    let buckets = clamp_buckets(p.buckets);

    match state
        .span_query
        .latency_heatmap(req, start_ns, end_ns, buckets)
        .await
    {
        Ok(h) => Json(json!({
            // Epoch MILLIS, matching every other chart payload the UI reads.
            "columns": h.columns.iter().map(|t| t / 1_000_000).collect::<Vec<_>>(),
            // Nanosecond STRINGS (JS-safe), matching the percentile fields elsewhere.
            "bounds": h.bounds.iter().map(|b| b.to_string()).collect::<Vec<_>>(),
            "cells": h
                .cells
                .iter()
                .map(|c| json!([c.t, c.band, c.count]))
                .collect::<Vec<_>>(),
            "open_spans": h.open_spans,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    async fn get(uri: &str) -> (StatusCode, Value) {
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
    async fn operations_requires_a_session() {
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/services/api/operations?start=0&end=100")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_empty_server_returns_an_empty_operation_list() {
        let (status, v) = get("/api/services/api/operations?start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["rows"], json!([]));
        assert_eq!(v["source"], json!("raw"));
    }

    #[tokio::test]
    async fn a_heatmap_over_nothing_still_has_a_complete_x_axis() {
        // An empty chart with no axis reads as "broken"; an empty chart with 48 columns reads as
        // "nothing happened", which is the true statement.
        let (status, v) =
            get("/api/services/api/operations/GET%20%2Fa/heatmap?start=0&end=100000000&buckets=48")
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["columns"].as_array().unwrap().len(), 48);
        assert_eq!(v["cells"], json!([]));
    }

    #[tokio::test]
    async fn the_heatmap_clamps_a_dos_sized_bucket_count() {
        let (status, v) =
            get("/api/services/api/operations/x/heatmap?start=0&end=100&buckets=2000000000").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            v["columns"].as_array().unwrap().len(),
            crate::query_params::MAX_BUCKETS
        );
    }

    #[test]
    fn an_operation_name_with_a_space_is_quoted_and_resolves_to_that_name() {
        let q = scoped_query("", "api", Some("GET /v1/charges"));
        let parsed = photon_core::query::parse(&q).unwrap();
        assert!(parsed.terms.iter().any(|t| t.kind
            == photon_core::query::TermKind::Match {
                field: "name".into(),
                values: vec!["GET /v1/charges".into()],
            }));
    }

    #[test]
    fn the_callers_own_filter_is_kept_not_replaced() {
        let q = scoped_query("http.status_code:500", "api", None);
        assert!(q.starts_with("http.status_code:500 "));
        assert!(q.contains("service.name:api"));
    }
}

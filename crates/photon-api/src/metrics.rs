//! `/api/metrics/*` handlers — the numbers query engine's HTTP surface. All behind `require_auth`.
//! Timestamps cross as decimal-nanosecond strings (JS-safe). Grammar filter errors → 400
//! {error, offset}; unknown metric metadata → 404.

use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_core::metric_agg::Agg;
use photon_core::metric_schema::metric_type as mtype;
use photon_core::query::parser::parse;
use photon_core::query::{MetricFieldResolver, MetricResolvedQuery};
use photon_query::{LabelsResult, MetricSeriesRequest, SeriesResult};

use crate::query_params::QueryParamError;
use crate::AppState;

const DEFAULT_BUCKETS: usize = 200;
const MAX_BUCKETS: usize = 3000;
const MIN_STEP_NANOS: i64 = 1_000_000_000; // 1s floor

// ---------- shared helpers ----------

fn parse_ts(s: &str, field: &str) -> Result<i64, QueryParamError> {
    s.parse::<i64>().map_err(|_| QueryParamError {
        message: format!("`{field}` must be a decimal-nanosecond timestamp string"),
        offset: None,
    })
}

/// Compile a metrics filter string (label matchers) to a resolved query. Empty → None. Parse
/// errors carry a byte offset for the UI underline.
fn resolve_metric_filter(
    filter: &str,
    promoted: &[String],
) -> Result<Option<MetricResolvedQuery>, QueryParamError> {
    if filter.trim().is_empty() {
        return Ok(None);
    }
    let ast = parse(filter).map_err(|e| QueryParamError {
        message: e.message,
        offset: Some(e.offset),
    })?;
    let rq = MetricFieldResolver::new(promoted)
        .resolve(&ast)
        .map_err(|e| QueryParamError {
            message: e.message,
            offset: None,
        })?;
    Ok(Some(rq))
}

fn buckets_for(start: i64, end: i64, step: Option<i64>) -> usize {
    let span = (end - start).max(1);
    match step {
        Some(s) => {
            let s = s.max(MIN_STEP_NANOS);
            (((span + s - 1) / s).clamp(1, MAX_BUCKETS as i64)) as usize
        }
        None => DEFAULT_BUCKETS,
    }
}

fn type_name(t: i32) -> &'static str {
    match t {
        mtype::GAUGE => "gauge",
        mtype::SUM => "sum",
        mtype::HISTOGRAM => "histogram",
        mtype::EXP_HISTOGRAM => "exp_histogram",
        mtype::SUMMARY => "summary",
        _ => "unknown",
    }
}

fn parse_type_name(s: &str) -> Option<i32> {
    Some(match s {
        "gauge" => mtype::GAUGE,
        "sum" => mtype::SUM,
        "histogram" => mtype::HISTOGRAM,
        "exp_histogram" => mtype::EXP_HISTOGRAM,
        "summary" => mtype::SUMMARY,
        other => return other.parse::<i32>().ok(),
    })
}

fn temporality_name(t: Option<i32>) -> Option<&'static str> {
    match t {
        Some(1) => Some("delta"),
        Some(2) => Some("cumulative"),
        _ => None,
    }
}

fn series_json(s: &SeriesResult) -> Value {
    let points: Vec<Value> = s
        .points
        .iter()
        .map(|p| json!({ "t": p.t.to_string(), "v": p.v }))
        .collect();
    json!({ "labels": s.labels, "points": points, "exemplars": [] })
}

// ---------- POST /api/metrics/query ----------

#[derive(Deserialize)]
pub(crate) struct MetricsQueryRequest {
    queries: Vec<QuerySpec>,
    start: String,
    end: String,
    #[serde(default)]
    step: Option<String>,
}

#[derive(Deserialize)]
struct QuerySpec {
    id: String,
    metric: String,
    #[serde(default)]
    agg: Option<String>,
    #[serde(default)]
    group_by: Vec<String>,
    #[serde(default)]
    filter: String,
}

pub(crate) async fn query(
    State(state): State<AppState>,
    Json(req): Json<MetricsQueryRequest>,
) -> Response {
    let started = Instant::now();
    let start = match parse_ts(&req.start, "start") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let end = match parse_ts(&req.end, "end") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let step = match req.step.as_deref() {
        Some(s) => match parse_ts(s, "step") {
            Ok(v) => Some(v),
            Err(e) => return e.into_response(),
        },
        None => None,
    };
    let buckets = buckets_for(start, end, step);
    let promoted = state.metrics_query.promoted_attributes().to_vec();

    let mut results = Vec::with_capacity(req.queries.len());
    let mut step_out: i64 = ((end - start).max(1) / buckets as i64).max(1);
    let mut any_capped = false;

    for q in &req.queries {
        let agg = match q.agg.as_deref() {
            Some(a) => match Agg::parse(a) {
                Some(a) => Some(a),
                None => {
                    return QueryParamError {
                        message: format!("unknown aggregation `{a}`"),
                        offset: None,
                    }
                    .into_response()
                }
            },
            None => None,
        };
        let filter = match resolve_metric_filter(&q.filter, &promoted) {
            Ok(f) => f,
            Err(e) => return e.into_response(),
        };
        let sreq = MetricSeriesRequest {
            metric: q.metric.clone(),
            agg,
            group_by: q.group_by.clone(),
            filter,
            start_ts_nanos: start,
            end_ts_nanos: end,
            buckets,
        };
        match state.metrics_query.query_series(sreq).await {
            Ok(r) => {
                step_out = r.step_nanos;
                any_capped |= r.capped;
                let series: Vec<Value> = r.series.iter().map(series_json).collect();
                results.push(json!({
                    "id": q.id,
                    "series": series,
                    "default_agg": r.default_agg.as_str(),
                }));
            }
            Err(e) => {
                // Engine query error (e.g. unsupported agg, bad group-by) → 400 with the message.
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        }
    }

    Json(json!({
        "results": results,
        "step": step_out.to_string(),
        "capped": any_capped,
        "elapsed_ms": started.elapsed().as_millis() as u64,
    }))
    .into_response()
}

// ---------- GET /api/metrics/catalog ----------

#[derive(Deserialize)]
pub(crate) struct CatalogParams {
    #[serde(default)]
    search: Option<String>,
    #[serde(rename = "type", default)]
    type_filter: Option<String>,
    start: String,
    end: String,
}

pub(crate) async fn catalog(
    State(state): State<AppState>,
    Query(p): Query<CatalogParams>,
) -> Response {
    let start = match parse_ts(&p.start, "start") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let end = match parse_ts(&p.end, "end") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let type_filter = p.type_filter.as_deref().and_then(parse_type_name);
    match state
        .metrics_query
        .catalog(start, end, p.search.as_deref(), type_filter)
        .await
    {
        Ok(entries) => {
            let out: Vec<Value> = entries
                .iter()
                .map(|e| {
                    json!({
                        "name": e.name,
                        "type": type_name(e.metric_type),
                        "unit": e.unit,
                        "temporality": temporality_name(e.temporality),
                        "is_monotonic": e.is_monotonic,
                        "series_count": e.series_count,
                        "last_seen": e.last_seen_nanos.to_string(),
                    })
                })
                .collect();
            Json(out).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ---------- GET /api/metrics/metadata/:name ----------

#[derive(Deserialize)]
pub(crate) struct MetadataParams {
    start: String,
    end: String,
}

pub(crate) async fn metadata(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(p): Query<MetadataParams>,
) -> Response {
    let start = match parse_ts(&p.start, "start") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let end = match parse_ts(&p.end, "end") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match state.metrics_query.metadata(&name, start, end).await {
        Ok(Some(m)) => Json(json!({
            "name": m.name,
            "type": type_name(m.metric_type),
            "temporality": temporality_name(m.temporality),
            "is_monotonic": m.is_monotonic,
            "unit": m.unit,
            "series_count": m.series_count,
            "last_seen": m.last_seen_nanos.to_string(),
            "attribute_keys": m.attribute_keys,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("unknown metric `{name}`") })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ---------- GET /api/metrics/labels ----------

#[derive(Deserialize)]
pub(crate) struct LabelsParams {
    metric: String,
    #[serde(default)]
    key: Option<String>,
    start: String,
    end: String,
}

pub(crate) async fn labels(
    State(state): State<AppState>,
    Query(p): Query<LabelsParams>,
) -> Response {
    let start = match parse_ts(&p.start, "start") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let end = match parse_ts(&p.end, "end") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match state
        .metrics_query
        .labels(&p.metric, p.key.as_deref(), start, end)
        .await
    {
        Ok(LabelsResult::Keys(keys)) => Json(json!({ "keys": keys })).into_response(),
        Ok(LabelsResult::Values { values, capped }) => {
            Json(json!({ "values": values, "capped": capped })).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use tower::ServiceExt;

    /// Authed GET returning `(status, json)`.
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

    /// Authed POST of a JSON body returning `(status, json)`.
    async fn post(
        uri: &str,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(axum::http::header::COOKIE, cookie)
                    .header("content-type", "application/json")
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

    #[tokio::test]
    async fn empty_server_returns_empty_catalog() {
        let (status, v) = get("/api/metrics/catalog?start=0&end=1").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v, serde_json::json!([]));
    }

    #[tokio::test]
    async fn unknown_metadata_is_not_found() {
        let (status, _v) = get("/api/metrics/metadata/unknown?start=0&end=1").await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn query_over_empty_server_returns_empty_series() {
        let (status, v) = post(
            "/api/metrics/query",
            serde_json::json!({
                "queries": [{ "id": "a", "metric": "m" }],
                "start": "0",
                "end": "1000",
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            v["results"],
            serde_json::json!([{ "id": "a", "series": [], "default_agg": "avg" }])
        );
    }

    #[tokio::test]
    async fn malformed_filter_is_bad_request_with_offset() {
        let (status, v) = post(
            "/api/metrics/query",
            serde_json::json!({
                "queries": [{ "id": "a", "metric": "m", "filter": "status>=" }],
                "start": "0",
                "end": "1000",
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(v.get("error").is_some(), "error field present: {v}");
        assert!(v.get("offset").is_some(), "offset field present: {v}");
    }

    #[tokio::test]
    async fn missing_cookie_is_unauthorized() {
        // GET catalog with no session cookie.
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/metrics/catalog?start=0&end=1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }
}

//! `GET /api/traces/{fields,facet,histogram,latency}` — span aggregation endpoints for the traces
//! explorer. Thin GET handlers mirroring `fields.rs`/`facet.rs`/`histogram.rs` (the logs
//! aggregation handlers) but reading `state.span_query` (the spans dataset) instead.
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::{FieldInfo, FieldKind, LatencyHistogram, SpanHistogramBucket};

use crate::query_params::{build_span_query_request, clamp_buckets, clamp_limit, parse_window};
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct TracesFieldsParams {
    start: String,
    end: String,
}

/// `GET /api/traces/fields?start&end` — the span field catalog for a window (metadata only, reads
/// the spans manifest, never the Parquet data).
pub(crate) async fn traces_fields(
    State(state): State<AppState>,
    Query(p): Query<TracesFieldsParams>,
) -> Response {
    let (start, end) = match parse_window(&p.start, &p.end) {
        Ok(w) => w,
        Err(e) => return e.into_response(),
    };
    match state.span_query.fields(start, end) {
        Ok(fields) => Json(fields.iter().map(field_to_json).collect::<Vec<_>>()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn field_to_json(f: &FieldInfo) -> Value {
    json!({ "name": f.name, "kind": kind_str(f.kind) })
}

fn kind_str(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Fixed => "fixed",
        FieldKind::Promoted => "promoted",
        FieldKind::Attribute => "attribute",
    }
}

#[derive(Deserialize)]
pub(crate) struct TracesFacetParams {
    field: String,
    #[serde(default)]
    query: String,
    start: String,
    end: String,
    #[serde(default = "default_facet_limit")]
    limit: usize,
}

fn default_facet_limit() -> usize {
    50
}

/// `GET /api/traces/facet?field&query&start&end&limit` — top field values + counts over the span
/// match set.
pub(crate) async fn traces_facet(
    State(state): State<AppState>,
    Query(p): Query<TracesFacetParams>,
) -> Response {
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
    match state
        .span_query
        .facet(&p.field, req, clamp_limit(p.limit))
        .await
    {
        Ok(r) => Json(json!({
            "values": r.values.iter()
                .map(|v| json!({ "value": v.value, "count": v.count }))
                .collect::<Vec<_>>(),
            "capped": r.capped,
        }))
        .into_response(),
        // A non-facetable field (e.g. `status`, `kind`, `duration`) surfaces here as a query
        // error -> 400, mirroring `facet::facet`.
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Shared params for the two span-histogram-shaped endpoints (`histogram`/`latency`): a grammar
/// `query`, the `[start, end]` window, and a bucket count.
#[derive(Deserialize)]
pub(crate) struct TracesHistogramParams {
    #[serde(default)]
    query: String,
    start: String,
    end: String,
    #[serde(default = "default_buckets")]
    buckets: usize,
}

fn default_buckets() -> usize {
    48
}

/// `GET /api/traces/histogram?query&start&end&buckets` — status-stacked span volume over the
/// full match set.
pub(crate) async fn traces_histogram(
    State(state): State<AppState>,
    Query(p): Query<TracesHistogramParams>,
) -> Response {
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
    match state
        .span_query
        .histogram(req, clamp_buckets(p.buckets))
        .await
    {
        Ok(buckets) => Json(
            buckets
                .iter()
                .map(histogram_bucket_to_json)
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

fn histogram_bucket_to_json(b: &SpanHistogramBucket) -> Value {
    json!({
        "t": b.t.to_string(), // epoch nanos as string (dodges JS 2^53), matches logs' histogram
        "ok": b.ok,
        "error": b.error,
        "unset": b.unset,
        "total": b.total,
    })
}

/// `GET /api/traces/latency?query&start&end&buckets` — duration-distribution histogram + p50/p90/
/// p99 percentiles over the full match set.
pub(crate) async fn traces_latency(
    State(state): State<AppState>,
    Query(p): Query<TracesHistogramParams>,
) -> Response {
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
    match state
        .span_query
        .latency(req, clamp_buckets(p.buckets))
        .await
    {
        Ok(hist) => Json(latency_to_json(&hist)).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

fn latency_to_json(h: &LatencyHistogram) -> Value {
    json!({
        "buckets": h.buckets.iter().map(|b| json!({
            "bucket_ns": b.bucket_ns.to_string(),
            "count": b.count,
        })).collect::<Vec<_>>(),
        "p50": h.p50.to_string(),
        "p90": h.p90.to_string(),
        "p99": h.p99.to_string(),
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
    async fn traces_fields_requires_session() {
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/traces/fields?start=0&end=100")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    // CRITICAL ROUTE-ORDERING CHECK: `/traces/fields` must hit `traces_fields`, not
    // `get_trace` with `trace_id = "fields"`. `get_trace` on an unknown trace id 404s and returns
    // `{ error: "trace not found" }`; `traces_fields` 200s with a JSON array of `{name,kind}`. If
    // this ever regresses to a 404/object response, that's a real route conflict, not a flaky test.
    #[tokio::test]
    async fn traces_fields_route_does_not_collide_with_trace_id_route() {
        let (status, v) = get("/api/traces/fields?start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(v.is_array(), "expected a fields array, got: {v}");
    }

    #[tokio::test]
    async fn empty_server_returns_fixed_and_promoted_fields() {
        let (status, v) = get("/api/traces/fields?start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let arr = v.as_array().unwrap();
        assert!(arr
            .iter()
            .any(|f| f["name"] == "span_id" && f["kind"] == "fixed"));
        assert!(arr
            .iter()
            .any(|f| f["name"] == "service.name" && f["kind"] == "promoted"));
    }

    #[tokio::test]
    async fn empty_server_returns_empty_facet() {
        let (status, v) = get("/api/traces/facet?field=service.name&start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["values"], serde_json::json!([]));
        assert_eq!(v["capped"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn faceting_on_status_is_a_bad_request() {
        let (status, _v) = get("/api/traces/facet?field=status&start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn empty_server_returns_zeroed_histogram_buckets() {
        let (status, v) = get("/api/traces/histogram?start=0&end=100&buckets=4").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0]["total"], serde_json::json!(0));
        assert!(arr[0]["t"].is_string());
    }

    #[tokio::test]
    async fn empty_server_returns_zeroed_latency_histogram() {
        let (status, v) = get("/api/traces/latency?start=0&end=100&buckets=4").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        // No survivors -> `SpanQueryEngine::latency` short-circuits to an empty bucket list
        // (unlike the time-bucketed `histogram`, which always returns `buckets` zeroed entries;
        // latency buckets are sized by max-observed-duration, which is undefined with no data).
        assert_eq!(v["buckets"], serde_json::json!([]));
        assert_eq!(v["p50"], serde_json::json!("0"));
        assert_eq!(v["p90"], serde_json::json!("0"));
        assert_eq!(v["p99"], serde_json::json!("0"));
    }

    #[tokio::test]
    async fn traces_histogram_clamps_a_dos_sized_bucket_count() {
        let (status, v) = get("/api/traces/histogram?start=0&end=100&buckets=2000000000").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            v.as_array().unwrap().len(),
            crate::query_params::MAX_BUCKETS
        );
    }

    #[tokio::test]
    async fn traces_latency_clamps_a_dos_sized_bucket_count() {
        // The doc's literal opening-line repro: `GET /api/traces/latency?buckets=2000000000`
        // must return 200 promptly instead of allocating ~16 GB.
        let (status, v) = get("/api/traces/latency?start=0&end=100&buckets=2000000000").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        // No survivors on an empty store -> `latency` short-circuits to empty buckets
        // regardless of the (clamped) bucket count, same as the existing zeroed-histogram test.
        assert_eq!(v["buckets"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn traces_facet_clamps_a_dos_sized_limit() {
        let (status, v) =
            get("/api/traces/facet?field=service.name&start=0&end=100&limit=999999999").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["values"], serde_json::json!([]));
        assert_eq!(v["capped"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn bad_query_on_traces_facet_is_a_400_with_offset() {
        let (status, v) =
            get("/api/traces/facet?field=service.name&query=ok+%3Abad&start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(v["offset"], serde_json::json!(3));
    }

    #[tokio::test]
    async fn bad_query_on_traces_histogram_is_a_400_with_offset() {
        let (status, v) = get("/api/traces/histogram?query=ok+%3Abad&start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(v["offset"], serde_json::json!(3));
    }
}

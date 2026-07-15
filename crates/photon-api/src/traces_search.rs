//! Trace/span search: `POST /api/traces/search` (rolled-up trace summaries) and
//! `POST /api/spans/search` (raw span rows), the paged siblings of `POST /api/search` for the
//! spans dataset. Both take a JSON body (timestamps as decimal-nanosecond strings, mirroring
//! `search::SearchRequest`) and share one `SpanSearchRequest` shape; `cursor`/`next_cursor` are
//! simple decimal-offset strings (v1 offset pagination).

use std::time::Instant;

use arrow::record_batch::RecordBatch;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::{SpanQueryRequest, TraceSearchResult, TraceSummary};

use crate::query_params::{build_span_query_request, QueryParamError};
use crate::traces::span_row_to_json;
use crate::AppState;

/// The shared request body for `/api/traces/search` and `/api/spans/search`. `limit` is optional
/// so each handler can apply its own default (100 for traces, 200 for spans, per the plan);
/// `cursor` is the decimal string offset of the previous page's `next_cursor` (absent → page 0).
#[derive(Debug, Deserialize)]
pub(crate) struct SpanSearchRequest {
    start: String,
    end: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    sort: String,
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    /// Attribute keys to project as each trace row's `root_attributes` (trace search only;
    /// ignored by `/api/spans/search`). Absent/empty ⇒ no `root_attributes` in the response.
    #[serde(default)]
    columns: Vec<String>,
}

/// Parse the optional decimal `cursor` string into a row offset; absent → `0`.
fn parse_cursor(cursor: &Option<String>) -> Result<usize, QueryParamError> {
    match cursor {
        None => Ok(0),
        Some(s) if s.is_empty() => Ok(0),
        Some(s) => s.parse().map_err(|_| QueryParamError {
            message: format!("invalid cursor: {s}"),
            offset: None,
        }),
    }
}

fn trace_summary_to_json(t: &TraceSummary) -> Value {
    let mut v = json!({
        "trace_id": t.trace_id,
        "root_service": t.root_service,
        "root_name": t.root_name,
        "start_ts": t.start_ts_nanos.to_string(),
        "duration_ns": t
            .duration_nanos
            .map(|d| Value::String(d.to_string()))
            .unwrap_or(Value::Null),
        "span_count": t.span_count,
        "error_count": t.error_count,
        "services": t.services,
    });
    // Only emit `root_attributes` when keys were requested and present — default payloads stay clean.
    if !t.root_attributes.is_empty() {
        if let Value::Object(map) = &mut v {
            map.insert(
                "root_attributes".to_string(),
                serde_json::to_value(&t.root_attributes).unwrap(),
            );
        }
    }
    v
}

fn span_batches_to_rows(batches: &[RecordBatch]) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut id: i64 = 0;
    for batch in batches {
        for row in 0..batch.num_rows() {
            rows.push(span_row_to_json(batch, row, id));
            id += 1;
        }
    }
    rows
}

/// `next_cursor` is present iff the returned page was full (didn't fall short of `limit`) AND
/// there are more matches beyond it.
fn next_cursor(page_len: usize, limit: usize, offset: usize, matched_count: u64) -> Option<String> {
    if page_len == limit && (offset + limit) < matched_count as usize {
        Some((offset + limit).to_string())
    } else {
        None
    }
}

/// `POST /api/traces/search` — rolled-up trace summaries whose spans match the filter request.
/// A fresh system (or a hard engine error) yields an empty `traces` array and zero `matched_count`,
/// not a 500 — mirroring `search::search`'s empty-server behavior.
pub(crate) async fn traces_search(
    State(state): State<AppState>,
    Json(req): Json<SpanSearchRequest>,
) -> Response {
    let offset = match parse_cursor(&req.cursor) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    let mut query = match build_span_query_request(
        &req.query,
        &req.start,
        &req.end,
        &req.sort,
        req.limit.unwrap_or(100),
        offset,
        state.span_query.promoted_attributes(),
    ) {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    // Trace search alone projects root-span attributes; empty ⇒ the engine decodes nothing.
    query.projected_attributes = req.columns.clone();
    let limit = query.limit;
    let offset = query.offset;

    let started = Instant::now();
    let result = match state.span_query.search_traces(query).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("photon-api: warning: traces search failed, returning empty results: {e}");
            TraceSearchResult {
                traces: Vec::new(),
                matched_count: 0,
            }
        }
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let cursor = next_cursor(result.traces.len(), limit, offset, result.matched_count);

    Json(json!({
        "traces": result.traces.iter().map(trace_summary_to_json).collect::<Vec<_>>(),
        "matched_count": result.matched_count,
        "elapsed_ms": elapsed_ms,
        "next_cursor": cursor,
    }))
    .into_response()
}

/// `POST /api/spans/search` — raw span rows matching the filter request, in the same JSON shape
/// as `GET /api/traces/:trace_id`'s spans. A fresh system (or a hard engine error) yields an empty
/// `rows` array and a zero `matched_count` — both come from the same `search_spans_with_count`
/// call, so they can't disagree (mirrors `search::search`).
pub(crate) async fn spans_search(
    State(state): State<AppState>,
    Json(req): Json<SpanSearchRequest>,
) -> Response {
    let offset = match parse_cursor(&req.cursor) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    let query: SpanQueryRequest = match build_span_query_request(
        &req.query,
        &req.start,
        &req.end,
        &req.sort,
        req.limit.unwrap_or(200),
        offset,
        state.span_query.promoted_attributes(),
    ) {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    let limit = query.limit;
    let offset = query.offset;

    let started = Instant::now();
    // One prune/open for both the (row-limited) page and the true total match count, instead of
    // `search_spans` + `count_matching_spans` independently re-pruning the manifest/skip-indexes
    // and re-opening every surviving Parquet file.
    let (rows, matched_count) = match state.span_query.search_spans_with_count(query).await {
        Ok((batches, matched_count)) => (span_batches_to_rows(&batches), matched_count),
        Err(e) => {
            eprintln!("photon-api: warning: spans search failed, returning empty results: {e}");
            (Vec::new(), 0)
        }
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let cursor = next_cursor(rows.len(), limit, offset, matched_count);

    Json(json!({
        "rows": rows,
        "matched_count": matched_count,
        "elapsed_ms": elapsed_ms,
        "next_cursor": cursor,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use tower::ServiceExt;

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

    fn window_body(query: &str) -> serde_json::Value {
        serde_json::json!({
            "start": "0",
            "end": "9223372036854775807",
            "query": query,
            "sort": "recent",
            "limit": 100
        })
    }

    #[tokio::test]
    async fn traces_search_requires_session() {
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/traces/search")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(window_body("").to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn empty_server_returns_empty_traces_envelope() {
        let (status, v) = post("/api/traces/search", window_body("")).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["traces"], serde_json::json!([]));
        assert_eq!(v["matched_count"], serde_json::json!(0));
        assert!(v["elapsed_ms"].is_number());
        assert!(v["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn empty_server_returns_empty_spans_envelope() {
        let (status, v) = post("/api/spans/search", window_body("")).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["rows"], serde_json::json!([]));
        assert_eq!(v["matched_count"], serde_json::json!(0));
        assert!(v["elapsed_ms"].is_number());
        assert!(v["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn bad_query_is_a_400_with_offset() {
        let (status, v) = post("/api/traces/search", window_body("ok :bad")).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(v["offset"], serde_json::json!(3));
    }

    #[tokio::test]
    async fn bad_query_on_spans_search_is_a_400_with_offset() {
        let (status, v) = post("/api/spans/search", window_body("ok :bad")).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(v["offset"], serde_json::json!(3));
    }

    #[tokio::test]
    async fn bad_window_on_traces_search_is_a_400() {
        let mut body = window_body("");
        body["start"] = serde_json::json!("not-a-number");
        let (status, _v) = post("/api/traces/search", body).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn traces_search_accepts_columns_field() {
        // The `columns` field must deserialize without error. The empty test server yields no
        // traces, so this guards the request wiring, not the (data-dependent) projection itself.
        let mut body = window_body("");
        body["columns"] = serde_json::json!(["http.route"]);
        let (status, v) = post("/api/traces/search", body).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["traces"], serde_json::json!([]));
    }

    #[test]
    fn traces_search_summary_json_emits_root_attributes_only_when_present() {
        use super::trace_summary_to_json;
        use std::collections::BTreeMap;

        let base = photon_query::TraceSummary {
            trace_id: "T".to_string(),
            root_service: Some("web".to_string()),
            root_name: Some("checkout".to_string()),
            start_ts_nanos: 1000,
            duration_nanos: Some(100),
            span_count: 2,
            error_count: 0,
            services: vec!["web".to_string()],
            root_attributes: BTreeMap::new(),
        };

        // No projected attributes → the key is omitted entirely.
        let v = trace_summary_to_json(&base);
        assert!(!v.as_object().unwrap().contains_key("root_attributes"));

        // Projected attributes → present and exact.
        let with_attrs = photon_query::TraceSummary {
            root_attributes: BTreeMap::from([("http.route".to_string(), "/checkout".to_string())]),
            ..base
        };
        let v = trace_summary_to_json(&with_attrs);
        assert_eq!(
            v["root_attributes"]["http.route"],
            serde_json::json!("/checkout")
        );
    }

    #[test]
    fn next_cursor_only_when_page_full_and_more_remain() {
        use super::next_cursor;
        // page short of limit -> no cursor even if matched_count is larger (shouldn't happen, but
        // a short page always means "no more").
        assert_eq!(next_cursor(3, 10, 0, 100), None);
        // full page, more remain -> cursor at offset+limit.
        assert_eq!(next_cursor(10, 10, 0, 100), Some("10".to_string()));
        // full page, exactly exhausts matches -> no cursor.
        assert_eq!(next_cursor(10, 10, 0, 10), None);
        // full page mid-way through paging -> cursor continues from the true offset.
        assert_eq!(next_cursor(10, 10, 20, 100), Some("30".to_string()));
    }
}

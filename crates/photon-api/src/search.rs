//! Log search: the `/api/services` and `/api/search` handlers, the SQL builder that turns the
//! UI's filter request into one `logs` query, and the `RecordBatch -> JSON` row conversion the
//! frontend's `hydrate` expects.

use std::time::Instant;

use arrow::array::{Array, Int32Array, MapArray, StringArray, TimestampNanosecondArray};
use arrow::record_batch::RecordBatch;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use photon_core::query::ResolvedQuery;
use photon_core::schema;
use photon_core::PhotonError;
use photon_query::QueryRequest;

use crate::query_params::resolve_query;
use crate::AppState;

/// The search request sent by the UI. Timestamps arrive as decimal-nanosecond strings (they
/// exceed JS's safe integer range), everything else is plain.
#[derive(Debug, Deserialize)]
pub(crate) struct SearchRequest {
    start_ts_nanos: String,
    end_ts_nanos: String,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    severities: Vec<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    query: String,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_limit() -> u64 {
    500
}

/// `GET /api/services` — the sorted set of distinct `service.name` values.
///
/// Delegates to [`QueryEngine::distinct_services`], which short-circuits an empty store
/// (no ingested data yet, or everything purged by retention) to an empty list with no SQL to
/// plan — so it never trips the "no `logs` table" schema error. The `Err` arm stays as a
/// defensive fallback for a genuine query failure.
pub(crate) async fn services(State(state): State<AppState>) -> Response {
    match state.query.distinct_services().await {
        Ok(list) => Json(list).into_response(),
        Err(e) => {
            eprintln!("photon-api: warning: services query failed, returning empty list: {e}");
            Json(Vec::<String>::new()).into_response()
        }
    }
}

/// The `POST /api/search` response envelope: the (row-limited) result rows, the true total match
/// count over the full pruned set (independent of the row limit), and the server-side elapsed
/// time in milliseconds.
#[derive(Serialize)]
struct SearchResponse {
    rows: Vec<Value>,
    matched_count: u64,
    elapsed_ms: u64,
}

/// `POST /api/search` — translate the filter request into a pruned `QueryRequest`, run it
/// through the query engine's manifest + skip-index pruning path, and return the rows wrapped in
/// a `{ rows, matched_count, elapsed_ms }` envelope. A fresh system with no Parquet yields an
/// empty `rows` array and a zero `matched_count` (the engine sees an empty manifest), not a 500.
pub(crate) async fn search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Response {
    let resolved = match resolve_query(&req.query, state.query.promoted_attributes()) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let query = match build_query(&req, resolved) {
        Ok(q) => q,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let started = Instant::now();
    // One prune/open for both the (row-limited) page and the true total match count, instead of
    // `search` + `count_matching` independently re-pruning the manifest/skip-indexes and
    // re-opening every surviving Parquet file.
    let (rows, matched_count) = match state.query.search_with_count(query).await {
        Ok((batches, matched_count)) => (batches_to_rows(&batches), matched_count),
        Err(e) => {
            eprintln!("photon-api: warning: search query failed, returning empty results: {e}");
            (Vec::new(), 0)
        }
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;

    Json(SearchResponse {
        rows,
        matched_count,
        elapsed_ms,
    })
    .into_response()
}

/// Map a UI severity key to its inclusive `severity_number` range, or `None` if unknown.
fn severity_range(key: &str) -> Option<(i32, i32)> {
    match key {
        "debug" => Some((1, 8)),
        "info" => Some((9, 12)),
        "warn" => Some((13, 16)),
        "error" => Some((17, 20)),
        "fatal" => Some((21, 24)),
        _ => None,
    }
}

/// Map a `severity_number` to the UI severity key. `0`/null (and anything <= 0) defaults to
/// `"info"`; otherwise the OTEL-style buckets apply.
fn severity_key(n: i32) -> &'static str {
    if n <= 0 {
        return "info";
    }
    match n {
        1..=8 => "debug",
        9..=12 => "info",
        13..=16 => "warn",
        17..=20 => "error",
        _ => "fatal",
    }
}

/// Translate the UI's filter request into a `QueryRequest` for the pruned query path. A bad
/// timestamp is the only hard error (surfaced as a 400); unknown severity keys are silently
/// dropped (they contribute no range). No SQL string is built — service/text/severity values
/// travel to the engine as bound parameters, so there is no injection surface.
fn build_query(
    req: &SearchRequest,
    query: Option<ResolvedQuery>,
) -> Result<QueryRequest, PhotonError> {
    let start_ts_nanos: i64 = req.start_ts_nanos.parse().map_err(|_| {
        PhotonError::Query(format!("invalid start_ts_nanos: {}", req.start_ts_nanos))
    })?;
    let end_ts_nanos: i64 = req
        .end_ts_nanos
        .parse()
        .map_err(|_| PhotonError::Query(format!("invalid end_ts_nanos: {}", req.end_ts_nanos)))?;

    let severities = req
        .severities
        .iter()
        .filter_map(|s| severity_range(s))
        .collect();
    let text = if req.text.is_empty() {
        None
    } else {
        Some(req.text.clone())
    };

    Ok(QueryRequest {
        start_ts_nanos,
        end_ts_nanos,
        services: req.services.clone(),
        severities,
        text,
        query,
        limit: req.limit as usize,
    })
}

/// Convert query result batches into the JSON row shape the UI's `hydrate` consumes. `id` is a
/// running index across all rows.
pub(crate) fn batches_to_rows(batches: &[RecordBatch]) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut id: i64 = 0;
    for batch in batches {
        for row in 0..batch.num_rows() {
            rows.push(row_to_json(batch, row, id));
            id += 1;
        }
    }
    rows
}

pub(crate) fn downcast<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Option<&'a T> {
    batch.column_by_name(name)?.as_any().downcast_ref::<T>()
}

/// A nullable Utf8 column value at `row`, as a JSON string or `null`.
pub(crate) fn string_or_null(batch: &RecordBatch, name: &str, row: usize) -> Value {
    match downcast::<StringArray>(batch, name) {
        Some(col) if !col.is_null(row) => Value::String(col.value(row).to_string()),
        _ => Value::Null,
    }
}

pub(crate) fn row_to_json(batch: &RecordBatch, row: usize, id: i64) -> Value {
    let timestamp = downcast::<TimestampNanosecondArray>(batch, schema::TIMESTAMP)
        .map(|col| if col.is_null(row) { 0 } else { col.value(row) })
        .unwrap_or(0);

    let severity_number = downcast::<Int32Array>(batch, schema::SEVERITY_NUMBER)
        .map(|col| if col.is_null(row) { 0 } else { col.value(row) })
        .unwrap_or(0);

    let service = match downcast::<StringArray>(batch, "service.name") {
        Some(col) if !col.is_null(row) => col.value(row).to_string(),
        _ => String::new(),
    };

    let mut attributes = serde_json::Map::new();

    // 1. The long-tail attributes Map column for this row.
    if let Some(map) = downcast::<MapArray>(batch, schema::ATTRIBUTES) {
        if !map.is_null(row) {
            let offsets = map.value_offsets();
            let entries = map.entries();
            if let (Some(keys), Some(values)) = (
                entries.column(0).as_any().downcast_ref::<StringArray>(),
                entries.column(1).as_any().downcast_ref::<StringArray>(),
            ) {
                let start = offsets[row] as usize;
                let end = offsets[row + 1] as usize;
                for i in start..end {
                    let value = if values.is_null(i) {
                        Value::Null
                    } else {
                        Value::String(values.value(i).to_string())
                    };
                    attributes.insert(keys.value(i).to_string(), value);
                }
            }
        }
    }

    // 2. Promoted columns other than `service.name` — any column that is neither a fixed
    //    column nor the attributes map is, by construction, a promoted attribute.
    let batch_schema = batch.schema();
    for field in batch_schema.fields() {
        let name = field.name();
        if schema::FIXED_COLUMNS.contains(&name.as_str()) || name == "service.name" {
            continue;
        }
        if let Some(col) = downcast::<StringArray>(batch, name) {
            if !col.is_null(row) {
                attributes.insert(name.clone(), Value::String(col.value(row).to_string()));
            }
        }
    }

    json!({
        "id": id,
        "timestamp": timestamp.to_string(),
        "severity": severity_key(severity_number),
        "service": service,
        "body": string_or_null(batch, schema::BODY, row),
        "trace_id": string_or_null(batch, schema::TRACE_ID, row),
        "span_id": string_or_null(batch, schema::SPAN_ID, row),
        "attributes": Value::Object(attributes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;

    fn request(services: &[&str], severities: &[&str], text: &str) -> SearchRequest {
        SearchRequest {
            start_ts_nanos: "100".to_string(),
            end_ts_nanos: "200".to_string(),
            services: services.iter().map(|s| s.to_string()).collect(),
            severities: severities.iter().map(|s| s.to_string()).collect(),
            text: text.to_string(),
            query: String::new(),
            limit: 500,
        }
    }

    #[test]
    fn resolves_a_valid_query() {
        let promoted = vec!["service.name".to_string(), "status_code".to_string()];
        let rq = resolve_query("service:api status_code>=500", &promoted).unwrap();
        assert_eq!(rq.map(|q| q.terms.len()), Some(2));
    }

    #[test]
    fn empty_query_resolves_to_none() {
        let promoted = vec!["service.name".to_string()];
        assert!(resolve_query("   ", &promoted).unwrap().is_none());
    }

    #[test]
    fn parse_error_reports_offset() {
        let promoted = vec!["service.name".to_string()];
        let err = resolve_query("ok :bad", &promoted).unwrap_err();
        assert_eq!(err.offset, Some(3));
    }

    #[test]
    fn unknown_field_is_an_error_without_offset() {
        // `body` is not a filterable field (it is free text) — a resolve error, which carries no
        // positional offset (resolution is not positional), so `offset` is `None`.
        let promoted = vec!["service.name".to_string()];
        let err = resolve_query("body:x", &promoted).unwrap_err();
        assert_eq!(err.offset, None);
    }

    #[test]
    fn severity_key_boundaries() {
        assert_eq!(severity_key(0), "info"); // 0/null default
        assert_eq!(severity_key(1), "debug");
        assert_eq!(severity_key(8), "debug");
        assert_eq!(severity_key(9), "info");
        assert_eq!(severity_key(12), "info");
        assert_eq!(severity_key(13), "warn");
        assert_eq!(severity_key(16), "warn");
        assert_eq!(severity_key(17), "error");
        assert_eq!(severity_key(20), "error");
        assert_eq!(severity_key(21), "fatal");
        assert_eq!(severity_key(24), "fatal");
    }

    #[test]
    fn query_time_only_when_no_filters() {
        let q = build_query(&request(&[], &[], ""), None).unwrap();
        assert_eq!(q.start_ts_nanos, 100);
        assert_eq!(q.end_ts_nanos, 200);
        assert!(q.services.is_empty());
        assert!(q.severities.is_empty());
        assert_eq!(q.text, None);
        assert_eq!(q.limit, 500);
    }

    #[test]
    fn query_maps_services_severities_and_text() {
        let q = build_query(&request(&["api", "web"], &["info", "error"], "boom"), None).unwrap();
        assert_eq!(q.services, vec!["api".to_string(), "web".to_string()]);
        // info -> (9,12), error -> (17,20), in request order.
        assert_eq!(q.severities, vec![(9, 12), (17, 20)]);
        assert_eq!(q.text.as_deref(), Some("boom"));
        assert_eq!(q.limit, 500);
    }

    #[test]
    fn unknown_severity_keys_are_ignored() {
        let q = build_query(&request(&[], &["nonsense"], ""), None).unwrap();
        assert!(q.severities.is_empty());
    }

    #[test]
    fn bad_timestamp_is_an_error() {
        let mut req = request(&[], &[], "");
        req.start_ts_nanos = "not-a-number".to_string();
        assert!(build_query(&req, None).is_err());
    }

    #[test]
    fn record_batch_converts_to_json_row() {
        let schema = LogSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        let mut builder = RecordBatchBuilder::new(&schema);

        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), "api".to_string());
        attributes.insert("host.name".to_string(), "api-1".to_string());
        attributes.insert("region".to_string(), "us-east-1".to_string());

        builder.append(&LogRecord {
            timestamp_nanos: 1_700_000_000_000_000_000,
            severity_number: Some(18),
            body: Some("kaboom".to_string()),
            trace_id: Some("abc123".to_string()),
            attributes,
            ..Default::default()
        });
        let batch = builder.finish().unwrap();

        let rows = batches_to_rows(&[batch]);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];

        assert_eq!(r["id"], json!(0));
        // timestamp is the i64 nanos rendered as a string (not a JS number).
        assert_eq!(r["timestamp"], json!("1700000000000000000"));
        assert!(r["timestamp"].is_string());
        assert_eq!(r["severity"], json!("error")); // 18 -> error
        assert_eq!(r["service"], json!("api"));
        assert_eq!(r["body"], json!("kaboom"));
        assert_eq!(r["trace_id"], json!("abc123"));
        assert_eq!(r["span_id"], Value::Null);

        let attrs = r["attributes"].as_object().unwrap();
        // promoted (non-service.name) column folded into attributes
        assert_eq!(attrs["host.name"], json!("api-1"));
        // long-tail map attribute
        assert_eq!(attrs["region"], json!("us-east-1"));
        // service.name is surfaced as `service`, not duplicated into attributes
        assert!(!attrs.contains_key("service.name"));
    }

    #[tokio::test]
    async fn search_returns_envelope_with_counts() {
        use tower::ServiceExt;
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let body = serde_json::json!({
            "start_ts_nanos": "0",
            "end_ts_nanos": "9223372036854775807",
            "query": "",
            "limit": 500
        })
        .to_string();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/search")
                    .header("content-type", "application/json")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["rows"].is_array());
        assert_eq!(v["matched_count"], serde_json::json!(0));
        assert!(v["elapsed_ms"].is_number());
    }
}

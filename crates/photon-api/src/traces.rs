//! Trace waterfall data path: `GET /api/traces/:trace_id` returns every span of one trace as
//! JSON, converted from the spans query engine's `RecordBatch`es. The frontend assembles the
//! tree from `parent_span_id`. Timestamps cross as decimal-nanosecond strings (JS-safe).

use std::time::Instant;

use arrow::array::{
    Array, Int32Array, Int64Array, MapArray, StringArray, TimestampNanosecondArray,
};
use arrow::record_batch::RecordBatch;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_core::span_schema;

use crate::search::{downcast, string_or_null};
use crate::AppState;

/// Query string for `GET /api/traces/:trace_id`. `time_hint` is a decimal-nanosecond string used
/// to narrow candidate-file selection; absent/empty means "scan all candidates".
#[derive(Debug, Deserialize)]
pub(crate) struct TraceParams {
    time_hint: Option<String>,
}

/// `GET /api/traces/:trace_id` — all spans of a trace (waterfall payload). 404 when the trace is
/// not in the local hot store; 500 on a query error.
pub(crate) async fn get_trace(
    State(state): State<AppState>,
    Path(trace_id): Path<String>,
    Query(params): Query<TraceParams>,
) -> Response {
    let time_hint = match params.time_hint.as_deref() {
        Some(s) if !s.is_empty() => match s.parse::<i64>() {
            Ok(v) => Some(v),
            Err(_) => {
                return (StatusCode::BAD_REQUEST, format!("invalid time_hint: {s}")).into_response()
            }
        },
        _ => None,
    };

    let started = Instant::now();
    let batches = match state.span_query.get_trace(&trace_id, time_hint).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("photon-api: error: get_trace failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "trace query failed" })),
            )
                .into_response();
        }
    };
    let spans = spans_to_json(&batches);
    let elapsed_ms = started.elapsed().as_millis() as u64;

    if spans.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "trace not found" })),
        )
            .into_response();
    }

    Json(json!({
        "trace_id": trace_id,
        "spans": spans,
        "elapsed_ms": elapsed_ms,
    }))
    .into_response()
}

fn spans_to_json(batches: &[RecordBatch]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut id: i64 = 0;
    for batch in batches {
        for row in 0..batch.num_rows() {
            out.push(span_row_to_json(batch, row, id));
            id += 1;
        }
    }
    out
}

/// A nullable Int32 column value at `row`, as a JSON number or `null`.
fn i32_or_null(batch: &RecordBatch, name: &str, row: usize) -> Value {
    match downcast::<Int32Array>(batch, name) {
        Some(col) if !col.is_null(row) => json!(col.value(row)),
        _ => Value::Null,
    }
}

/// A nullable Int64 column value at `row`, as a JSON number or `null`.
fn i64_num_or_null(batch: &RecordBatch, name: &str, row: usize) -> Value {
    match downcast::<Int64Array>(batch, name) {
        Some(col) if !col.is_null(row) => json!(col.value(row)),
        _ => Value::Null,
    }
}

/// A nullable Int64 column value at `row`, as a JSON *string* (JS-safe) or `null`.
fn i64_string_or_null(batch: &RecordBatch, name: &str, row: usize) -> Value {
    match downcast::<Int64Array>(batch, name) {
        Some(col) if !col.is_null(row) => Value::String(col.value(row).to_string()),
        _ => Value::Null,
    }
}

/// The timestamp column at `row` as a decimal-nanosecond string; `"0"` when null/absent.
fn ts_string(batch: &RecordBatch, name: &str, row: usize) -> Value {
    match downcast::<TimestampNanosecondArray>(batch, name) {
        Some(col) if !col.is_null(row) => Value::String(col.value(row).to_string()),
        _ => Value::String("0".to_string()),
    }
}

/// Parse a JSON-string column value at `row` into a JSON value; `null` when absent or unparseable.
fn json_or_null(batch: &RecordBatch, name: &str, row: usize) -> Value {
    match downcast::<StringArray>(batch, name) {
        Some(col) if !col.is_null(row) => {
            serde_json::from_str(col.value(row)).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

/// The long-tail attributes Map plus any promoted column other than `service.name`.
fn span_attributes(batch: &RecordBatch, row: usize) -> serde_json::Map<String, Value> {
    let mut attributes = serde_json::Map::new();

    if let Some(map) = downcast::<MapArray>(batch, span_schema::ATTRIBUTES) {
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

    // Promoted columns other than `service.name` (surfaced separately as `service`).
    let batch_schema = batch.schema();
    for field in batch_schema.fields() {
        let name = field.name();
        if span_schema::SPAN_FIXED_COLUMNS.contains(&name.as_str()) || name == "service.name" {
            continue;
        }
        if let Some(col) = downcast::<StringArray>(batch, name) {
            if !col.is_null(row) {
                attributes.insert(name.clone(), Value::String(col.value(row).to_string()));
            }
        }
    }
    attributes
}

/// Convert one span row into the JSON shape shared by `GET /api/traces/:trace_id` (the waterfall)
/// and `POST /api/spans/search` (the spans table) — one source of truth for the span JSON shape.
/// `id` is a running index across all rows in the response (unused by the waterfall, which keys
/// off `span_id`/`parent_span_id`; the spans table uses it for row selection like the log rows).
pub(crate) fn span_row_to_json(batch: &RecordBatch, row: usize, id: i64) -> Value {
    let service = match downcast::<StringArray>(batch, "service.name") {
        Some(col) if !col.is_null(row) => col.value(row).to_string(),
        _ => String::new(),
    };

    json!({
        "id": id,
        "trace_id": string_or_null(batch, span_schema::TRACE_ID, row),
        "span_id": string_or_null(batch, span_schema::SPAN_ID, row),
        "parent_span_id": string_or_null(batch, span_schema::PARENT_SPAN_ID, row),
        "name": string_or_null(batch, span_schema::NAME, row),
        "kind": i32_or_null(batch, span_schema::KIND, row),
        "kind_text": string_or_null(batch, span_schema::KIND_TEXT, row),
        "start_time_nanos": ts_string(batch, span_schema::START_TIME, row),
        "end_time_nanos": i64_string_or_null(batch, span_schema::END_TIME, row),
        "duration_nanos": i64_num_or_null(batch, span_schema::DURATION, row),
        "status_code": i32_or_null(batch, span_schema::STATUS_CODE, row),
        "status_text": string_or_null(batch, span_schema::STATUS_TEXT, row),
        "status_message": string_or_null(batch, span_schema::STATUS_MESSAGE, row),
        "scope_name": string_or_null(batch, span_schema::SCOPE_NAME, row),
        "service": service,
        "events": json_or_null(batch, span_schema::EVENTS, row),
        "links": json_or_null(batch, span_schema::LINKS, row),
        "attributes": Value::Object(span_attributes(batch, row)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;

    fn sample_batch() -> RecordBatch {
        let schema = SpanSchema::new(&["service.name".to_string()]);
        let mut b = SpanBatchBuilder::new(&schema);
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), "checkout".to_string());
        attributes.insert("http.method".to_string(), "POST".to_string());
        b.append(&SpanRecord {
            trace_id: "t1".to_string(),
            span_id: "s1".to_string(),
            parent_span_id: None,
            name: Some("charge".to_string()),
            kind: Some(3),
            kind_text: Some("CLIENT".to_string()),
            start_time_nanos: 1_700_000_000_000_000_000,
            end_time_nanos: Some(1_700_000_000_500_000_000),
            duration_nanos: Some(500_000_000),
            status_code: Some(2),
            status_text: Some("ERROR".to_string()),
            status_message: Some("boom".to_string()),
            scope_name: Some("payments".to_string()),
            events: Some(
                r#"[{"name":"retry","time_unix_nano":"1700000000100000000"}]"#.to_string(),
            ),
            links: None,
            attributes,
        });
        b.finish().unwrap()
    }

    #[test]
    fn span_batch_converts_to_json() {
        let rows = spans_to_json(&[sample_batch()]);
        assert_eq!(rows.len(), 1);
        let s = &rows[0];
        assert_eq!(s["trace_id"], json!("t1"));
        assert_eq!(s["span_id"], json!("s1"));
        assert_eq!(s["parent_span_id"], Value::Null);
        assert_eq!(s["name"], json!("charge"));
        assert_eq!(s["kind"], json!(3));
        // start/end are JS-safe strings; duration is a number.
        assert_eq!(s["start_time_nanos"], json!("1700000000000000000"));
        assert!(s["start_time_nanos"].is_string());
        assert_eq!(s["end_time_nanos"], json!("1700000000500000000"));
        assert_eq!(s["duration_nanos"], json!(500_000_000));
        assert_eq!(s["status_code"], json!(2));
        assert_eq!(s["service"], json!("checkout"));
        // events JSON string was parsed into an array.
        assert!(s["events"].is_array());
        assert_eq!(s["events"][0]["name"], json!("retry"));
        assert_eq!(s["links"], Value::Null);
        // promoted non-service.name column folded into attributes.
        assert_eq!(s["attributes"]["http.method"], json!("POST"));
        assert!(!s["attributes"]
            .as_object()
            .unwrap()
            .contains_key("service.name"));
    }

    #[tokio::test]
    async fn get_trace_requires_session() {
        use tower::ServiceExt;
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/api/traces/whatever")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_trace_unknown_is_404() {
        use tower::ServiceExt;
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/api/traces/deadbeef")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

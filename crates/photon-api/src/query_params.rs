//! Shared parsing for the UI query-string params (`query`, `start`, `end`) used by `/api/search`,
//! `/api/facet`, and `/api/histogram` (and their spans siblings under `/api/traces/*` +
//! `/api/spans/search`). Keeps grammar parse/resolve error → HTTP mapping in one place.
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use photon_core::query::{
    parse, FieldResolver, ResolvedQuery, SpanFieldResolver, SpanResolvedQuery,
};
use photon_query::{QueryRequest, SpanQueryRequest, SpanSort};

/// A bad request param, with an optional character offset (set for grammar parse errors so the UI
/// can underline the offending character).
#[derive(Debug)]
pub(crate) struct QueryParamError {
    pub message: String,
    pub offset: Option<usize>,
}

impl QueryParamError {
    pub(crate) fn into_response(self) -> Response {
        let offset = self.offset.map(|o| json!(o)).unwrap_or(Value::Null);
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": self.message, "offset": offset })),
        )
            .into_response()
    }
}

/// Parse+resolve the grammar `query`. Empty/blank → `None` (no grammar filter).
pub(crate) fn resolve_query(
    query: &str,
    promoted: &[String],
) -> Result<Option<ResolvedQuery>, QueryParamError> {
    if query.trim().is_empty() {
        return Ok(None);
    }
    let ast = parse(query).map_err(|e| QueryParamError {
        message: e.message,
        offset: Some(e.offset),
    })?;
    let resolved = FieldResolver::new(promoted)
        .resolve(&ast)
        .map_err(|e| QueryParamError {
            message: e.message,
            offset: None,
        })?;
    Ok(Some(resolved))
}

/// Parse the `[start, end]` epoch-nanosecond window (sent as decimal strings to dodge JS 2^53).
pub(crate) fn parse_window(start: &str, end: &str) -> Result<(i64, i64), QueryParamError> {
    let s = start.parse().map_err(|_| QueryParamError {
        message: format!("invalid start: {start}"),
        offset: None,
    })?;
    let e = end.parse().map_err(|_| QueryParamError {
        message: format!("invalid end: {end}"),
        offset: None,
    })?;
    Ok((s, e))
}

/// Build a `QueryRequest` for an aggregation endpoint from the shared params. `services` /
/// `severities` / `text` are empty and `limit` is 0 — aggregations carry all filters in the
/// grammar `query` and never use the row limit.
pub(crate) fn build_query_request(
    query: &str,
    start: &str,
    end: &str,
    promoted: &[String],
) -> Result<QueryRequest, QueryParamError> {
    let resolved = resolve_query(query, promoted)?;
    let (start_ts_nanos, end_ts_nanos) = parse_window(start, end)?;
    Ok(QueryRequest {
        start_ts_nanos,
        end_ts_nanos,
        services: Vec::new(),
        severities: Vec::new(),
        text: None,
        query: resolved,
        limit: 0,
    })
}

/// Parse+resolve the grammar `query` against the SPANS schema (sibling of [`resolve_query`]).
/// Empty/blank → `None` (no grammar filter).
pub(crate) fn resolve_span_query(
    query: &str,
    promoted: &[String],
) -> Result<Option<SpanResolvedQuery>, QueryParamError> {
    if query.trim().is_empty() {
        return Ok(None);
    }
    let ast = parse(query).map_err(|e| QueryParamError {
        message: e.message,
        offset: Some(e.offset),
    })?;
    let resolved = SpanFieldResolver::new(promoted)
        .resolve(&ast)
        .map_err(|e| QueryParamError {
            message: e.message,
            offset: None,
        })?;
    Ok(Some(resolved))
}

/// Map a UI sort key to `SpanSort`; unknown or absent values default to `Recent`.
pub(crate) fn parse_sort(s: &str) -> SpanSort {
    match s {
        "slowest" => SpanSort::Slowest,
        "errors" => SpanSort::Errors,
        _ => SpanSort::Recent,
    }
}

/// Build a `SpanQueryRequest` for a spans/traces endpoint from the shared params. Unlike
/// [`build_query_request`] (logs aggregations, which never page), spans searches are paged, so
/// `sort`/`limit`/`offset` are caller-supplied rather than zeroed; `limit` is clamped to 1000.
pub(crate) fn build_span_query_request(
    query: &str,
    start: &str,
    end: &str,
    sort: &str,
    limit: usize,
    offset: usize,
    promoted: &[String],
) -> Result<SpanQueryRequest, QueryParamError> {
    let resolved = resolve_span_query(query, promoted)?;
    let (start_ts_nanos, end_ts_nanos) = parse_window(start, end)?;
    Ok(SpanQueryRequest {
        start_ts_nanos,
        end_ts_nanos,
        query: resolved,
        sort: parse_sort(sort),
        limit: limit.min(1000),
        offset,
        projected_attributes: Vec::new(),
    })
}

#[cfg(test)]
mod span_query_tests {
    use super::*;

    #[test]
    fn resolves_a_valid_span_query() {
        let promoted = vec!["service.name".to_string()];
        let rq = resolve_span_query("status:error duration>=500ms", &promoted).unwrap();
        assert_eq!(rq.map(|q| q.terms.len()), Some(2));
    }

    #[test]
    fn empty_span_query_resolves_to_none() {
        let promoted = vec!["service.name".to_string()];
        assert!(resolve_span_query("   ", &promoted).unwrap().is_none());
    }

    #[test]
    fn span_query_parse_error_reports_offset() {
        let promoted = vec!["service.name".to_string()];
        let err = resolve_span_query("ok :bad", &promoted).unwrap_err();
        assert_eq!(err.offset, Some(3));
    }

    #[test]
    fn parse_sort_maps_known_keys_and_defaults_to_recent() {
        assert_eq!(parse_sort("slowest"), SpanSort::Slowest);
        assert_eq!(parse_sort("errors"), SpanSort::Errors);
        assert_eq!(parse_sort("recent"), SpanSort::Recent);
        assert_eq!(parse_sort("nonsense"), SpanSort::Recent);
        assert_eq!(parse_sort(""), SpanSort::Recent);
    }

    #[test]
    fn build_span_query_request_clamps_limit_and_carries_offset() {
        let promoted = vec!["service.name".to_string()];
        let req =
            build_span_query_request("", "100", "200", "slowest", 5000, 40, &promoted).unwrap();
        assert_eq!(req.start_ts_nanos, 100);
        assert_eq!(req.end_ts_nanos, 200);
        assert_eq!(req.sort, SpanSort::Slowest);
        assert_eq!(req.limit, 1000);
        assert_eq!(req.offset, 40);
        assert!(req.query.is_none());
    }

    #[test]
    fn build_span_query_request_bad_window_is_an_error() {
        let promoted = vec!["service.name".to_string()];
        let err = build_span_query_request("", "not-a-number", "200", "recent", 10, 0, &promoted)
            .unwrap_err();
        assert_eq!(err.offset, None);
    }
}

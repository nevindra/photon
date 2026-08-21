//! Backend Issues over the API: `/api/services/errors` (fleet-wide) and the three per-service
//! routes under `/api/services/:service/errors`.
//!
//! Shaped as a deliberate mirror of `rum.rs`'s `/api/rum/errors*` — same route names, same list /
//! facets / detail split, same "200 with empty sections" answer for a fingerprint that did not
//! occur in the window. A Frontend Issue and a Backend Issue are the same idea about two
//! runtimes, and an engineer who has read one page should not have to relearn the other.
//!
//! Where they differ, they differ because the runtimes do: RUM counts distinct **sessions**, this
//! counts distinct **traces** — one broken request rather than one affected person.
//!
//! The identity rule (`(service, fingerprint)`, never the fingerprint alone) lives in
//! `photon_query::span_issues`; these handlers only carry the service through.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::{IssueEvent, IssueTag, SpanIssue, SpanIssueDetail, ISSUE_FACET_FIELDS};

use crate::query_params::{build_span_query_request, clamp_limit, grammar_term};
use crate::AppState;

/// Max Issues returned by a list route. Same number as `rum.rs`'s `ERROR_LIMIT`.
const ISSUE_LIMIT: usize = 100;

/// Top-N values returned per facet field.
const ISSUE_FACET_LIMIT: usize = 20;

/// Sample occurrences carried on a detail. Twenty is enough to see a pattern (one host? one
/// environment? one operation?) without turning the detail into a span search.
const SAMPLE_EVENTS: usize = 20;

#[derive(Deserialize)]
pub(crate) struct IssuesParams {
    #[serde(default)]
    q: String,
    start: String,
    end: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
pub(crate) struct IssueDetailParams {
    #[serde(default)]
    q: String,
    start: String,
    end: String,
}

/// The caller's query narrowed to one service, when there is one.
///
/// `grammar_term` quotes a value containing whitespace or a comma. Service names rarely need it,
/// but the same helper is used everywhere precisely so nobody has to decide per call site which
/// values are "safe" — see `service_operations::scoped_query`.
fn scoped_query(base: &str, service: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !base.trim().is_empty() {
        parts.push(base.trim().to_string());
    }
    if let Some(s) = service {
        parts.extend(grammar_term("service.name", s));
    }
    parts.join(" ")
}

/// `GET /api/services/errors?start&end&q&limit` — Backend Issues across the whole fleet, ordered
/// by occurrence count. Every row names its own service, because the same fingerprint in two
/// services is two Issues.
pub(crate) async fn fleet_errors(
    State(state): State<AppState>,
    Query(p): Query<IssuesParams>,
) -> Response {
    issues_response(state, None, p).await
}

/// `GET /api/services/:service/errors?start&end&q&limit` — the same list, scoped to one service.
pub(crate) async fn errors(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Query(p): Query<IssuesParams>,
) -> Response {
    issues_response(state, Some(service), p).await
}

async fn issues_response(state: AppState, service: Option<String>, p: IssuesParams) -> Response {
    let q = scoped_query(&p.q, service.as_deref());
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
    let limit = clamp_limit(p.limit.unwrap_or(ISSUE_LIMIT));

    let issues = match state.span_query.span_issues(req, limit).await {
        Ok(v) => v,
        Err(e) => return query_error(e),
    };
    let errors: Vec<Value> = issues.iter().map(issue_json).collect();
    let mut body = json!({
        "window": { "start": start_ns.to_string(), "end": end_ns.to_string() },
        "errors": errors,
    });
    if let (Some(s), Some(obj)) = (service, body.as_object_mut()) {
        obj.insert("service".into(), Value::String(s));
    }
    Json(body).into_response()
}

/// `GET /api/services/:service/errors/facets?start&end&q` — top values (+counts) for each of the
/// six Issue facet fields, over the service's **errored** spans only.
///
/// "Errored spans only" is the whole point: a facet over all spans would report that 99% of
/// traffic is `kind_text:SERVER`, which is true and useless. Scoping to `error.fingerprint:*`
/// makes every bar an answer to "which of these is breaking".
pub(crate) async fn error_facets(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Query(p): Query<IssuesParams>,
) -> Response {
    let mut q = scoped_query(&p.q, Some(&service));
    q.push(' ');
    q.push_str(&format!("{}:*", photon_core::span_error::ERROR_FINGERPRINT));

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

    let mut facets = serde_json::Map::new();
    for field in ISSUE_FACET_FIELDS {
        let fr = match state
            .span_query
            .facet(field, req.clone(), ISSUE_FACET_LIMIT)
            .await
        {
            Ok(f) => f,
            Err(e) => return query_error(e),
        };
        facets.insert(
            field.to_string(),
            json!({
                "values": fr.values.iter().map(|v| json!({ "value": v.value, "count": v.count })).collect::<Vec<_>>(),
                "capped": fr.capped,
            }),
        );
    }
    Json(json!({ "service": service, "facets": facets })).into_response()
}

/// `GET /api/services/:service/errors/:fingerprint?start&end&q` — full detail for one Issue:
/// header stats, an occurrence series, tag breakdowns, a sample stack trace, and recent sample
/// occurrences. 200 with all-empty sections when the Issue had no occurrence in the window — a
/// 404 would say the Issue does not exist, when what happened is that it did not happen *here*.
pub(crate) async fn error_detail(
    State(state): State<AppState>,
    Path((service, fingerprint)): Path<(String, String)>,
    Query(p): Query<IssueDetailParams>,
) -> Response {
    // The service scope is applied inside the engine (with the fingerprint, as one identity), so
    // the request query carries only the caller's own filter.
    let req = match build_span_query_request(
        p.q.trim(),
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
        .span_issue_detail(req, &service, &fingerprint, SAMPLE_EVENTS)
        .await
    {
        Ok(d) => Json(detail_json(&d)).into_response(),
        Err(e) => query_error(e),
    }
}

fn issue_json(i: &SpanIssue) -> Value {
    json!({
        "service": i.service,
        "fingerprint": i.fingerprint,
        "error_type": i.error_type,
        "message": i.message,
        "count": i.count,
        "traces": i.traces,
        "operations": i.operations,
        // Epoch NANOSECOND strings, matching every other span timestamp on this surface.
        "first_seen": i.first_seen.to_string(),
        "last_seen": i.last_seen.to_string(),
        "trace_id": i.trace_id,
    })
}

fn detail_json(d: &SpanIssueDetail) -> Value {
    json!({
        "service": d.service,
        "fingerprint": d.fingerprint,
        "error_type": d.error_type,
        "message": d.message,
        "first_seen": d.first_seen.to_string(),
        "last_seen": d.last_seen.to_string(),
        "occurrences": d.occurrences,
        "traces": d.traces,
        "operations": d.operations,
        // Epoch MILLIS for the chart, like every other chart payload the UI reads.
        "series": d.series.iter().map(|b| json!([b.t / 1_000_000, b.count])).collect::<Vec<_>>(),
        "tags": d.tags.iter().map(tag_json).collect::<Vec<_>>(),
        "sample_stack": d.sample_stack,
        "events": d.events.iter().map(event_json).collect::<Vec<_>>(),
    })
}

fn tag_json(t: &IssueTag) -> Value {
    json!({
        "field": t.field,
        "values": t.values.iter().map(|v| json!({ "value": v.value, "count": v.count })).collect::<Vec<_>>(),
    })
}

fn event_json(e: &IssueEvent) -> Value {
    json!({
        "timestamp": e.timestamp.to_string(),
        "trace_id": e.trace_id,
        "span_id": e.span_id,
        "operation": e.operation,
        "kind": e.kind,
        "duration_nanos": e.duration_nanos.map(|d| d.to_string()),
        "status_message": e.status_message,
        "host": e.host,
        "environment": e.environment,
    })
}

/// A query failure here is nearly always a bad grammar `q` the resolver accepted but the engine
/// could not compile — a 400, same as `/api/red` and the operations routes.
fn query_error(e: photon_core::PhotonError) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
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
    async fn the_issue_routes_require_a_session() {
        let router = crate::test_router();
        for uri in [
            "/api/services/errors?start=0&end=100",
            "/api/services/api/errors?start=0&end=100",
            "/api/services/api/errors/facets?start=0&end=100",
            "/api/services/api/errors/abc?start=0&end=100",
        ] {
            let resp = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }
    }

    #[tokio::test]
    async fn an_empty_server_has_no_issues() {
        let (status, v) = get("/api/services/api/errors?start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["errors"], json!([]));
        assert_eq!(v["service"], json!("api"));
    }

    #[tokio::test]
    async fn the_fleet_list_names_no_service_of_its_own() {
        let (status, v) = get("/api/services/errors?start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["errors"], json!([]));
        // The fleet list is not "some service's" list — each ROW carries its service instead.
        assert!(v.get("service").is_none());
    }

    #[tokio::test]
    async fn a_static_errors_segment_beats_the_service_param() {
        // `/services/errors` must be the fleet list, not service "errors"'s list.
        let (_, v) = get("/api/services/errors?start=0&end=100").await;
        assert!(v.get("service").is_none());
    }

    #[tokio::test]
    async fn facets_answer_for_every_field_even_with_no_data() {
        let (status, v) = get("/api/services/api/errors/facets?start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        for field in ISSUE_FACET_FIELDS {
            assert_eq!(v["facets"][field]["values"], json!([]), "{field}");
        }
    }

    #[tokio::test]
    async fn a_fingerprint_with_no_occurrences_is_an_empty_detail_not_a_404() {
        let (status, v) = get("/api/services/api/errors/deadbeef?start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["fingerprint"], json!("deadbeef"));
        assert_eq!(v["occurrences"], json!(0));
        assert_eq!(v["events"], json!([]));
    }

    #[tokio::test]
    async fn a_malformed_query_is_a_400_with_an_offset() {
        let (status, v) = get("/api/services/api/errors?start=0&end=100&q=ok%20%3Abad").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(v["offset"].is_number());
    }

    #[test]
    fn the_callers_filter_is_kept_and_the_service_added() {
        let q = scoped_query("http.status_code:500", Some("api"));
        assert!(q.starts_with("http.status_code:500 "));
        assert!(q.contains("service.name:api"));
    }

    #[test]
    fn the_fleet_scope_adds_no_service_term() {
        assert_eq!(scoped_query("status:error", None), "status:error");
        assert_eq!(scoped_query("", None), "");
    }
}

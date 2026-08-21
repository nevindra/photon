//! `GET /api/services/overview` — the single request behind the Services index.
//!
//! The index is a **fleet board** ([ADR-0004](../../../docs/adr/0004-the-services-index-is-a-fleet-board.md)):
//! one row per `(service, Site, environment)`, plus a Site band across the top, plus a sparkline on
//! every row. Assembling that from the existing routes would be `1 + N` requests at a 15s poll —
//! unusable the moment a fleet has thirty services. So it is one route that returns the whole page.
//!
//! Two things it deliberately does NOT do:
//!
//! - **It does not zero a stale Site's rows.** A Site that stopped reporting has no recent data, and
//!   rendering that as `0/s` and green is the exact failure ADR-0004 was written to stop. The `sites`
//!   band carries each Site's liveness so the UI can mark its rows Unknown instead.
//! - **It does not carry a column on a different time scale.** No 30-day error budget, no cumulative
//!   data usage. Every number here is over the requested window, so the row cannot mislead by mixing
//!   three clocks with none of them labelled.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::{RedGroup, RedRow};

use crate::query_params::{build_span_query_request, clamp_buckets, grammar_term};
use crate::red::{red_row_to_json, window_seconds};
use crate::services_rollup::{red_buckets, red_rows};
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct OverviewParams {
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

/// How many rows get a sparkline. Each one is its own bucketed read, so this is the difference
/// between a page load and a thundering herd. Rows past the cap render without a sparkline rather
/// than being dropped — the table is still complete, it just stops drawing.
const MAX_SPARKLINES: usize = 40;

/// `GET /api/services/overview?start&end&buckets&q`.
pub(crate) async fn overview(
    State(state): State<AppState>,
    Query(p): Query<OverviewParams>,
) -> Response {
    let req = match build_span_query_request(
        &p.q,
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
        &p.q,
        req,
        None,
        RedGroup::Service,
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

    // Sparklines: one bucketed read per row, concurrently, for the busiest rows only.
    let sparked = rows.len().min(MAX_SPARKLINES);
    let sparks = futures::future::join_all(rows.iter().take(sparked).map(|r| {
        let state = state.clone();
        let q = row_query(&p.q, r);
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

    // The SAME row shape `/api/red` returns (`rate`, `error_rate` and `apdex` are derived there),
    // plus this route's two sparklines. Hand-rolling a second copy is how the index lost `rate` and
    // `error_rate` the first time — the frontend's `OverviewRow extends RedRow` says these are one
    // shape, so they are produced by one function.
    let window_secs = window_seconds(start_ns, end_ns);
    let out_rows: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut v = red_row_to_json(r, window_secs, source);
            let series = sparks.get(i);
            if let Some(obj) = v.as_object_mut() {
                // `[]` where the row fell past MAX_SPARKLINES — an empty sparkline, not a flat one.
                // A flat line would claim the service had no traffic.
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
        "sites": sites_block(&state).await,
        "rows": out_rows,
    }))
    .into_response()
}

/// `(epoch ms, value)` pairs — the shape the UI's chart components already read.
fn points(
    buckets: &[photon_query::RedBucket],
    f: impl Fn(&photon_query::RedBucket) -> u64,
) -> Vec<Value> {
    buckets
        .iter()
        .map(|b| json!([b.ts / 1_000_000, f(b)]))
        .collect()
}

/// The caller's query narrowed to one row's identity, for that row's sparkline.
///
/// Every term goes through `grammar_term`, which quotes a value containing whitespace or a comma
/// and drops one the grammar cannot express at all. An unquoted `service.name:my api` would parse
/// as a service plus a stray free-text word, and the sparkline would silently belong to a different
/// set of spans than its row.
fn row_query(base: &str, r: &RedRow) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !base.trim().is_empty() {
        parts.push(base.trim().to_string());
    }
    parts.extend(grammar_term("service.name", &r.service));
    if let Some(env) = &r.environment {
        parts.extend(grammar_term("deployment.environment", env));
    }
    if let Some(t) = r.tenant.as_deref() {
        parts.extend(grammar_term("tenant", t));
    }
    parts.join(" ")
}

/// The Site band: the tenant registry's liveness facts, and nothing else.
///
/// Deliberately NOT this Site's per-service totals. Those are already in `rows` — one row per
/// `(service, Site, environment)` — and the frontend's fleet merge has to walk them anyway to
/// compute Spread. Sending a second, separately-computed copy is how a band and the table under it
/// start disagreeing about the same Site in the same response.
///
/// What the frontend genuinely cannot derive is here: whether the Site is still pushing, when it
/// was last heard from, which federation mode it runs, and where its own UI lives.
///
/// Empty on a Site install (the registry is empty), which is what makes the same page work in both
/// deployments — the band simply does not render.
async fn sites_block(state: &AppState) -> Vec<Value> {
    let Some(api) = state.tenants.as_ref() else {
        return Vec::new();
    };
    let Ok(tenants) = api.list().await else {
        return Vec::new();
    };
    crate::tenants::summaries_for(state, &tenants)
        .await
        .into_iter()
        .map(|s| {
            json!({
                "name": s.name,
                "status": s.status,
                "last_seen_ms": s.last_seen_ms,
                "mode": s.mode,
                "ui_url": s.ui_url,
            })
        })
        .collect()
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

    fn row(service: &str, tenant: Option<&str>, environment: Option<&str>) -> RedRow {
        RedRow {
            service: service.into(),
            tenant: tenant.map(String::from),
            environment: environment.map(String::from),
            operation: None,
            count: 1,
            span_count: 1,
            basis: photon_query::RedBasis::Entry,
            error_count: 0,
            p50: 0,
            p90: 0,
            p99: 0,
            satisfied: 0,
            tolerating: 0,
            frustrated: 0,
            http_4xx: 0,
            http_5xx: 0,
        }
    }

    #[tokio::test]
    async fn requires_session() {
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/services/overview?start=0&end=100")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn empty_server_returns_empty_rows() {
        let (status, v) = get("/api/services/overview?start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["rows"], json!([]));
        assert_eq!(v["source"], json!("raw"));
        assert_eq!(v["window"]["buckets"], json!(30));
    }

    #[tokio::test]
    async fn sites_block_is_empty_without_a_registry() {
        let (_, v) = get("/api/services/overview?start=0&end=100").await;
        assert_eq!(
            v["sites"],
            json!([]),
            "a Site install has no tenant registry, and the band must simply not render"
        );
    }

    #[tokio::test]
    async fn clamps_a_dos_sized_bucket_count() {
        let (status, v) = get("/api/services/overview?start=0&end=100&buckets=2000000000").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            v["window"]["buckets"],
            json!(crate::query_params::MAX_BUCKETS)
        );
    }

    #[tokio::test]
    async fn a_bad_query_is_a_400_with_an_offset() {
        let (status, v) = get("/api/services/overview?q=ok+%3Abad&start=0&end=100").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["offset"], json!(3));
    }

    #[test]
    fn a_row_carries_the_same_derived_fields_as_the_red_route() {
        // The index reads `rate`, `error_rate` and `apdex` off every row. All three are DERIVED,
        // not stored, so the hand-rolled row shape this route started with dropped them silently
        // and the table rendered blanks. Reusing `/api/red`'s builder is the fix; this pins it.
        let mut r = row("api", None, Some("prod"));
        r.count = 100;
        r.error_count = 5;
        r.satisfied = 90;
        r.tolerating = 10;
        let v = red_row_to_json(&r, 10.0, crate::services_rollup::RedSource::RawSpans);
        assert_eq!(v["rate"], json!(10.0));
        assert_eq!(v["error_rate"], json!(0.05));
        assert_eq!(v["apdex"], json!(0.95));
        assert_eq!(v["environment"], json!("prod"));
    }

    #[test]
    fn a_rows_sparkline_query_is_scoped_to_that_row_alone() {
        let q = row_query("", &row("payments-api", Some("jkt"), Some("prod")));
        assert!(q.contains("service.name:payments-api"));
        assert!(q.contains("deployment.environment:prod"));
        assert!(q.contains("tenant:jkt"));
    }

    #[test]
    fn the_callers_own_filter_is_kept_not_replaced() {
        let q = row_query("http.route:/checkout", &row("api", None, None));
        assert!(q.starts_with("http.route:/checkout "));
        assert!(q.contains("service.name:api"));
    }

    #[test]
    fn a_service_name_with_a_space_is_quoted_and_still_resolves_to_that_name() {
        // Unquoted, `service.name:my api` would parse as a service plus a stray free-text word,
        // and the sparkline would silently belong to a different set of spans than its row. The
        // parse assertion is not enough on its own — a term can parse cleanly and match nothing,
        // which is exactly what quoting used to do, so the VALUE is what this pins.
        let q = row_query("", &row("my api", None, None));
        assert!(q.contains(r#"service.name:"my api""#));
        let parsed = photon_core::query::parse(&q).unwrap();
        assert_eq!(
            parsed.terms[0].kind,
            photon_core::query::TermKind::Match {
                field: "service.name".into(),
                values: vec!["my api".into()],
            }
        );
    }

    #[test]
    fn a_name_the_grammar_cannot_express_drops_its_term_instead_of_mismatching() {
        // There is no escape for a `"` in the grammar. A term built around one would parse and
        // then match nothing — a blank sparkline with no error. Widening is the honest failure.
        let q = row_query("", &row(r#"we"ird"#, None, None));
        assert!(!q.contains("service.name"));
    }

    #[test]
    fn an_empty_tenant_label_is_not_turned_into_a_term() {
        // The rollup reader fills `tenant` with `Some("")` for a local (untenanted) service;
        // `tenant:""` would match nothing and blank the row's sparkline.
        let q = row_query("", &row("api", Some(""), None));
        assert!(!q.contains("tenant:"));
    }
}

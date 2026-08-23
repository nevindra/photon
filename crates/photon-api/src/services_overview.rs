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

use std::collections::HashMap;

use photon_query::{RedGroup, RedSeriesKey, SparkPoint};

use crate::query_params::{build_span_query_request, clamp_buckets};
use crate::red::{red_row_to_json, window_seconds};
use crate::services_rollup::red_rows;
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

    // Sparklines for every row, from ONE bucketed pass keyed by the same tuple the rows are.
    let sparks = spark_series(&state, &p.q, &p.start, &p.end, buckets, RedGroup::Service).await;

    // The SAME row shape `/api/red` returns (`rate`, `error_rate` and `apdex` are derived there),
    // plus this route's two sparklines. Hand-rolling a second copy is how the index lost `rate` and
    // `error_rate` the first time — the frontend's `OverviewRow extends RedRow` says these are one
    // shape, so they are produced by one function.
    let window_secs = window_seconds(start_ns, end_ns);
    let out_rows: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut v = red_row_to_json(r, window_secs, source);
            let series = sparks.as_ref().and_then(|s| {
                s.get(&RedSeriesKey {
                    service: r.service.clone(),
                    tenant: r.tenant.clone(),
                    environment: r.environment.clone(),
                    operation: None,
                })
            });
            if let Some(obj) = v.as_object_mut() {
                // `[]` when the series is missing — an empty sparkline, not a flat one. A flat
                // line would claim the service had no traffic.
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
pub(crate) fn points(buckets: &[SparkPoint], f: impl Fn(&SparkPoint) -> u64) -> Vec<Value> {
    buckets
        .iter()
        .map(|b| json!([b.ts / 1_000_000, f(b)]))
        .collect()
}

/// Every row's sparkline for one board, in a single bucketed pass over the same spans the RED
/// aggregate just read.
///
/// This replaced a `1 + N` fan-out — one `red_timeseries` call per row, concurrently, capped at 40
/// — which re-scanned the whole window once per row. On 86.6M spans a 24h Services index was eight
/// scans instead of two, and an Operations tab with 40 rows would have been forty-one.
///
/// A failure here is not fatal: the board still renders, just without sparklines. Losing the
/// mini-charts is much better than 500-ing the page they sit on.
pub(crate) async fn spark_series(
    state: &AppState,
    q: &str,
    start: &str,
    end: &str,
    buckets: usize,
    group: RedGroup,
) -> Option<HashMap<RedSeriesKey, Vec<SparkPoint>>> {
    let req = build_span_query_request(
        q,
        start,
        end,
        "recent",
        0,
        0,
        state.span_query.promoted_attributes(),
    )
    .ok()?;
    let (start_ns, end_ns) = (req.start_ts_nanos, req.end_ts_nanos);

    // Off the rollup when it covers the window, same rule and same seam the rows above use — a
    // board whose numbers came from the rollup and whose sparklines came from a raw span scan
    // would pay the full cost of the scan it was built to avoid, and could draw a shape its own
    // totals disagree with.
    let width = ((end_ns - start_ns).max(1) / buckets.max(1) as i64).max(1);
    let coverage = match &state.rollup {
        Some(status) => status.coverage().await.unwrap_or(None),
        None => None,
    };
    if let Some(through) =
        crate::services_rollup::rollup_split(q, start_ns, end_ns, Some(width), coverage)
    {
        match crate::services_rollup::spark_from_rollup(
            &state.metrics_query,
            q,
            start_ns,
            end_ns,
            buckets,
            group,
        )
        .await
        {
            Ok(mut prefix) => {
                // Same seam as the rows: the rollup cannot see the last ~11 minutes, and a
                // sparkline that ends in zeros reads as an outage. Top the tail up from spans —
                // see `services_rollup::rollup_split`.
                if through < end_ns {
                    let from_idx = ((through - start_ns) / width).max(0);
                    let from_ts = start_ns + from_idx * width;
                    let n = (((end_ns - from_ts) + width - 1) / width).max(1) as usize;
                    let mut tail_req = req.clone();
                    tail_req.start_ts_nanos = from_ts;
                    match state
                        .span_query
                        .red_spark_series(tail_req, from_ts, end_ns, n, group)
                        .await
                    {
                        Ok(tail) => splice_spark(&mut prefix, from_idx as usize, tail),
                        Err(e) => {
                            eprintln!("photon-api: sparkline tail failed ({e}) — prefix only")
                        }
                    }
                }
                return Some(prefix);
            }
            Err(e) => eprintln!("photon-api: rollup sparklines failed ({e}) — falling back to raw"),
        }
    }

    match state
        .span_query
        .red_spark_series(req, start_ns, end_ns, buckets, group)
        .await
    {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("photon-api: warning: sparkline pass failed, rendering without it: {e}");
            None
        }
    }
}

/// Overwrite each series' trailing buckets with the raw-span tail, keeping the grid's timestamps.
/// A series only the tail saw is added whole — it started serving inside the unsettled window, and
/// its zero-filled lead-in is the truth about the rest of the chart.
fn splice_spark(
    prefix: &mut HashMap<RedSeriesKey, Vec<SparkPoint>>,
    from: usize,
    tail: HashMap<RedSeriesKey, Vec<SparkPoint>>,
) {
    for (key, points) in tail {
        let Some(slots) = prefix.get_mut(&key) else {
            prefix.insert(key, points);
            continue;
        };
        for (i, p) in points.into_iter().enumerate() {
            let Some(slot) = slots.get_mut(from + i) else {
                break;
            };
            slot.count = p.count;
            slot.error_count = p.error_count;
        }
    }
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
    use photon_query::RedRow;
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
}

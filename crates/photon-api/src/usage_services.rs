//! `GET /api/usage/services` — who is filling the disk.
//!
//! ## Why it lives on `/data` and not on the Backend index
//!
//! Retention and cost are `/data`'s job, and `/data` owns its own window. Putting a footprint
//! number in a services-index column would sit a cumulative figure beside per-window RED numbers
//! and invite exactly the comparison that is wrong.
//!
//! ## Bytes are an ESTIMATE, and the response says so
//!
//! Nothing stores bytes per service. Parquet is columnar and sorted by `(service.name, timestamp)`,
//! so a service's rows are physically contiguous and compress *together* — there is no per-service
//! byte figure to read, and computing one honestly would mean re-encoding.
//!
//! What exists is exact: a signal's total rows and total on-disk bytes (from the manifest, no data
//! scan), and a service's row count in the window (a facet, the same one the search UI uses). The
//! estimate is therefore `service_rows × (store_bytes / store_rows)` — the signal's mean bytes per
//! row. Every row of the response carries `estimated: true` and the mean it used, because a number
//! presented as measured when it was divided is how a capacity plan goes wrong quietly.
//!
//! ## Metrics are deliberately absent, and that is a fact about the data model
//!
//! Logs and spans are keyed by `service.name` — a facet over them is a real answer. Metrics are
//! keyed by `metric_name` first (it leads the sort key AND the pruning bloom), and most metric
//! series do not belong to a service at all: host and GPU metrics belong to a host, RUM vitals to a
//! browser app, and the service rollup's own output is *about* a service rather than emitted by
//! one. A facet over every metric in the store would need a metric-name-free scan, which defeats
//! the pruning the metrics engine is built on. So this route reports metrics as unattributable and
//! says why, rather than inventing a split.
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use photon_core::PhotonError;
use photon_query::{QueryRequest, SpanQueryRequest, SpanSort, StorageStats};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

use crate::query_params::{clamp_limit, parse_window};
use crate::AppState;

/// Services returned by default. The tail of a fleet is long and uninteresting for a cost
/// question — the top talkers are the whole point.
const DEFAULT_LIMIT: usize = 50;

#[derive(Deserialize)]
pub(crate) struct UsageServicesParams {
    start: String,
    end: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// One signal's exact, store-wide accounting — the denominator behind every estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SignalBasis {
    store_rows: u64,
    store_bytes: u64,
}

impl SignalBasis {
    fn from(stats: StorageStats) -> Self {
        Self {
            store_rows: stats.total_rows,
            store_bytes: stats.bytes,
        }
    }

    /// Mean bytes per row, or `0.0` for an empty store. Guarding the divide matters: a fresh
    /// install has zero rows, and `0/0` would put `NaN` into JSON, which serializes as `null` and
    /// reads as "unknown" rather than "nothing stored yet".
    fn bytes_per_row(&self) -> f64 {
        if self.store_rows == 0 {
            0.0
        } else {
            self.store_bytes as f64 / self.store_rows as f64
        }
    }

    fn estimate(&self, rows: u64) -> u64 {
        (rows as f64 * self.bytes_per_row()).round() as u64
    }
}

/// Per-service row counts for one signal, plus whether the facet hit its cap.
struct SignalCounts {
    by_service: BTreeMap<String, u64>,
    capped: bool,
}

fn empty_counts() -> SignalCounts {
    SignalCounts {
        by_service: BTreeMap::new(),
        capped: false,
    }
}

pub(crate) async fn usage_services(
    State(st): State<AppState>,
    Query(p): Query<UsageServicesParams>,
) -> Response {
    let (start_ns, end_ns) = match parse_window(&p.start, &p.end) {
        Ok(w) => w,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.message, "offset": e.offset })),
            )
                .into_response()
        }
    };
    let limit = clamp_limit(p.limit.unwrap_or(DEFAULT_LIMIT));

    // Facet both signals concurrently. Each is a group-by over already-pruned files, so the two
    // together are one page-load's worth of work rather than two.
    let logs_fut = st.query.facet(
        "service.name",
        QueryRequest {
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            services: Vec::new(),
            severities: Vec::new(),
            text: None,
            query: None,
            limit: 0,
        },
        limit,
    );
    let spans_fut = st.span_query.facet(
        "service.name",
        SpanQueryRequest {
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            query: None,
            sort: SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
        },
        limit,
    );
    let (logs, spans) = match futures::try_join!(logs_fut, spans_fut) {
        Ok(v) => v,
        Err(e) => return server_error(e),
    };

    let to_counts = |r: photon_query::FacetResult| SignalCounts {
        by_service: r
            .values
            .into_iter()
            // An empty facet value is a row with no `service.name`; it is real data taking real
            // space, so it is kept and labelled rather than dropped.
            .map(|v| {
                (
                    if v.value.is_empty() {
                        "(no service)".to_string()
                    } else {
                        v.value
                    },
                    v.count,
                )
            })
            .collect(),
        capped: r.capped,
    };
    let logs = to_counts(logs);
    let spans = to_counts(spans);
    let metrics = empty_counts();

    let logs_basis = match st.query.storage_stats() {
        Ok(s) => SignalBasis::from(s),
        Err(e) => return server_error(e),
    };
    let spans_basis = match st.span_query.storage_stats() {
        Ok(s) => SignalBasis::from(s),
        Err(e) => return server_error(e),
    };
    let metrics_basis = match st.metrics_query.storage_stats() {
        Ok(s) => SignalBasis::from(s),
        Err(e) => return server_error(e),
    };

    let rows = merge_rows(&logs, &spans, &metrics, logs_basis, spans_basis, limit);

    Json(json!({
        "window": { "start": p.start, "end": p.end },
        "rows": rows,
        // Stated on the envelope AND implied by every row's shape: these bytes were divided, not
        // measured. See the module header for why no measured figure exists.
        "estimated": true,
        "signals": {
            "logs": signal_json(logs_basis, logs.capped, true),
            "traces": signal_json(spans_basis, spans.capped, true),
            "metrics": signal_json(metrics_basis, false, false),
        },
        // The one thing a reader of this page must not conclude is that the missing metrics bytes
        // belong to nobody. They belong to hosts, browser apps, and the rollup.
        "notes": {
            "metrics": "Metrics are not attributed to services: a metric series is keyed by name, \
                        and most of them (host and GPU metrics, RUM vitals, the service rollup's \
                        own output) do not belong to a service at all.",
        },
    }))
    .into_response()
}

fn signal_json(basis: SignalBasis, capped: bool, attributed: bool) -> serde_json::Value {
    json!({
        "store_rows": basis.store_rows,
        "store_bytes": basis.store_bytes,
        "bytes_per_row": basis.bytes_per_row(),
        // `false` means this signal contributes nothing to any row's total — not that it is empty.
        "attributed": attributed,
        "capped": capped,
    })
}

/// Fold the per-signal counts into one row per service, largest estimated footprint first.
fn merge_rows(
    logs: &SignalCounts,
    spans: &SignalCounts,
    metrics: &SignalCounts,
    logs_basis: SignalBasis,
    spans_basis: SignalBasis,
    limit: usize,
) -> Vec<serde_json::Value> {
    let mut names: Vec<&String> = logs
        .by_service
        .keys()
        .chain(spans.by_service.keys())
        .chain(metrics.by_service.keys())
        .collect();
    names.sort();
    names.dedup();

    let mut rows: Vec<(u64, serde_json::Value)> = names
        .into_iter()
        .map(|name| {
            let log_rows = logs.by_service.get(name).copied().unwrap_or(0);
            let span_rows = spans.by_service.get(name).copied().unwrap_or(0);
            let log_bytes = logs_basis.estimate(log_rows);
            let span_bytes = spans_basis.estimate(span_rows);
            let bytes_total = log_bytes + span_bytes;
            (
                bytes_total,
                json!({
                    "service": name,
                    "signals": {
                        "logs": { "rows": log_rows, "bytes": log_bytes },
                        "traces": { "rows": span_rows, "bytes": span_bytes },
                    },
                    "rows_total": log_rows + span_rows,
                    "bytes_total": bytes_total,
                }),
            )
        })
        .collect();

    // Biggest first, name as the tiebreak so equal footprints order deterministically rather than
    // shuffling between refreshes.
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            a.1["service"]
                .as_str()
                .unwrap_or_default()
                .cmp(b.1["service"].as_str().unwrap_or_default())
        })
    });
    rows.into_iter().take(limit).map(|(_, v)| v).collect()
}

fn server_error(e: PhotonError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

#[cfg(test)]
mod route_tests {
    use axum::http::StatusCode;
    use serde_json::Value;

    async fn get(uri: &str) -> (StatusCode, Value) {
        use tower::ServiceExt;
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
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn an_empty_store_answers_with_an_empty_but_labelled_body() {
        // The test server has no data. The route must still say what it would have measured with —
        // a 200 with zeros beats a 404 that reads as "this feature is off".
        let (status, body) = get("/api/usage/services?start=0&end=1000000000").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["rows"].as_array().unwrap().len(), 0);
        assert_eq!(body["estimated"], serde_json::json!(true));
        assert_eq!(
            body["signals"]["logs"]["attributed"],
            serde_json::json!(true)
        );
        assert_eq!(
            body["signals"]["metrics"]["attributed"],
            serde_json::json!(false)
        );
        assert!(body["notes"]["metrics"]
            .as_str()
            .unwrap()
            .contains("not attributed"));
    }

    #[tokio::test]
    async fn a_malformed_window_is_a_400_with_the_reason() {
        let (status, body) = get("/api/usage/services?start=nope&end=1").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn the_route_needs_a_session() {
        use tower::ServiceExt;
        let resp = crate::test_router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/usage/services?start=0&end=1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, u64)], capped: bool) -> SignalCounts {
        SignalCounts {
            by_service: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            capped,
        }
    }

    fn basis(rows: u64, bytes: u64) -> SignalBasis {
        SignalBasis {
            store_rows: rows,
            store_bytes: bytes,
        }
    }

    #[test]
    fn an_empty_store_has_no_bytes_per_row_rather_than_nan() {
        // `0/0` would serialize as `null`, which a UI reads as "unknown" — a very different claim
        // from "nothing is stored yet".
        assert_eq!(basis(0, 0).bytes_per_row(), 0.0);
        assert_eq!(basis(0, 0).estimate(100), 0);
    }

    #[test]
    fn bytes_are_the_signals_mean_row_size_times_the_services_rows() {
        // 1,000 rows in 100,000 bytes = 100 bytes/row; a service with 250 rows estimates 25,000.
        let b = basis(1_000, 100_000);
        assert_eq!(b.bytes_per_row(), 100.0);
        assert_eq!(b.estimate(250), 25_000);
    }

    #[test]
    fn a_service_present_in_one_signal_only_still_gets_a_row() {
        let rows = merge_rows(
            &counts(&[("api", 100)], false),
            &counts(&[("web", 50)], false),
            &empty_counts(),
            basis(1_000, 10_000),
            basis(1_000, 100_000),
            10,
        );
        let names: Vec<&str> = rows
            .iter()
            .map(|r| r["service"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"api") && names.contains(&"web"));
    }

    #[test]
    fn a_signal_a_service_has_no_rows_in_reads_as_zero_not_missing() {
        let rows = merge_rows(
            &counts(&[("api", 100)], false),
            &empty_counts(),
            &empty_counts(),
            basis(1_000, 10_000),
            basis(1_000, 100_000),
            10,
        );
        assert_eq!(rows[0]["signals"]["traces"]["rows"], json!(0));
        assert_eq!(rows[0]["signals"]["traces"]["bytes"], json!(0));
    }

    #[test]
    fn rows_are_ordered_by_estimated_footprint_not_row_count() {
        // `web` has ten times `api`'s rows, but they are LOG rows at 10 bytes each while `api`'s
        // are span rows at 1,000. Ordering by count would put the cheap service on top.
        let rows = merge_rows(
            &counts(&[("web", 10_000)], false),
            &counts(&[("api", 1_000)], false),
            &empty_counts(),
            basis(1_000, 10_000),    // logs: 10 bytes/row
            basis(1_000, 1_000_000), // spans: 1,000 bytes/row
            10,
        );
        assert_eq!(rows[0]["service"], json!("api"));
        assert_eq!(rows[0]["bytes_total"], json!(1_000_000));
        assert_eq!(rows[1]["service"], json!("web"));
    }

    #[test]
    fn equal_footprints_order_by_name_so_a_refresh_does_not_shuffle() {
        let rows = merge_rows(
            &counts(&[("zeta", 100), ("alpha", 100)], false),
            &empty_counts(),
            &empty_counts(),
            basis(1_000, 10_000),
            basis(1_000, 10_000),
            10,
        );
        assert_eq!(rows[0]["service"], json!("alpha"));
        assert_eq!(rows[1]["service"], json!("zeta"));
    }

    #[test]
    fn the_limit_keeps_the_biggest_not_the_first_alphabetically() {
        let rows = merge_rows(
            &counts(&[("a", 1), ("b", 2), ("c", 300)], false),
            &empty_counts(),
            &empty_counts(),
            basis(1_000, 10_000),
            basis(1_000, 10_000),
            1,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["service"], json!("c"));
    }

    #[test]
    fn metrics_are_reported_as_unattributed_rather_than_zero_bytes() {
        // The distinction matters: `attributed: false` says "this signal's bytes belong to
        // somebody, just not to a service", where a zero would say nobody is using it.
        let v = signal_json(basis(5_000, 500_000), false, false);
        assert_eq!(v["attributed"], json!(false));
        assert_eq!(v["store_bytes"], json!(500_000));
        assert_eq!(v["bytes_per_row"], json!(100.0));
    }

    #[test]
    fn a_capped_facet_is_reported_on_its_signal() {
        assert_eq!(signal_json(basis(1, 1), true, true)["capped"], json!(true));
    }
}

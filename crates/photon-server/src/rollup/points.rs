//! `RollupRow` → `MetricPoint`s. The whole of the `photon.service.*` wire contract lives here.
//!
//! The plan for this task said "build OTLP protobuf … and write through the local metrics ingest
//! path, exactly as `photon-agent` does". That is the right shape for `photon-agent`, which is a
//! separate process and has no choice but to speak the wire. Inside `photon-server` it would mean
//! encoding protobuf, POSTing to ourselves over HTTP, authenticating against our own ingest token,
//! and decoding it back into the same `MetricPoint`s this module builds directly. `RumWalSink` in
//! `main.rs` already takes the direct route for RUM vitals; the rollup takes it for the same
//! reason.

use std::collections::BTreeMap;

use photon_core::metric_record::MetricPoint;
use photon_core::metric_schema;
use photon_query::{RedBasis, RollupRow, LATENCY_BOUNDS_NS, LATENCY_METRIC};

/// The two metric families this module emits, one per level. They are deliberately NOT the same
/// name distinguished by an `operation` attribute: `metric_query`'s bucket fold is `sum(value)`
/// over every series that matches, so a bare `photon.service.requests` query would silently add
/// the per-operation series to the per-service one and report a service serving roughly twice its
/// real traffic. Separate names make `photon.service.requests` mean exactly one thing.
///
/// Both are reserved: an operator's own metric under either prefix would be indistinguishable
/// from a rollup series.
pub const SERVICE_PREFIX: &str = "photon.service.";
/// The per-service request counter — the one series every rolled-up bucket emits, so "does this
/// window already hold rollup points" can be answered by asking about this name alone.
pub const REQUESTS_METRIC: &str = "photon.service.requests";
pub const OPERATION_PREFIX: &str = "photon.operation.";

/// The family a row belongs to, by level.
fn prefix(row: &RollupRow) -> &'static str {
    if row.operation.is_some() {
        OPERATION_PREFIX
    } else {
        SERVICE_PREFIX
    }
}

/// Delta temporality (`metric_query::TEMPORALITY_DELTA`, restated — that constant is crate-private
/// to `photon-query`). Each bucket's counters describe THAT bucket, not a running total, which is
/// what lets a bucket be written once and never revisited.
const TEMPORALITY_DELTA: i32 = 1;

/// The attribute keys a rollup series is identified by. `service.name` and
/// `deployment.environment` deliberately reuse the OTel-conventional (and promoted, since Task 3)
/// key names rather than the plain `service`/`environment` the plan sketched: promoted keys get
/// their own column, so the `/metrics` explorer groups them, the alert engine filters on them, and
/// the frontend's `withContext()` environment term applies to rollup series for free. A private
/// naming would have cost all three.
pub const ATTR_SERVICE: &str = "service.name";
pub const ATTR_ENVIRONMENT: &str = "deployment.environment";
pub const ATTR_OPERATION: &str = "operation";
/// The Site a mirrored service belongs to. Only set on rows Central rolled up itself; a Site's
/// own pushed rollup carries the same label from the federation egress path.
pub const ATTR_TENANT: &str = "tenant";

/// The identifying attributes for one row, in the order the schema wants them.
fn attrs(row: &RollupRow) -> BTreeMap<String, String> {
    let mut a = BTreeMap::new();
    a.insert(ATTR_SERVICE.to_string(), row.service.clone());
    if let Some(tenant) = &row.tenant {
        // Present only when Central rolled up a Site's mirrored spans itself. It is the same
        // `tenant` label a Site's own pushed rollup carries, and `services_rollup::CARRIED_KEYS`
        // already lists it, so both arrive at the read path identically.
        a.insert(ATTR_TENANT.to_string(), tenant.clone());
    }
    if let Some(env) = &row.environment {
        // Omitted rather than emitted as "" when the service never set it: an empty-string
        // environment would look like a real one in the picker.
        a.insert(ATTR_ENVIRONMENT.to_string(), env.clone());
    }
    if let Some(op) = &row.operation {
        a.insert(ATTR_OPERATION.to_string(), op.clone());
    }
    a
}

fn sum_point(
    name: &str,
    unit: &str,
    value: u64,
    row: &RollupRow,
    attrs: &BTreeMap<String, String>,
) -> MetricPoint {
    MetricPoint {
        metric_name: format!("{}{name}", prefix(row)),
        metric_type: metric_schema::metric_type::SUM,
        type_text: Some("Sum".into()),
        temporality: Some(TEMPORALITY_DELTA),
        is_monotonic: Some(true),
        unit: Some(unit.into()),
        // Both timestamps are the bucket START. OTLP would put the interval END in `timestamp`,
        // but Photon's metric read path buckets by `timestamp`, so an end-labelled point lands in
        // the FOLLOWING bucket — every rollup series would sit one minute to the right of the raw
        // spans it summarizes, and the two would never line up on a chart.
        timestamp_nanos: row.ts,
        start_timestamp_nanos: Some(row.ts),
        scope_name: Some("photon.rollup".into()),
        value: Some(value as f64),
        histogram: None,
        exp_histogram: None,
        summary: None,
        exemplars: None,
        attributes: attrs.clone(),
    }
}

/// The one latency histogram a bucket publishes, or `None` when the bucket measured nothing.
///
/// Split out of [`points_for`] because it is also published *alone*, by the ladder repair: when
/// [`photon_query::LATENCY_METRIC`] is renamed (which is what a renumbering of
/// `LATENCY_BOUNDS_NS` costs), the already-covered range holds counters that are still correct
/// and a histogram under a name nothing reads any more. Republishing the counters alongside it
/// would double every one of them; republishing this point alone is close to free, because a
/// quantile is scale-invariant — a bucket whose histogram somehow landed twice reports the same
/// p50, p90 and p99 as one that landed once.
pub fn latency_point(row: &RollupRow) -> Option<MetricPoint> {
    if row.duration_count == 0 {
        return None;
    }
    Some(MetricPoint {
        metric_name: format!("{}{LATENCY_METRIC}", prefix(row)),
        metric_type: metric_schema::metric_type::HISTOGRAM,
        type_text: Some("Histogram".into()),
        temporality: Some(TEMPORALITY_DELTA),
        is_monotonic: None,
        unit: Some("ns".into()),
        timestamp_nanos: row.ts,
        start_timestamp_nanos: Some(row.ts),
        scope_name: Some("photon.rollup".into()),
        value: None,
        histogram: Some(histogram_json(row)),
        exp_histogram: None,
        summary: None,
        exemplars: None,
        attributes: attrs(row),
    })
}

/// Every `MetricPoint` one rolled-up bucket publishes.
///
/// A bucket with no requests still emits its counters as zeros — that is what makes "this service
/// stopped serving traffic" distinguishable from "nobody rolled this bucket up", which is the
/// distinction the Attention Board's Silence-vs-Unknown rule is built on. The histogram and the
/// basis gauge are the two exceptions: an empty histogram carries no information, and the basis
/// gauge is by construction only interesting when it fires.
pub fn points_for(row: &RollupRow) -> Vec<MetricPoint> {
    let a = attrs(row);
    let mut out = vec![
        sum_point("requests", "{requests}", row.count, row, &a),
        sum_point("spans", "{spans}", row.span_count, row, &a),
        sum_point("errors", "{errors}", row.error_count, row, &a),
        sum_point("http_4xx", "{responses}", row.http_4xx, row, &a),
        sum_point("http_5xx", "{responses}", row.http_5xx, row, &a),
        sum_point("apdex.satisfied", "{requests}", row.satisfied, row, &a),
        sum_point("apdex.tolerating", "{requests}", row.tolerating, row, &a),
        sum_point("apdex.frustrated", "{requests}", row.frustrated, row, &a),
    ];

    out.extend(latency_point(row));

    // Service level only, and only when the count is NOT plain entry spans. An operation now
    // inherits its service's basis, so publishing it again per operation would be the same fact
    // repeated `top_operations` times; and emitting a `0` for every healthy service would
    // multiply the series count for something that is almost always "nothing to say".
    if row.operation.is_none() && row.basis != RedBasis::Entry {
        let mut ba = a.clone();
        ba.insert(
            "basis".into(),
            match row.basis {
                RedBasis::Entry => "entry",
                RedBasis::TraceRoot => "trace_root",
                RedBasis::AllSpans => "all_spans",
            }
            .into(),
        );
        out.push(MetricPoint {
            metric_name: format!("{SERVICE_PREFIX}basis"),
            metric_type: metric_schema::metric_type::GAUGE,
            type_text: Some("Gauge".into()),
            temporality: None,
            is_monotonic: None,
            unit: Some("1".into()),
            timestamp_nanos: row.ts,
            start_timestamp_nanos: None,
            scope_name: Some("photon.rollup".into()),
            value: Some(1.0),
            histogram: None,
            exp_histogram: None,
            summary: None,
            exemplars: None,
            attributes: ba,
        });
    }

    out
}

/// The histogram payload `photon-ingest` serializes and `photon-query`'s `HistPayload` reads.
/// Built literally, rather than through a mirrored `#[derive(Serialize)]` struct, because it is a
/// storage format each side pins independently — a struct here would look like a shared definition
/// while being nothing of the kind.
fn histogram_json(row: &RollupRow) -> String {
    let bounds: Vec<f64> = LATENCY_BOUNDS_NS.iter().map(|b| *b as f64).collect();
    serde_json::json!({
        "count": row.duration_count,
        "sum": row.duration_sum as f64,
        "bucket_counts": row.latency_buckets,
        "explicit_bounds": bounds,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> RollupRow {
        RollupRow {
            ts: 1_700_000_000_000_000_000,
            service: "api".into(),
            environment: Some("prod".into()),
            tenant: None,
            operation: None,
            count: 10,
            span_count: 31,
            basis: RedBasis::Entry,
            error_count: 2,
            http_4xx: 1,
            http_5xx: 2,
            satisfied: 7,
            tolerating: 2,
            frustrated: 1,
            latency_buckets: {
                let mut v = vec![0; LATENCY_BOUNDS_NS.len() + 1];
                v[3] = 7;
                v[8] = 2;
                v[LATENCY_BOUNDS_NS.len()] = 1;
                v
            },
            duration_sum: 12_345_678,
            duration_count: 10,
        }
    }

    fn named<'a>(points: &'a [MetricPoint], name: &str) -> Option<&'a MetricPoint> {
        points
            .iter()
            .find(|p| p.metric_name == format!("{SERVICE_PREFIX}{name}"))
    }

    #[test]
    fn every_counter_is_a_delta_sum_stamped_at_the_bucket_start() {
        let r = row();
        let points = points_for(&r);
        for name in [
            "requests",
            "spans",
            "errors",
            "http_4xx",
            "http_5xx",
            "apdex.satisfied",
            "apdex.tolerating",
            "apdex.frustrated",
        ] {
            let p = named(&points, name).unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(p.metric_type, metric_schema::metric_type::SUM, "{name}");
            assert_eq!(p.temporality, Some(TEMPORALITY_DELTA), "{name}");
            assert_eq!(p.is_monotonic, Some(true), "{name}");
            assert_eq!(p.timestamp_nanos, r.ts, "{name}");
            assert_eq!(p.start_timestamp_nanos, Some(r.ts), "{name}");
        }
        assert_eq!(named(&points, "requests").unwrap().value, Some(10.0));
        assert_eq!(
            named(&points, "spans").unwrap().value,
            Some(31.0),
            "the honest second number rides along, it is not folded into requests"
        );
    }

    #[test]
    fn a_zero_traffic_bucket_still_publishes_its_counters() {
        let mut r = row();
        r.count = 0;
        r.span_count = 0;
        r.error_count = 0;
        r.satisfied = 0;
        r.tolerating = 0;
        r.frustrated = 0;
        r.http_4xx = 0;
        r.http_5xx = 0;
        r.duration_count = 0;
        let points = points_for(&r);

        assert_eq!(named(&points, "requests").unwrap().value, Some(0.0));
        assert!(
            named(&points, LATENCY_METRIC).is_none(),
            "an empty histogram carries no information"
        );
    }

    #[test]
    fn the_histogram_matches_the_bounds_the_query_bucketed_by() {
        let r = row();
        let points = points_for(&r);
        let h = named(&points, LATENCY_METRIC).unwrap();
        assert_eq!(h.metric_type, metric_schema::metric_type::HISTOGRAM);
        assert_eq!(h.unit.as_deref(), Some("ns"));

        let json: serde_json::Value = serde_json::from_str(h.histogram.as_ref().unwrap()).unwrap();
        assert_eq!(json["count"], 10);
        assert_eq!(json["sum"], 12_345_678.0);
        assert_eq!(
            json["explicit_bounds"].as_array().unwrap().len(),
            LATENCY_BOUNDS_NS.len()
        );
        assert_eq!(
            json["bucket_counts"].as_array().unwrap().len(),
            LATENCY_BOUNDS_NS.len() + 1,
            "one more count than bound — the +Inf overflow bucket"
        );
        let total: u64 = json["bucket_counts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .sum();
        assert_eq!(total, 10, "buckets must sum to the histogram's own count");
    }

    #[test]
    fn the_basis_gauge_fires_only_for_a_service_that_is_not_entry_based() {
        assert!(named(&points_for(&row()), "basis").is_none());

        let mut r = row();
        r.basis = RedBasis::AllSpans;
        let points = points_for(&r);
        let g = named(&points, "basis").unwrap();
        assert_eq!(g.metric_type, metric_schema::metric_type::GAUGE);
        assert_eq!(g.value, Some(1.0));
        assert_eq!(
            g.attributes.get("basis").map(String::as_str),
            Some("all_spans")
        );
    }

    #[test]
    fn the_two_levels_never_share_a_metric_name() {
        let mut op = row();
        op.operation = Some("GET /x".into());

        let names = |r: &RollupRow| -> Vec<String> {
            points_for(r)
                .iter()
                .map(|p| p.metric_name.clone())
                .collect()
        };
        let service_names = names(&row());
        let op_names = names(&op);

        assert!(service_names.iter().all(|n| n.starts_with(SERVICE_PREFIX)));
        assert!(op_names.iter().all(|n| n.starts_with(OPERATION_PREFIX)));
        assert!(
            !op_names.iter().any(|n| service_names.contains(n)),
            "a shared name would make a bare `photon.service.requests` query sum both levels"
        );
    }

    #[test]
    fn the_basis_gauge_is_a_service_level_fact_only() {
        let mut op = row();
        op.operation = Some("GET /x".into());
        op.basis = RedBasis::AllSpans;
        assert!(
            !points_for(&op)
                .iter()
                .any(|p| p.metric_name.ends_with("basis")),
            "an operation inherits its service's basis — republishing it says nothing new"
        );
    }

    #[test]
    fn identity_uses_the_promoted_otel_keys_and_omits_a_missing_environment() {
        let a = attrs(&row());
        assert_eq!(a.get(ATTR_SERVICE).map(String::as_str), Some("api"));
        assert_eq!(a.get(ATTR_ENVIRONMENT).map(String::as_str), Some("prod"));
        assert!(
            !a.contains_key(ATTR_OPERATION),
            "service level carries no operation"
        );

        let mut r = row();
        r.environment = None;
        r.operation = Some("GET /x".into());
        let a = attrs(&r);
        assert!(
            !a.contains_key(ATTR_ENVIRONMENT),
            "absent, not empty — an empty string would look like a real environment"
        );
        assert_eq!(a.get(ATTR_OPERATION).map(String::as_str), Some("GET /x"));
    }
}

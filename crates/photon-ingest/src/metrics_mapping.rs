//! OTLP metrics → `MetricPoint` mapping. Mirrors `mapping.rs`/`trace_mapping.rs`: walk
//! resource → scope → metric → data points, merge resource+point attributes into a flat map,
//! and serialize distribution payloads (histogram/exp-histogram/summary) + exemplars to JSON
//! string columns (the codebase has no Arrow List/Struct columns). Scope attributes are
//! excluded, matching logs/traces.
use std::collections::BTreeMap;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::{
    exemplar, metric::Data, number_data_point, Exemplar, ExponentialHistogramDataPoint,
    HistogramDataPoint, Metric, NumberDataPoint, SummaryDataPoint,
};
use photon_core::metric_record::{MetricBatchBuilder, MetricFixed, MetricPoint};
use photon_core::metric_schema::metric_type;
use serde::Serialize;

use crate::otlp_value::{any_value_into_string, bytes_to_hex_opt};

/// Number of data points a single metric's `Data` payload carries, regardless of type. Used
/// both to pre-size the output `Vec` (F8) and to find the last data point in a resource group
/// so its resource-attribute map can be moved rather than cloned (F9a).
pub(crate) fn data_point_count(data: &Data) -> usize {
    match data {
        Data::Gauge(g) => g.data_points.len(),
        Data::Sum(s) => s.data_points.len(),
        Data::Histogram(h) => h.data_points.len(),
        Data::ExponentialHistogram(h) => h.data_points.len(),
        Data::Summary(s) => s.data_points.len(),
    }
}

pub fn otlp_metrics_to_points(req: ExportMetricsServiceRequest) -> Vec<MetricPoint> {
    let total: usize = req
        .resource_metrics
        .iter()
        .flat_map(|rm| &rm.scope_metrics)
        .flat_map(|sm| &sm.metrics)
        .map(|metric| metric.data.as_ref().map(data_point_count).unwrap_or(0))
        .sum();
    let mut out: Vec<MetricPoint> = Vec::with_capacity(total);

    for rm in req.resource_metrics {
        let mut resource_attrs: BTreeMap<String, String> = rm
            .resource
            .map(|r| {
                r.attributes
                    .into_iter()
                    .map(|kv| {
                        (
                            kv.key,
                            kv.value.map(any_value_into_string).unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Total data points this resource group will emit, so the resource-attr map can be
        // moved (not cloned) into the very last one — every earlier point still needs its own
        // copy. The "last point" spans across all scopes/metrics/data-point-lists under this
        // resource, so track it with a flat counter rather than per-loop-level state.
        let points_in_resource: usize = rm
            .scope_metrics
            .iter()
            .flat_map(|sm| &sm.metrics)
            .map(|metric| metric.data.as_ref().map(data_point_count).unwrap_or(0))
            .sum();
        let mut emitted = 0usize;

        for sm in rm.scope_metrics {
            let scope_name = sm
                .scope
                .and_then(|s| (!s.name.is_empty()).then_some(s.name));

            for metric in sm.metrics {
                let Metric {
                    name, unit, data, ..
                } = metric;
                let unit = (!unit.is_empty()).then_some(unit);
                let Some(data) = data else { continue };

                match data {
                    Data::Gauge(g) => {
                        for dp in g.data_points {
                            emitted += 1;
                            let attrs = if emitted == points_in_resource {
                                std::mem::take(&mut resource_attrs)
                            } else {
                                resource_attrs.clone()
                            };
                            out.push(number_point(
                                &name,
                                &unit,
                                &scope_name,
                                attrs,
                                metric_type::GAUGE,
                                None,
                                None,
                                dp,
                            ));
                        }
                    }
                    Data::Sum(s) => {
                        for dp in s.data_points {
                            emitted += 1;
                            let attrs = if emitted == points_in_resource {
                                std::mem::take(&mut resource_attrs)
                            } else {
                                resource_attrs.clone()
                            };
                            out.push(number_point(
                                &name,
                                &unit,
                                &scope_name,
                                attrs,
                                metric_type::SUM,
                                Some(s.aggregation_temporality),
                                Some(s.is_monotonic),
                                dp,
                            ));
                        }
                    }
                    Data::Histogram(h) => {
                        for dp in h.data_points {
                            emitted += 1;
                            let attrs = if emitted == points_in_resource {
                                std::mem::take(&mut resource_attrs)
                            } else {
                                resource_attrs.clone()
                            };
                            out.push(histogram_point(
                                &name,
                                &unit,
                                &scope_name,
                                attrs,
                                h.aggregation_temporality,
                                dp,
                            ));
                        }
                    }
                    Data::ExponentialHistogram(h) => {
                        for dp in h.data_points {
                            emitted += 1;
                            let attrs = if emitted == points_in_resource {
                                std::mem::take(&mut resource_attrs)
                            } else {
                                resource_attrs.clone()
                            };
                            out.push(exp_histogram_point(
                                &name,
                                &unit,
                                &scope_name,
                                attrs,
                                h.aggregation_temporality,
                                dp,
                            ));
                        }
                    }
                    Data::Summary(s) => {
                        for dp in s.data_points {
                            emitted += 1;
                            let attrs = if emitted == points_in_resource {
                                std::mem::take(&mut resource_attrs)
                            } else {
                                resource_attrs.clone()
                            };
                            out.push(summary_point(&name, &unit, &scope_name, attrs, dp));
                        }
                    }
                }
            }
        }
    }
    out
}

/// Total data-point count across every resource/scope/metric group and all 5 data-point-type
/// lists, used to pre-size the `MetricBatchBuilder` on the streaming ingest path so its column
/// builders don't pay for geometric reallocation. Same summation `otlp_metrics_to_points` uses
/// to size its output `Vec`; reuses `data_point_count`.
pub(crate) fn estimate_rows(req: &ExportMetricsServiceRequest) -> usize {
    req.resource_metrics
        .iter()
        .flat_map(|rm| &rm.scope_metrics)
        .flat_map(|sm| &sm.metrics)
        .map(|metric| metric.data.as_ref().map(data_point_count).unwrap_or(0))
        .sum()
}

/// Stream an OTLP metrics request straight into the Arrow metric builder — the hot ingest path.
/// No intermediate `Vec<MetricPoint>` and no per-point `BTreeMap`: for each resource group the
/// resource attributes are owned once, then chained (as borrowed pairs) with each point's own
/// attributes and appended directly. Produces the SAME batch as `otlp_metrics_to_points` +
/// `append`, proven byte-identical in tests. Fans out across all 5 data-point types, applying
/// the IDENTICAL untrusted-nanos clamping (`nz` / `i64::try_from`) and building the IDENTICAL
/// distribution/exemplar JSON payloads (same serde structs) as the reference `*_point`
/// constructors.
pub fn otlp_metrics_into_builder(
    req: ExportMetricsServiceRequest,
    builder: &mut MetricBatchBuilder,
) {
    for rm in req.resource_metrics {
        // Own the resource attrs once per group (OTLP moves the strings out); each point below
        // re-borrows them via the merged-borrow trick — no per-point clone, no per-point BTreeMap.
        let resource_attrs: Vec<(String, String)> = rm
            .resource
            .map(|r| kvs_to_pairs(r.attributes))
            .unwrap_or_default();

        for sm in rm.scope_metrics {
            let scope_name = sm
                .scope
                .and_then(|s| (!s.name.is_empty()).then_some(s.name));

            for metric in sm.metrics {
                let Metric {
                    name, unit, data, ..
                } = metric;
                let unit = (!unit.is_empty()).then_some(unit);
                let Some(data) = data else { continue };

                match data {
                    Data::Gauge(g) => {
                        for dp in g.data_points {
                            append_number(
                                builder,
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                metric_type::GAUGE,
                                None,
                                None,
                                dp,
                            );
                        }
                    }
                    Data::Sum(s) => {
                        for dp in s.data_points {
                            append_number(
                                builder,
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                metric_type::SUM,
                                Some(s.aggregation_temporality),
                                Some(s.is_monotonic),
                                dp,
                            );
                        }
                    }
                    Data::Histogram(h) => {
                        for dp in h.data_points {
                            append_histogram(
                                builder,
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                h.aggregation_temporality,
                                dp,
                            );
                        }
                    }
                    Data::ExponentialHistogram(h) => {
                        for dp in h.data_points {
                            append_exp_histogram(
                                builder,
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                h.aggregation_temporality,
                                dp,
                            );
                        }
                    }
                    Data::Summary(s) => {
                        for dp in s.data_points {
                            append_summary(builder, &name, &unit, &scope_name, &resource_attrs, dp);
                        }
                    }
                }
            }
        }
    }
}

/// Convert an OTLP `KeyValue` list into owned `(key, value)` pairs — the same conversion the
/// reference path applies when building its attribute `BTreeMap`s (value → `any_value_into_string`,
/// missing value → empty string). Preserves duplicate keys and OTLP order; the merged-borrow
/// dedup in `append_metric_point` reproduces `BTreeMap` collapse (last wins) at append time.
fn kvs_to_pairs(
    kvs: Vec<opentelemetry_proto::tonic::common::v1::KeyValue>,
) -> Vec<(String, String)> {
    kvs.into_iter()
        .map(|kv| {
            (
                kv.key,
                kv.value.map(any_value_into_string).unwrap_or_default(),
            )
        })
        .collect()
}

/// Merge resource + point attrs and append one streaming row. Reproduces `BTreeMap` iteration
/// semantics without a per-point `BTreeMap` (identical trick to `otlp_traces_into_builder`): tag
/// the merged (resource-then-point) pairs with their insertion index, sort by (key asc, index
/// desc), then `dedup_by` key. `dedup_by` keeps the FIRST of each equal-key run; with index
/// descending that first is the highest index = the point's value — so duplicates collapse to
/// one entry, POINT beats resource on a collision, and keys come out ascending. No String
/// clones: `merged` borrows from the already-owned `resource_attrs`/`point_attrs` slices.
fn append_metric_point(
    builder: &mut MetricBatchBuilder,
    fixed: MetricFixed<'_>,
    resource_attrs: &[(String, String)],
    point_attrs: &[(String, String)],
) {
    let mut merged: Vec<(usize, &str, &str)> = resource_attrs
        .iter()
        .chain(point_attrs.iter())
        .enumerate()
        .map(|(i, (k, v))| (i, k.as_str(), v.as_str()))
        .collect();
    merged.sort_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(&a.0)));
    merged.dedup_by(|a, b| a.1 == b.1);
    builder.append_streaming(fixed, merged.iter().map(|&(_, k, v)| (k, v)));
}

/// Streaming counterpart of `number_point`: build the same `MetricFixed` columns straight from a
/// borrowed `NumberDataPoint` (used for both Gauge and Sum) and append. Same value/timestamp
/// semantics as the reference (`nz`, `i64::try_from` clamp, int→f64).
#[allow(clippy::too_many_arguments)]
fn append_number(
    builder: &mut MetricBatchBuilder,
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    resource_attrs: &[(String, String)],
    mtype: i32,
    temporality: Option<i32>,
    is_monotonic: Option<bool>,
    dp: NumberDataPoint,
) {
    let value = dp.value.map(|v| match v {
        number_data_point::Value::AsDouble(d) => d,
        number_data_point::Value::AsInt(i) => i as f64,
    });
    let timestamp_nanos = i64::try_from(dp.time_unix_nano).unwrap_or(i64::MAX);
    let start_timestamp_nanos = nz(dp.start_time_unix_nano);
    let exemplars = exemplars_json(dp.exemplars);
    let point_attrs = kvs_to_pairs(dp.attributes);
    let fixed = MetricFixed {
        metric_name: name,
        metric_type: mtype,
        type_text: Some(type_text(mtype)),
        temporality,
        is_monotonic,
        unit: unit.as_deref(),
        timestamp_nanos,
        start_timestamp_nanos,
        scope_name: scope.as_deref(),
        value,
        exemplars: exemplars.as_deref(),
        ..MetricFixed::default()
    };
    append_metric_point(builder, fixed, resource_attrs, &point_attrs);
}

/// Streaming counterpart of `histogram_point`. Serializes the SAME `HistogramJson` struct the
/// reference builds into a local `String` and lends it to `MetricFixed`.
fn append_histogram(
    builder: &mut MetricBatchBuilder,
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    resource_attrs: &[(String, String)],
    temporality: i32,
    mut dp: HistogramDataPoint,
) {
    let histogram = histogram_json(&mut dp);
    let timestamp_nanos = i64::try_from(dp.time_unix_nano).unwrap_or(i64::MAX);
    let start_timestamp_nanos = nz(dp.start_time_unix_nano);
    let exemplars = exemplars_json(dp.exemplars);
    let point_attrs = kvs_to_pairs(dp.attributes);
    let fixed = MetricFixed {
        metric_name: name,
        metric_type: metric_type::HISTOGRAM,
        type_text: Some("HISTOGRAM"),
        temporality: Some(temporality),
        unit: unit.as_deref(),
        timestamp_nanos,
        start_timestamp_nanos,
        scope_name: scope.as_deref(),
        histogram: histogram.as_deref(),
        exemplars: exemplars.as_deref(),
        ..MetricFixed::default()
    };
    append_metric_point(builder, fixed, resource_attrs, &point_attrs);
}

/// Streaming counterpart of `exp_histogram_point`. Serializes the SAME `ExpHistogramJson` struct.
fn append_exp_histogram(
    builder: &mut MetricBatchBuilder,
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    resource_attrs: &[(String, String)],
    temporality: i32,
    mut dp: ExponentialHistogramDataPoint,
) {
    let exp_histogram = exp_histogram_json(&mut dp);
    let timestamp_nanos = i64::try_from(dp.time_unix_nano).unwrap_or(i64::MAX);
    let start_timestamp_nanos = nz(dp.start_time_unix_nano);
    let exemplars = exemplars_json(dp.exemplars);
    let point_attrs = kvs_to_pairs(dp.attributes);
    let fixed = MetricFixed {
        metric_name: name,
        metric_type: metric_type::EXP_HISTOGRAM,
        type_text: Some("EXPONENTIAL_HISTOGRAM"),
        temporality: Some(temporality),
        unit: unit.as_deref(),
        timestamp_nanos,
        start_timestamp_nanos,
        scope_name: scope.as_deref(),
        exp_histogram: exp_histogram.as_deref(),
        exemplars: exemplars.as_deref(),
        ..MetricFixed::default()
    };
    append_metric_point(builder, fixed, resource_attrs, &point_attrs);
}

/// Streaming counterpart of `summary_point`. Serializes the SAME `SummaryJson` struct. Summary
/// data points carry no exemplars, matching the reference (which leaves that column null).
fn append_summary(
    builder: &mut MetricBatchBuilder,
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    resource_attrs: &[(String, String)],
    mut dp: SummaryDataPoint,
) {
    let summary = summary_json(&mut dp);
    let timestamp_nanos = i64::try_from(dp.time_unix_nano).unwrap_or(i64::MAX);
    let start_timestamp_nanos = nz(dp.start_time_unix_nano);
    let point_attrs = kvs_to_pairs(dp.attributes);
    let fixed = MetricFixed {
        metric_name: name,
        metric_type: metric_type::SUMMARY,
        type_text: Some("SUMMARY"),
        unit: unit.as_deref(),
        timestamp_nanos,
        start_timestamp_nanos,
        scope_name: scope.as_deref(),
        summary: summary.as_deref(),
        ..MetricFixed::default()
    };
    append_metric_point(builder, fixed, resource_attrs, &point_attrs);
}

/// Serialize a `HistogramDataPoint`'s distribution into the SAME `HistogramJson` payload the
/// reference `histogram_point` builds. Takes `&mut dp` so the bucket/bound Vecs can be moved out
/// (`mem::take`) rather than cloned — identical to the reference's F9b optimization — while the
/// caller keeps `dp` for its exemplars/attributes.
fn histogram_json(dp: &mut HistogramDataPoint) -> Option<String> {
    let bucket_counts = std::mem::take(&mut dp.bucket_counts);
    let explicit_bounds = std::mem::take(&mut dp.explicit_bounds);
    let payload = HistogramJson {
        count: dp.count,
        sum: dp.sum,
        bucket_counts,
        explicit_bounds,
        min: dp.min,
        max: dp.max,
    };
    serde_json::to_string(&payload).ok()
}

/// Serialize an `ExponentialHistogramDataPoint` into the SAME `ExpHistogramJson` payload the
/// reference `exp_histogram_point` builds (`positive`/`negative` bucket sub-objects included).
fn exp_histogram_json(dp: &mut ExponentialHistogramDataPoint) -> Option<String> {
    let payload = ExpHistogramJson {
        count: dp.count,
        sum: dp.sum,
        scale: dp.scale,
        zero_count: dp.zero_count,
        positive: dp.positive.take().map(|b| BucketsJson {
            offset: b.offset,
            bucket_counts: b.bucket_counts,
        }),
        negative: dp.negative.take().map(|b| BucketsJson {
            offset: b.offset,
            bucket_counts: b.bucket_counts,
        }),
        min: dp.min,
        max: dp.max,
    };
    serde_json::to_string(&payload).ok()
}

/// Serialize a `SummaryDataPoint` into the SAME `SummaryJson` payload the reference
/// `summary_point` builds.
fn summary_json(dp: &mut SummaryDataPoint) -> Option<String> {
    let payload = SummaryJson {
        count: dp.count,
        sum: dp.sum,
        quantiles: std::mem::take(&mut dp.quantile_values)
            .into_iter()
            .map(|q| QuantileJson {
                quantile: q.quantile,
                value: q.value,
            })
            .collect(),
    };
    serde_json::to_string(&payload).ok()
}

/// Merge resource attrs (base) with this point's attributes (override on key collision).
///
/// Takes `base` by value: the caller has already decided whether this data point needed a
/// clone of the resource-attr map or could take ownership of it outright (it was the last
/// point in the resource group), so this function never clones — it just extends in place.
fn merge_attrs(
    mut base: BTreeMap<String, String>,
    point_attrs: Vec<opentelemetry_proto::tonic::common::v1::KeyValue>,
) -> BTreeMap<String, String> {
    for kv in point_attrs {
        base.insert(
            kv.key,
            kv.value.map(any_value_into_string).unwrap_or_default(),
        );
    }
    base
}

fn nz(nanos: u64) -> Option<i64> {
    // Untrusted OTLP u64 nanos: clamp instead of `as i64`, which wraps negative for values
    // above i64::MAX.
    (nanos != 0).then_some(i64::try_from(nanos).unwrap_or(i64::MAX))
}

#[allow(clippy::too_many_arguments)]
fn number_point(
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    attrs: BTreeMap<String, String>,
    mtype: i32,
    temporality: Option<i32>,
    is_monotonic: Option<bool>,
    dp: NumberDataPoint,
) -> MetricPoint {
    let value = dp.value.map(|v| match v {
        number_data_point::Value::AsDouble(d) => d,
        number_data_point::Value::AsInt(i) => i as f64,
    });
    MetricPoint {
        metric_name: name.to_string(),
        metric_type: mtype,
        type_text: Some(type_text(mtype).to_string()),
        temporality,
        is_monotonic,
        unit: unit.clone(),
        timestamp_nanos: i64::try_from(dp.time_unix_nano).unwrap_or(i64::MAX),
        start_timestamp_nanos: nz(dp.start_time_unix_nano),
        scope_name: scope.clone(),
        value,
        exemplars: exemplars_json(dp.exemplars),
        attributes: merge_attrs(attrs, dp.attributes),
        ..Default::default()
    }
}

fn histogram_point(
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    attrs: BTreeMap<String, String>,
    temporality: i32,
    mut dp: HistogramDataPoint,
) -> MetricPoint {
    // `dp` is owned by this function, so move its Vecs into the JSON payload instead of
    // cloning them (F9b) — `mem::take` leaves empty Vecs behind in `dp`, which is fine since
    // nothing reads `dp.bucket_counts`/`dp.explicit_bounds` again below.
    let bucket_counts = std::mem::take(&mut dp.bucket_counts);
    let explicit_bounds = std::mem::take(&mut dp.explicit_bounds);
    let payload = HistogramJson {
        count: dp.count,
        sum: dp.sum,
        bucket_counts,
        explicit_bounds,
        min: dp.min,
        max: dp.max,
    };
    MetricPoint {
        metric_name: name.to_string(),
        metric_type: metric_type::HISTOGRAM,
        type_text: Some("HISTOGRAM".into()),
        temporality: Some(temporality),
        unit: unit.clone(),
        timestamp_nanos: i64::try_from(dp.time_unix_nano).unwrap_or(i64::MAX),
        start_timestamp_nanos: nz(dp.start_time_unix_nano),
        scope_name: scope.clone(),
        histogram: serde_json::to_string(&payload).ok(),
        exemplars: exemplars_json(dp.exemplars),
        attributes: merge_attrs(attrs, dp.attributes),
        ..Default::default()
    }
}

fn exp_histogram_point(
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    attrs: BTreeMap<String, String>,
    temporality: i32,
    dp: ExponentialHistogramDataPoint,
) -> MetricPoint {
    let payload = ExpHistogramJson {
        count: dp.count,
        sum: dp.sum,
        scale: dp.scale,
        zero_count: dp.zero_count,
        positive: dp.positive.map(|b| BucketsJson {
            offset: b.offset,
            bucket_counts: b.bucket_counts,
        }),
        negative: dp.negative.map(|b| BucketsJson {
            offset: b.offset,
            bucket_counts: b.bucket_counts,
        }),
        min: dp.min,
        max: dp.max,
    };
    MetricPoint {
        metric_name: name.to_string(),
        metric_type: metric_type::EXP_HISTOGRAM,
        type_text: Some("EXPONENTIAL_HISTOGRAM".into()),
        temporality: Some(temporality),
        unit: unit.clone(),
        timestamp_nanos: i64::try_from(dp.time_unix_nano).unwrap_or(i64::MAX),
        start_timestamp_nanos: nz(dp.start_time_unix_nano),
        scope_name: scope.clone(),
        exp_histogram: serde_json::to_string(&payload).ok(),
        exemplars: exemplars_json(dp.exemplars),
        attributes: merge_attrs(attrs, dp.attributes),
        ..Default::default()
    }
}

fn summary_point(
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    attrs: BTreeMap<String, String>,
    dp: SummaryDataPoint,
) -> MetricPoint {
    let payload = SummaryJson {
        count: dp.count,
        sum: dp.sum,
        quantiles: dp
            .quantile_values
            .into_iter()
            .map(|q| QuantileJson {
                quantile: q.quantile,
                value: q.value,
            })
            .collect(),
    };
    MetricPoint {
        metric_name: name.to_string(),
        metric_type: metric_type::SUMMARY,
        type_text: Some("SUMMARY".into()),
        unit: unit.clone(),
        timestamp_nanos: i64::try_from(dp.time_unix_nano).unwrap_or(i64::MAX),
        start_timestamp_nanos: nz(dp.start_time_unix_nano),
        scope_name: scope.clone(),
        summary: serde_json::to_string(&payload).ok(),
        attributes: merge_attrs(attrs, dp.attributes),
        ..Default::default()
    }
}

fn type_text(mtype: i32) -> &'static str {
    match mtype {
        metric_type::GAUGE => "GAUGE",
        metric_type::SUM => "SUM",
        metric_type::HISTOGRAM => "HISTOGRAM",
        metric_type::EXP_HISTOGRAM => "EXPONENTIAL_HISTOGRAM",
        metric_type::SUMMARY => "SUMMARY",
        _ => "UNKNOWN",
    }
}

fn exemplars_json(exemplars: Vec<Exemplar>) -> Option<String> {
    if exemplars.is_empty() {
        return None;
    }
    let items: Vec<ExemplarJson> = exemplars
        .into_iter()
        .map(|e| ExemplarJson {
            value: e.value.map(|v| match v {
                exemplar::Value::AsDouble(d) => d,
                exemplar::Value::AsInt(i) => i as f64,
            }),
            timestamp_nanos: e.time_unix_nano.to_string(),
            trace_id: bytes_to_hex_opt(&e.trace_id),
            span_id: bytes_to_hex_opt(&e.span_id),
            filtered_attributes: e
                .filtered_attributes
                .into_iter()
                .map(|kv| {
                    (
                        kv.key,
                        kv.value.map(any_value_into_string).unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect();
    serde_json::to_string(&items).ok()
}

#[derive(Serialize)]
struct HistogramJson {
    count: u64,
    sum: Option<f64>,
    bucket_counts: Vec<u64>,
    explicit_bounds: Vec<f64>,
    min: Option<f64>,
    max: Option<f64>,
}
#[derive(Serialize)]
struct ExpHistogramJson {
    count: u64,
    sum: Option<f64>,
    scale: i32,
    zero_count: u64,
    positive: Option<BucketsJson>,
    negative: Option<BucketsJson>,
    min: Option<f64>,
    max: Option<f64>,
}
#[derive(Serialize)]
struct BucketsJson {
    offset: i32,
    bucket_counts: Vec<u64>,
}
#[derive(Serialize)]
struct SummaryJson {
    count: u64,
    sum: f64,
    quantiles: Vec<QuantileJson>,
}
#[derive(Serialize)]
struct QuantileJson {
    quantile: f64,
    value: f64,
}
#[derive(Serialize)]
struct ExemplarJson {
    value: Option<f64>,
    timestamp_nanos: String,
    trace_id: Option<String>,
    span_id: Option<String>,
    filtered_attributes: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{
        any_value::Value, AnyValue, InstrumentationScope, KeyValue,
    };
    use opentelemetry_proto::tonic::metrics::v1::{
        exponential_histogram_data_point::Buckets, metric::Data,
        number_data_point::Value as NumVal, summary_data_point::ValueAtQuantile,
        AggregationTemporality, ExponentialHistogram, Gauge, Histogram, HistogramDataPoint, Metric,
        NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum, Summary,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use photon_core::metric_schema::metric_type;

    fn kv(k: &str, v: &str) -> KeyValue {
        KeyValue {
            key: k.into(),
            value: Some(AnyValue {
                value: Some(Value::StringValue(v.into())),
            }),
        }
    }
    fn req(metrics: Vec<Metric>) -> ExportMetricsServiceRequest {
        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![kv("service.name", "checkout")],
                    dropped_attributes_count: 0,
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics,
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
    }

    #[test]
    fn maps_a_gauge_point() {
        let m = Metric {
            name: "cpu.usage".into(),
            description: String::new(),
            unit: "1".into(),
            metadata: vec![],
            data: Some(Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: vec![kv("core", "0")],
                    start_time_unix_nano: 0,
                    time_unix_nano: 1_700_000_000_000_000_000,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(NumVal::AsDouble(0.73)),
                }],
            })),
        };
        let pts = otlp_metrics_to_points(req(vec![m]));
        assert_eq!(pts.len(), 1);
        let p = &pts[0];
        assert_eq!(p.metric_name, "cpu.usage");
        assert_eq!(p.metric_type, metric_type::GAUGE);
        assert_eq!(p.timestamp_nanos, 1_700_000_000_000_000_000);
        assert_eq!(p.value, Some(0.73));
        assert_eq!(
            p.attributes.get("service.name").map(String::as_str),
            Some("checkout")
        );
        assert_eq!(p.attributes.get("core").map(String::as_str), Some("0"));
    }

    /// F11: OTLP nanos (`time_unix_nano` / `start_time_unix_nano`) are an untrusted u64 from
    /// the exporter. `as i64` wraps negative for values above `i64::MAX` — the hardened casts
    /// (in `nz` and every `*_point` constructor) must clamp to `i64::MAX` instead.
    #[test]
    fn huge_nanos_clamp_to_i64_max_instead_of_wrapping_negative() {
        assert_eq!(nz(u64::MAX), Some(i64::MAX));
        assert_eq!(nz(0), None);

        let m = Metric {
            name: "cpu.usage".into(),
            description: String::new(),
            unit: "1".into(),
            metadata: vec![],
            data: Some(Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: vec![],
                    start_time_unix_nano: u64::MAX,
                    time_unix_nano: u64::MAX,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(NumVal::AsDouble(1.0)),
                }],
            })),
        };
        let p = &otlp_metrics_to_points(req(vec![m]))[0];
        assert_eq!(p.timestamp_nanos, i64::MAX);
        assert_eq!(p.start_timestamp_nanos, Some(i64::MAX));
    }

    #[test]
    fn maps_a_cumulative_monotonic_sum() {
        let m = Metric {
            name: "http.requests".into(),
            description: String::new(),
            unit: "1".into(),
            metadata: vec![],
            data: Some(Data::Sum(Sum {
                aggregation_temporality: AggregationTemporality::Cumulative as i32,
                is_monotonic: true,
                data_points: vec![NumberDataPoint {
                    attributes: vec![],
                    start_time_unix_nano: 5,
                    time_unix_nano: 10,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(NumVal::AsInt(42)),
                }],
            })),
        };
        let p = &otlp_metrics_to_points(req(vec![m]))[0];
        assert_eq!(p.metric_type, metric_type::SUM);
        assert_eq!(
            p.temporality,
            Some(AggregationTemporality::Cumulative as i32)
        );
        assert_eq!(p.is_monotonic, Some(true));
        assert_eq!(p.value, Some(42.0));
        assert_eq!(p.start_timestamp_nanos, Some(5));
    }

    #[test]
    fn maps_a_histogram_to_json_payload() {
        let m = Metric {
            name: "http.duration".into(),
            description: String::new(),
            unit: "ms".into(),
            metadata: vec![],
            data: Some(Data::Histogram(Histogram {
                aggregation_temporality: AggregationTemporality::Delta as i32,
                data_points: vec![HistogramDataPoint {
                    attributes: vec![],
                    start_time_unix_nano: 0,
                    time_unix_nano: 20,
                    count: 3,
                    sum: Some(30.0),
                    bucket_counts: vec![1, 2],
                    explicit_bounds: vec![10.0],
                    exemplars: vec![],
                    flags: 0,
                    min: Some(4.0),
                    max: Some(18.0),
                }],
            })),
        };
        let p = &otlp_metrics_to_points(req(vec![m]))[0];
        assert_eq!(p.metric_type, metric_type::HISTOGRAM);
        let json = p.histogram.as_ref().expect("histogram JSON present");
        assert!(json.contains("\"bucket_counts\":[1,2]"));
        assert!(json.contains("\"explicit_bounds\":[10.0]"));
        assert!(json.contains("\"count\":3"));
    }

    #[test]
    fn empty_request_yields_no_points() {
        assert!(otlp_metrics_to_points(ExportMetricsServiceRequest {
            resource_metrics: vec![]
        })
        .is_empty());
    }

    /// Locks the F9a `mem::take`-on-last-point optimization: within a resource group, EVERY
    /// data point must still see the resource attrs, including the very last one (which is
    /// served via `mem::take` instead of `.clone()`) and points that cross a scope/metric
    /// boundary before reaching that last point. Also checks two resources in the same request
    /// don't leak attrs into each other.
    #[test]
    fn resource_attrs_reach_every_point_across_scopes_including_the_last() {
        let resource_a = ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "checkout"), kv("region", "us")],
                dropped_attributes_count: 0,
            }),
            scope_metrics: vec![
                // Scope 1: one Gauge metric with two data points.
                ScopeMetrics {
                    scope: None,
                    metrics: vec![Metric {
                        name: "m1".into(),
                        description: String::new(),
                        unit: String::new(),
                        metadata: vec![],
                        data: Some(Data::Gauge(Gauge {
                            data_points: vec![
                                NumberDataPoint {
                                    attributes: vec![kv("core", "0")],
                                    start_time_unix_nano: 0,
                                    time_unix_nano: 1,
                                    exemplars: vec![],
                                    flags: 0,
                                    value: Some(NumVal::AsDouble(1.0)),
                                },
                                NumberDataPoint {
                                    attributes: vec![kv("core", "1")],
                                    start_time_unix_nano: 0,
                                    time_unix_nano: 2,
                                    exemplars: vec![],
                                    flags: 0,
                                    value: Some(NumVal::AsDouble(2.0)),
                                },
                            ],
                        })),
                    }],
                    schema_url: String::new(),
                },
                // Scope 2: one Sum metric with a single data point — this is the LAST data
                // point in resource_a's group, so it's the one served via `mem::take`.
                ScopeMetrics {
                    scope: None,
                    metrics: vec![Metric {
                        name: "m2".into(),
                        description: String::new(),
                        unit: String::new(),
                        metadata: vec![],
                        data: Some(Data::Sum(Sum {
                            aggregation_temporality: AggregationTemporality::Cumulative as i32,
                            is_monotonic: true,
                            data_points: vec![NumberDataPoint {
                                attributes: vec![],
                                start_time_unix_nano: 0,
                                time_unix_nano: 3,
                                exemplars: vec![],
                                flags: 0,
                                value: Some(NumVal::AsInt(3)),
                            }],
                        })),
                    }],
                    schema_url: String::new(),
                },
            ],
            schema_url: String::new(),
        };

        let resource_b = ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "payments")],
                dropped_attributes_count: 0,
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics: vec![Metric {
                    name: "m3".into(),
                    description: String::new(),
                    unit: String::new(),
                    metadata: vec![],
                    data: Some(Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            attributes: vec![],
                            start_time_unix_nano: 0,
                            time_unix_nano: 4,
                            exemplars: vec![],
                            flags: 0,
                            value: Some(NumVal::AsDouble(4.0)),
                        }],
                    })),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        };

        let pts = otlp_metrics_to_points(ExportMetricsServiceRequest {
            resource_metrics: vec![resource_a, resource_b],
        });

        assert_eq!(pts.len(), 4);

        // All three resource_a points (spanning both scopes, including the last one served
        // via mem::take) must carry both resource attrs.
        for p in &pts[0..3] {
            assert_eq!(
                p.attributes.get("service.name").map(String::as_str),
                Some("checkout"),
                "point {:?} missing resource attr service.name",
                p.metric_name
            );
            assert_eq!(
                p.attributes.get("region").map(String::as_str),
                Some("us"),
                "point {:?} missing resource attr region",
                p.metric_name
            );
        }
        // Per-point attrs on the first two gauge points are preserved alongside resource attrs.
        assert_eq!(pts[0].attributes.get("core").map(String::as_str), Some("0"));
        assert_eq!(pts[1].attributes.get("core").map(String::as_str), Some("1"));

        // resource_b's single point gets its own resource attrs, not resource_a's.
        let p3 = &pts[3];
        assert_eq!(
            p3.attributes.get("service.name").map(String::as_str),
            Some("payments")
        );
        assert!(!p3.attributes.contains_key("region"));
    }

    // --- Streaming-path equivalence gate ------------------------------------------------------
    //
    // The streaming `otlp_metrics_into_builder` (the production ingest path) must produce a
    // byte-identical Arrow batch to the reference `otlp_metrics_to_points` +
    // `MetricBatchBuilder::append` path — across all 5 data-point types, including the
    // histogram/exp-histogram/summary/exemplar JSON payload columns. Mirrors the trace
    // equivalence tests (`into_builder_matches_to_records_then_build` /
    // `into_builder_sorts_and_dedups_attrs_span_wins`).

    /// Build the reference metric batch (map → append each → finish) and the streaming batch
    /// (`otlp_metrics_into_builder`) from the SAME request, then assert they are byte-identical.
    /// The reference path is independently asserted correct by the direct-value tests above; the
    /// equality here proves the streaming path reproduces it exactly (attr precedence,
    /// promoted-vs-map routing, timestamp clamping, per-type JSON payloads, exemplars, ...).
    fn assert_streaming_matches_reference(req: ExportMetricsServiceRequest, promoted: &[String]) {
        use photon_core::metric_record::MetricBatchBuilder;
        use photon_core::metric_schema::MetricSchema;
        let schema = MetricSchema::new(promoted);

        // Reference batch via the map+append path.
        let mut ref_b = MetricBatchBuilder::with_capacity(&schema, 8);
        for p in otlp_metrics_to_points(req.clone()) {
            ref_b.append(&p);
        }
        let reference = ref_b.finish().unwrap();

        // Streaming path.
        let mut b = MetricBatchBuilder::with_capacity(&schema, 8);
        otlp_metrics_into_builder(req, &mut b);
        let streamed = b.finish().unwrap();

        // `RecordBatch: PartialEq` does a typed, column-by-column comparison — assert the
        // "byte-identical" claim directly rather than comparing Debug strings. This subsumes the
        // `num_rows` check (unequal row counts compare unequal).
        assert_eq!(streamed, reference);
    }

    const SVC: &[&str] = &["service.name"];
    fn promoted() -> Vec<String> {
        SVC.iter().map(|s| s.to_string()).collect()
    }

    /// One exemplar carrying a value, timestamp, trace/span ids, and filtered attributes — so the
    /// `exemplars` JSON column is exercised (and proven identical) on the types that carry them.
    fn exemplar() -> Exemplar {
        Exemplar {
            filtered_attributes: vec![kv("trace.sampled", "true")],
            time_unix_nano: 1_699_999_999_000_000_000,
            span_id: vec![0x11; 8],
            trace_id: vec![0x22; 16],
            value: Some(exemplar::Value::AsDouble(1.5)),
        }
    }

    fn gauge_metric() -> Metric {
        Metric {
            name: "cpu.usage".into(),
            description: String::new(),
            unit: "1".into(),
            metadata: vec![],
            data: Some(Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: vec![kv("core", "0")],
                    start_time_unix_nano: 5,
                    time_unix_nano: 1_700_000_000_000_000_000,
                    exemplars: vec![exemplar()],
                    flags: 0,
                    value: Some(NumVal::AsDouble(0.73)),
                }],
            })),
        }
    }

    fn sum_metric() -> Metric {
        Metric {
            name: "http.requests".into(),
            description: String::new(),
            unit: "1".into(),
            metadata: vec![],
            data: Some(Data::Sum(Sum {
                aggregation_temporality: AggregationTemporality::Cumulative as i32,
                is_monotonic: true,
                data_points: vec![NumberDataPoint {
                    attributes: vec![kv("route", "/pay")],
                    start_time_unix_nano: 5,
                    time_unix_nano: 10,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(NumVal::AsInt(42)),
                }],
            })),
        }
    }

    fn histogram_metric() -> Metric {
        Metric {
            name: "http.duration".into(),
            description: String::new(),
            unit: "ms".into(),
            metadata: vec![],
            data: Some(Data::Histogram(Histogram {
                aggregation_temporality: AggregationTemporality::Delta as i32,
                data_points: vec![HistogramDataPoint {
                    attributes: vec![kv("route", "/pay")],
                    start_time_unix_nano: 5,
                    time_unix_nano: 20,
                    count: 3,
                    sum: Some(30.0),
                    bucket_counts: vec![1, 2],
                    explicit_bounds: vec![10.0],
                    exemplars: vec![exemplar()],
                    flags: 0,
                    min: Some(4.0),
                    max: Some(18.0),
                }],
            })),
        }
    }

    fn exp_histogram_metric() -> Metric {
        Metric {
            name: "rpc.duration".into(),
            description: String::new(),
            unit: "ms".into(),
            metadata: vec![],
            data: Some(Data::ExponentialHistogram(ExponentialHistogram {
                aggregation_temporality: AggregationTemporality::Cumulative as i32,
                data_points: vec![ExponentialHistogramDataPoint {
                    attributes: vec![kv("route", "/pay")],
                    start_time_unix_nano: 5,
                    time_unix_nano: 25,
                    count: 6,
                    sum: Some(42.0),
                    scale: 2,
                    zero_count: 1,
                    positive: Some(Buckets {
                        offset: 0,
                        bucket_counts: vec![1, 2, 3],
                    }),
                    negative: Some(Buckets {
                        offset: -1,
                        bucket_counts: vec![0, 1],
                    }),
                    flags: 0,
                    exemplars: vec![exemplar()],
                    min: Some(0.5),
                    max: Some(9.0),
                    zero_threshold: 0.0,
                }],
            })),
        }
    }

    fn summary_metric() -> Metric {
        Metric {
            name: "db.latency".into(),
            description: String::new(),
            unit: "ms".into(),
            metadata: vec![],
            data: Some(Data::Summary(Summary {
                data_points: vec![SummaryDataPoint {
                    attributes: vec![kv("db", "pg")],
                    start_time_unix_nano: 5,
                    time_unix_nano: 30,
                    count: 10,
                    sum: 123.4,
                    quantile_values: vec![
                        ValueAtQuantile {
                            quantile: 0.5,
                            value: 1.0,
                        },
                        ValueAtQuantile {
                            quantile: 0.99,
                            value: 9.0,
                        },
                    ],
                    flags: 0,
                }],
            })),
        }
    }

    /// Each of the 5 data-point types round-trips identically through the streaming path,
    /// INCLUDING its type-specific JSON payload column (and exemplars where present). Wrapped in
    /// `req` (one resource carrying `service.name=checkout`, one scope), so both paths route the
    /// promoted column and merge the point attrs the same way.
    #[test]
    fn into_builder_matches_reference_for_gauge() {
        assert_streaming_matches_reference(req(vec![gauge_metric()]), &promoted());
    }

    #[test]
    fn into_builder_matches_reference_for_sum() {
        assert_streaming_matches_reference(req(vec![sum_metric()]), &promoted());
    }

    #[test]
    fn into_builder_matches_reference_for_histogram() {
        assert_streaming_matches_reference(req(vec![histogram_metric()]), &promoted());
    }

    #[test]
    fn into_builder_matches_reference_for_exp_histogram() {
        assert_streaming_matches_reference(req(vec![exp_histogram_metric()]), &promoted());
    }

    #[test]
    fn into_builder_matches_reference_for_summary() {
        assert_streaming_matches_reference(req(vec![summary_metric()]), &promoted());
    }

    /// All 5 types in a single request (one resource/scope) — proves the per-point column
    /// selection (value vs histogram vs exp_histogram vs summary vs exemplars) is applied
    /// row-by-row identically on the streaming path.
    #[test]
    fn into_builder_matches_reference_for_all_types_together() {
        let req = req(vec![
            gauge_metric(),
            sum_metric(),
            histogram_metric(),
            exp_histogram_metric(),
            summary_metric(),
        ]);
        assert_streaming_matches_reference(req, &promoted());
    }

    /// Attr collision: point attrs must win over resource attrs, come out sorted ascending, and
    /// collapse to one entry per key (BTreeMap semantics), for BOTH the promoted `service.name`
    /// column and the long-tail map. Mirrors `into_builder_sorts_and_dedups_attrs_span_wins`:
    ///   (a) ≥3 non-promoted keys in NON-alphabetical order (`region`, `http.status`, `env`);
    ///   (b) a non-promoted key (`env`) in BOTH resource and point with different values;
    ///   (c) the promoted key (`service.name`) in BOTH with different values.
    #[test]
    fn into_builder_sorts_and_dedups_attrs_point_wins() {
        let m = Metric {
            name: "cpu.usage".into(),
            description: String::new(),
            unit: "1".into(),
            metadata: vec![],
            data: Some(Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: vec![
                        kv("region", "apac"),
                        kv("http.status", "504"),
                        kv("env", "point-env"),
                        kv("service.name", "point-svc"),
                    ],
                    start_time_unix_nano: 0,
                    time_unix_nano: 1,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(NumVal::AsDouble(1.0)),
                }],
            })),
        };
        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![
                        kv("service.name", "resource-svc"),
                        kv("env", "resource-env"),
                    ],
                    dropped_attributes_count: 0,
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![m],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        assert_streaming_matches_reference(request, &promoted());
    }

    /// Empty request: no resource groups → zero-row batches, identical both ways.
    #[test]
    fn into_builder_matches_reference_on_empty_request() {
        let req = ExportMetricsServiceRequest {
            resource_metrics: vec![],
        };
        assert_streaming_matches_reference(req, &promoted());
    }

    /// A point carrying no attributes at all, under a resource with no attributes — the merged
    /// set is empty, so the promoted column and the map are both null/empty. Must match.
    #[test]
    fn into_builder_matches_reference_for_point_without_attributes() {
        let m = Metric {
            name: "cpu.usage".into(),
            description: String::new(),
            unit: String::new(),
            metadata: vec![],
            data: Some(Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: vec![],
                    start_time_unix_nano: 0,
                    time_unix_nano: 1,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(NumVal::AsDouble(1.0)),
                }],
            })),
        };
        let req = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: None,
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![m],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        assert_streaming_matches_reference(req, &promoted());
    }

    /// Multiple resource groups, each with multiple scope groups and per-scope scope names,
    /// mixing all 5 types, a point overriding the promoted `service.name`, and a no-attrs point.
    /// Exercises the reference's per-resource `mem::take`-on-last-point path (across scopes) and
    /// proves the streaming path — which re-borrows the owned resource attrs per point — matches.
    #[test]
    fn into_builder_matches_reference_across_multiple_groups() {
        fn scope(name: &str, metrics: Vec<Metric>) -> ScopeMetrics {
            ScopeMetrics {
                scope: Some(InstrumentationScope {
                    name: name.to_string(),
                    version: String::new(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                metrics,
                schema_url: String::new(),
            }
        }
        // A gauge whose point overrides the promoted service.name.
        let override_gauge = Metric {
            name: "cpu.usage".into(),
            description: String::new(),
            unit: "1".into(),
            metadata: vec![],
            data: Some(Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: vec![kv("service.name", "override"), kv("core", "7")],
                    start_time_unix_nano: 0,
                    time_unix_nano: 9,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(NumVal::AsDouble(3.0)),
                }],
            })),
        };
        let req = ExportMetricsServiceRequest {
            resource_metrics: vec![
                ResourceMetrics {
                    resource: Some(Resource {
                        attributes: vec![kv("service.name", "svc-a"), kv("region", "us")],
                        dropped_attributes_count: 0,
                    }),
                    scope_metrics: vec![
                        scope("scope-1", vec![gauge_metric(), histogram_metric()]),
                        scope("scope-2", vec![summary_metric(), override_gauge]),
                    ],
                    schema_url: String::new(),
                },
                ResourceMetrics {
                    resource: Some(Resource {
                        attributes: vec![kv("service.name", "svc-b")],
                        dropped_attributes_count: 0,
                    }),
                    scope_metrics: vec![scope(
                        "scope-3",
                        vec![sum_metric(), exp_histogram_metric()],
                    )],
                    schema_url: String::new(),
                },
            ],
        };
        assert_streaming_matches_reference(req, &promoted());
    }

    /// The streaming path must apply the IDENTICAL untrusted-nanos clamping as the reference:
    /// `time_unix_nano = u64::MAX` → `i64::MAX`, and `start_time_unix_nano = u64::MAX` →
    /// `Some(i64::MAX)` (via `nz`). Also covers a `start = 0` point (→ `None` start). Proving
    /// streaming == reference (whose clamping is asserted correct by the direct tests above)
    /// proves streaming clamps too.
    #[test]
    fn into_builder_clamps_timestamps_identically() {
        let m = Metric {
            name: "cpu.usage".into(),
            description: String::new(),
            unit: "1".into(),
            metadata: vec![],
            data: Some(Data::Gauge(Gauge {
                data_points: vec![
                    NumberDataPoint {
                        attributes: vec![],
                        start_time_unix_nano: u64::MAX,
                        time_unix_nano: u64::MAX,
                        exemplars: vec![],
                        flags: 0,
                        value: Some(NumVal::AsDouble(1.0)),
                    },
                    NumberDataPoint {
                        attributes: vec![],
                        start_time_unix_nano: 0,               // → None start
                        time_unix_nano: (i64::MAX as u64) + 1, // wraps negative under `as i64`
                        exemplars: vec![],
                        flags: 0,
                        value: Some(NumVal::AsDouble(2.0)),
                    },
                ],
            })),
        };
        assert_streaming_matches_reference(req(vec![m]), &promoted());
    }
}

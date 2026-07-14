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
use photon_core::metric_record::MetricPoint;
use photon_core::metric_schema::metric_type;
use serde::Serialize;

use crate::otlp_value::{any_value_into_string, bytes_to_hex_opt};

pub fn otlp_metrics_to_points(req: ExportMetricsServiceRequest) -> Vec<MetricPoint> {
    let mut out: Vec<MetricPoint> = Vec::new();

    for rm in req.resource_metrics {
        let resource_attrs: BTreeMap<String, String> = rm
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
                            out.push(number_point(
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                metric_type::GAUGE,
                                None,
                                None,
                                dp,
                            ));
                        }
                    }
                    Data::Sum(s) => {
                        for dp in s.data_points {
                            out.push(number_point(
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                metric_type::SUM,
                                Some(s.aggregation_temporality),
                                Some(s.is_monotonic),
                                dp,
                            ));
                        }
                    }
                    Data::Histogram(h) => {
                        for dp in h.data_points {
                            out.push(histogram_point(
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                h.aggregation_temporality,
                                dp,
                            ));
                        }
                    }
                    Data::ExponentialHistogram(h) => {
                        for dp in h.data_points {
                            out.push(exp_histogram_point(
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                h.aggregation_temporality,
                                dp,
                            ));
                        }
                    }
                    Data::Summary(s) => {
                        for dp in s.data_points {
                            out.push(summary_point(
                                &name,
                                &unit,
                                &scope_name,
                                &resource_attrs,
                                dp,
                            ));
                        }
                    }
                }
            }
        }
    }
    out
}

/// Merge resource attrs (base) with this point's attributes (override on key collision).
fn merge_attrs(
    base: &BTreeMap<String, String>,
    point_attrs: Vec<opentelemetry_proto::tonic::common::v1::KeyValue>,
) -> BTreeMap<String, String> {
    let mut m = base.clone();
    for kv in point_attrs {
        m.insert(
            kv.key,
            kv.value.map(any_value_into_string).unwrap_or_default(),
        );
    }
    m
}

fn nz(nanos: u64) -> Option<i64> {
    (nanos != 0).then_some(nanos as i64)
}

#[allow(clippy::too_many_arguments)]
fn number_point(
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    base: &BTreeMap<String, String>,
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
        timestamp_nanos: dp.time_unix_nano as i64,
        start_timestamp_nanos: nz(dp.start_time_unix_nano),
        scope_name: scope.clone(),
        value,
        exemplars: exemplars_json(dp.exemplars),
        attributes: merge_attrs(base, dp.attributes),
        ..Default::default()
    }
}

fn histogram_point(
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    base: &BTreeMap<String, String>,
    temporality: i32,
    dp: HistogramDataPoint,
) -> MetricPoint {
    let payload = HistogramJson {
        count: dp.count,
        sum: dp.sum,
        bucket_counts: dp.bucket_counts.clone(),
        explicit_bounds: dp.explicit_bounds.clone(),
        min: dp.min,
        max: dp.max,
    };
    MetricPoint {
        metric_name: name.to_string(),
        metric_type: metric_type::HISTOGRAM,
        type_text: Some("HISTOGRAM".into()),
        temporality: Some(temporality),
        unit: unit.clone(),
        timestamp_nanos: dp.time_unix_nano as i64,
        start_timestamp_nanos: nz(dp.start_time_unix_nano),
        scope_name: scope.clone(),
        histogram: serde_json::to_string(&payload).ok(),
        exemplars: exemplars_json(dp.exemplars),
        attributes: merge_attrs(base, dp.attributes),
        ..Default::default()
    }
}

fn exp_histogram_point(
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    base: &BTreeMap<String, String>,
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
        timestamp_nanos: dp.time_unix_nano as i64,
        start_timestamp_nanos: nz(dp.start_time_unix_nano),
        scope_name: scope.clone(),
        exp_histogram: serde_json::to_string(&payload).ok(),
        exemplars: exemplars_json(dp.exemplars),
        attributes: merge_attrs(base, dp.attributes),
        ..Default::default()
    }
}

fn summary_point(
    name: &str,
    unit: &Option<String>,
    scope: &Option<String>,
    base: &BTreeMap<String, String>,
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
        timestamp_nanos: dp.time_unix_nano as i64,
        start_timestamp_nanos: nz(dp.start_time_unix_nano),
        scope_name: scope.clone(),
        summary: serde_json::to_string(&payload).ok(),
        attributes: merge_attrs(base, dp.attributes),
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
    use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
    use opentelemetry_proto::tonic::metrics::v1::{
        metric::Data, number_data_point::Value as NumVal, AggregationTemporality, Gauge, Histogram,
        HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
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
}

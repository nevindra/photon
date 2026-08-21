//! `MetricPoint` → OTLP protobuf, for shipping this Site's rollup to a federation central.
//!
//! Note what this is NOT for: the local write path does not use it. The rollup appends
//! `MetricPoint`s straight to the metrics WAL (see `points.rs`), because encoding protobuf to POST
//! to ourselves would be pure overhead. This module exists only because central is a different
//! process, reached over the wire, and the wire speaks OTLP.
//!
//! It also exists because neither federation mode covers the rollup on its own: `summary` mode
//! pushes synthetic health metrics and nothing per-service, and `full` mode mirrors *accepted OTLP
//! batches* — which the rollup, writing directly to the WAL, never becomes. So the points are
//! offered to the same tee the full-mode forwarder drains, in both modes.

use std::collections::BTreeMap;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{
    any_value::Value, AnyValue, InstrumentationScope, KeyValue,
};
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point::Value as NumVal, AggregationTemporality, Gauge,
    HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use photon_core::metric_record::MetricPoint;
use photon_core::metric_schema;
use prost::Message as _;

use super::points::{ATTR_ENVIRONMENT, ATTR_SERVICE};

fn kv(key: &str, val: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(val.to_string())),
        }),
    }
}

/// Attributes minus the two that become RESOURCE attributes. Sending `service.name` on both the
/// resource and the data point would make central's mapper see it twice, and which copy wins is
/// not something to leave to chance.
fn point_attrs(attrs: &BTreeMap<String, String>) -> Vec<KeyValue> {
    attrs
        .iter()
        .filter(|(k, _)| k.as_str() != ATTR_SERVICE && k.as_str() != ATTR_ENVIRONMENT)
        .map(|(k, v)| kv(k, v))
        .collect()
}

/// Encode a pass's points as one `ExportMetricsServiceRequest`, grouped into one `ResourceMetrics`
/// per `(service.name, deployment.environment)` — the shape a real OTel SDK would emit, and the
/// shape central's existing mapper already understands.
pub fn encode(points: &[MetricPoint]) -> Vec<u8> {
    let mut by_resource: BTreeMap<(String, Option<String>), Vec<Metric>> = BTreeMap::new();
    for p in points {
        let key = (
            p.attributes.get(ATTR_SERVICE).cloned().unwrap_or_default(),
            p.attributes.get(ATTR_ENVIRONMENT).cloned(),
        );
        if let Some(m) = to_metric(p) {
            by_resource.entry(key).or_default().push(m);
        }
    }

    let resource_metrics = by_resource
        .into_iter()
        .map(|((service, environment), metrics)| {
            let mut attributes = vec![kv(ATTR_SERVICE, &service)];
            if let Some(env) = &environment {
                attributes.push(kv(ATTR_ENVIRONMENT, env));
            }
            ResourceMetrics {
                resource: Some(Resource {
                    attributes,
                    dropped_attributes_count: 0,
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: Some(InstrumentationScope {
                        name: "photon.rollup".into(),
                        ..Default::default()
                    }),
                    metrics,
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }
        })
        .collect();

    ExportMetricsServiceRequest { resource_metrics }.encode_to_vec()
}

/// One point as an OTLP `Metric`. `None` for a shape this module does not emit — the rollup only
/// ever writes delta sums, explicit histograms and one gauge, so anything else is a caller bug and
/// dropping it is better than guessing a temporality.
fn to_metric(p: &MetricPoint) -> Option<Metric> {
    let attributes = point_attrs(&p.attributes);
    let ts = p.timestamp_nanos.max(0) as u64;
    let start = p.start_timestamp_nanos.unwrap_or(p.timestamp_nanos).max(0) as u64;

    let data = match p.metric_type {
        metric_schema::metric_type::SUM => Data::Sum(Sum {
            data_points: vec![NumberDataPoint {
                attributes,
                start_time_unix_nano: start,
                time_unix_nano: ts,
                exemplars: vec![],
                flags: 0,
                value: Some(NumVal::AsDouble(p.value.unwrap_or(0.0))),
            }],
            // DELTA, matching what the local write path stored. Sending these as cumulative would
            // make central difference them against a previous value that does not exist.
            aggregation_temporality: AggregationTemporality::Delta as i32,
            is_monotonic: p.is_monotonic.unwrap_or(true),
        }),
        metric_schema::metric_type::GAUGE => Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes,
                start_time_unix_nano: 0,
                time_unix_nano: ts,
                exemplars: vec![],
                flags: 0,
                value: Some(NumVal::AsDouble(p.value.unwrap_or(0.0))),
            }],
        }),
        metric_schema::metric_type::HISTOGRAM => {
            // Read back through `serde_json::Value` rather than a mirrored struct: this is the
            // THIRD place that JSON shape is pinned (the writer in `points.rs`, `photon-query`'s
            // reader, and here), and a struct would look like a shared definition while being
            // nothing of the kind. `tests/rollup.rs` is what keeps all three honest.
            let h: serde_json::Value = serde_json::from_str(p.histogram.as_deref()?).ok()?;
            let nums = |key: &str| -> Vec<u64> {
                h[key]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
                    .unwrap_or_default()
            };
            Data::Histogram(opentelemetry_proto::tonic::metrics::v1::Histogram {
                data_points: vec![HistogramDataPoint {
                    attributes,
                    start_time_unix_nano: start,
                    time_unix_nano: ts,
                    count: h["count"].as_u64().unwrap_or(0),
                    sum: h["sum"].as_f64(),
                    bucket_counts: nums("bucket_counts"),
                    explicit_bounds: h["explicit_bounds"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
                        .unwrap_or_default(),
                    exemplars: vec![],
                    flags: 0,
                    min: None,
                    max: None,
                }],
                aggregation_temporality: AggregationTemporality::Delta as i32,
            })
        }
        _ => return None,
    };

    Some(Metric {
        name: p.metric_name.clone(),
        description: String::new(),
        unit: p.unit.clone().unwrap_or_default(),
        metadata: vec![],
        data: Some(data),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use photon_query::{RedBasis, RollupRow, LATENCY_BOUNDS_NS};

    fn row(operation: Option<&str>) -> RollupRow {
        RollupRow {
            ts: 1_700_000_000_000_000_000,
            service: "api".into(),
            environment: Some("prod".into()),
            operation: operation.map(String::from),
            count: 10,
            span_count: 14,
            basis: RedBasis::Entry,
            error_count: 1,
            http_4xx: 0,
            http_5xx: 1,
            satisfied: 9,
            tolerating: 1,
            frustrated: 0,
            latency_buckets: {
                let mut v = vec![0; LATENCY_BOUNDS_NS.len() + 1];
                v[2] = 10;
                v
            },
            duration_sum: 30_000_000,
            duration_count: 10,
        }
    }

    fn decoded(points: &[MetricPoint]) -> ExportMetricsServiceRequest {
        ExportMetricsServiceRequest::decode(encode(points).as_slice()).unwrap()
    }

    #[test]
    fn identity_rides_on_the_resource_not_the_data_point() {
        let req = decoded(&super::super::points::points_for(&row(None)));
        assert_eq!(req.resource_metrics.len(), 1);
        let res = req.resource_metrics[0].resource.as_ref().unwrap();
        let keys: Vec<&str> = res.attributes.iter().map(|a| a.key.as_str()).collect();
        assert!(keys.contains(&ATTR_SERVICE));
        assert!(keys.contains(&ATTR_ENVIRONMENT));

        for m in &req.resource_metrics[0].scope_metrics[0].metrics {
            if let Some(Data::Sum(sum)) = &m.data {
                let dp_keys: Vec<&str> = sum.data_points[0]
                    .attributes
                    .iter()
                    .map(|a| a.key.as_str())
                    .collect();
                assert!(
                    !dp_keys.contains(&ATTR_SERVICE),
                    "sending it twice leaves which copy wins to chance"
                );
            }
        }
    }

    #[test]
    fn counters_cross_the_wire_as_delta_sums() {
        let req = decoded(&super::super::points::points_for(&row(None)));
        let m = req.resource_metrics[0].scope_metrics[0]
            .metrics
            .iter()
            .find(|m| m.name == "photon.service.requests")
            .unwrap();
        let Some(Data::Sum(sum)) = &m.data else {
            panic!("requests must be a Sum")
        };
        assert_eq!(
            sum.aggregation_temporality,
            AggregationTemporality::Delta as i32,
            "cumulative would make central difference against a value that never existed"
        );
        assert!(sum.is_monotonic);
        assert_eq!(sum.data_points[0].value, Some(NumVal::AsDouble(10.0)));
    }

    #[test]
    fn the_histogram_survives_the_json_round_trip_into_protobuf() {
        let req = decoded(&super::super::points::points_for(&row(None)));
        let m = req.resource_metrics[0].scope_metrics[0]
            .metrics
            .iter()
            .find(|m| m.name == "photon.service.duration")
            .unwrap();
        let Some(Data::Histogram(h)) = &m.data else {
            panic!("duration must be a Histogram")
        };
        let dp = &h.data_points[0];
        assert_eq!(dp.count, 10);
        assert_eq!(dp.sum, Some(30_000_000.0));
        assert_eq!(dp.explicit_bounds.len(), LATENCY_BOUNDS_NS.len());
        assert_eq!(dp.bucket_counts.len(), LATENCY_BOUNDS_NS.len() + 1);
        assert_eq!(dp.bucket_counts[2], 10);
    }

    #[test]
    fn the_operation_level_keeps_its_own_metric_names_on_the_wire() {
        let req = decoded(&super::super::points::points_for(&row(Some("GET /a"))));
        let names: Vec<&str> = req.resource_metrics[0].scope_metrics[0]
            .metrics
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert!(names.iter().all(|n| n.starts_with("photon.operation.")));
        // And `operation` is a data-point attribute, since it is not part of the resource.
        let m = &req.resource_metrics[0].scope_metrics[0].metrics[0];
        let Some(Data::Sum(sum)) = &m.data else {
            panic!()
        };
        assert!(sum.data_points[0]
            .attributes
            .iter()
            .any(|a| a.key == "operation"));
    }

    #[test]
    fn two_services_become_two_resources() {
        let mut a = super::super::points::points_for(&row(None));
        let mut second = row(None);
        second.service = "web".into();
        a.extend(super::super::points::points_for(&second));
        assert_eq!(decoded(&a).resource_metrics.len(), 2);
    }
}

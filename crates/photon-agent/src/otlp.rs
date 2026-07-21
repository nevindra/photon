//! Maps `ResourceSample` → an OTLP `ExportMetricsServiceRequest`, mirroring
//! `photon-loadgen/src/metrics.rs`'s builder style but driven from the agent's `MetricSample`s.
//! Resource attributes (`host.name`, `host.id`, `os.type`) go on the `Resource`; each
//! `MetricSample`'s `attrs` become the data-point attributes.
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point::Value as NumVal, AggregationTemporality, Gauge, Metric,
    NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
};
use opentelemetry_proto::tonic::resource::v1::Resource;

use crate::sample::{Kind, MetricSample, ResourceSample};

fn kv(key: &str, val: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(val.to_string())),
        }),
    }
}

fn to_metric(m: &MetricSample, start: u64, now: u64) -> Metric {
    let dp = NumberDataPoint {
        attributes: m.attrs.iter().map(|(k, v)| kv(k, v)).collect(),
        // Cumulative monotonic sums carry the process start per the OTLP data model (lets
        // consumers compute rates and detect counter resets); gauges have no start.
        start_time_unix_nano: match m.kind {
            Kind::Gauge => 0,
            Kind::SumMonotonic => start,
        },
        time_unix_nano: now,
        exemplars: vec![],
        flags: 0,
        value: Some(NumVal::AsDouble(m.value)),
    };
    let data = match m.kind {
        Kind::Gauge => Data::Gauge(Gauge {
            data_points: vec![dp],
        }),
        Kind::SumMonotonic => Data::Sum(Sum {
            data_points: vec![dp],
            aggregation_temporality: AggregationTemporality::Cumulative as i32,
            is_monotonic: true,
        }),
    };
    Metric {
        name: m.name.to_string(),
        description: String::new(),
        unit: m.unit.to_string(),
        metadata: vec![],
        data: Some(data),
    }
}

pub fn to_otlp(
    host_name: &str,
    sample: &ResourceSample,
    start_nanos: u64,
    now_nanos: u64,
) -> ExportMetricsServiceRequest {
    let metrics = sample
        .metrics
        .iter()
        .map(|m| to_metric(m, start_nanos, now_nanos))
        .collect();
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![
                    kv("host.name", host_name),
                    kv("host.id", &sample.host_id),
                    kv("os.type", &sample.os_type),
                ],
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

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;

    pub(crate) fn sample() -> ResourceSample {
        ResourceSample {
            host_id: "id-1".into(),
            os_type: "linux".into(),
            metrics: vec![
                MetricSample {
                    name: "system.cpu.utilization",
                    unit: "1",
                    kind: Kind::Gauge,
                    value: 0.5,
                    attrs: vec![("cpu".into(), "total".into())],
                },
                MetricSample {
                    name: "system.network.io",
                    unit: "By",
                    kind: Kind::SumMonotonic,
                    value: 1234.0,
                    attrs: vec![
                        ("device".into(), "eth0".into()),
                        ("direction".into(), "receive".into()),
                    ],
                },
            ],
        }
    }

    #[test]
    fn cumulative_sums_carry_process_start_time_gauges_do_not() {
        let start = 1_699_999_000_000_000_000_u64;
        let req = to_otlp("web-1", &sample(), start, 1_700_000_000_000_000_000);
        let metrics = &req.resource_metrics[0].scope_metrics[0].metrics;
        let Some(Data::Gauge(gauge)) = &metrics[0].data else {
            panic!("cpu must be a Gauge")
        };
        assert_eq!(gauge.data_points[0].start_time_unix_nano, 0);
        let Some(Data::Sum(sum)) = &metrics[1].data else {
            panic!("network.io must be a Sum")
        };
        assert_eq!(sum.data_points[0].start_time_unix_nano, start);
    }

    #[test]
    fn maps_host_resource_attrs_and_kinds() {
        let req = to_otlp("web-1", &sample(), 0, 1_700_000_000_000_000_000);
        let rm = &req.resource_metrics[0];
        let attrs = &rm.resource.as_ref().unwrap().attributes;
        assert!(attrs.iter().any(|kv| kv.key == "host.name"));
        assert!(attrs.iter().any(|kv| kv.key == "host.id"));
        assert!(attrs.iter().any(|kv| kv.key == "os.type"));
        let metrics = &rm.scope_metrics[0].metrics;
        assert!(matches!(metrics[0].data, Some(Data::Gauge(_))));
        let Some(Data::Sum(sum)) = &metrics[1].data else {
            panic!("network.io must be a Sum")
        };
        assert!(sum.is_monotonic);
    }
}

/// Dev-only round-trip: the agent's OTLP payload decodes + maps the same way the real
/// `/v1/metrics` receiver maps it (`photon_ingest::otlp_metrics_to_points`), and a
/// `host.name`-tagged `system.cpu.utilization` point comes out. `otlp_metrics_to_points` returns
/// `Vec<MetricPoint>` directly (not a `Result`) — the plan's sketch assumed a fallible signature.
#[cfg(test)]
mod ingest_roundtrip {
    use super::tests::sample;
    use super::to_otlp;

    #[test]
    fn payload_maps_through_the_real_receiver_with_host_name() {
        let req = to_otlp("web-1", &sample(), 0, 1_700_000_000_000_000_000);
        let points = photon_ingest::otlp_metrics_to_points(req);
        let cpu = points
            .iter()
            .find(|p| p.metric_name == "system.cpu.utilization")
            .expect("cpu point");
        assert_eq!(
            cpu.attributes.get("host.name").map(String::as_str),
            Some("web-1")
        );
    }
}

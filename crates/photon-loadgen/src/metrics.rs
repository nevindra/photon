//! OTLP metrics payload construction + the metrics [`Payload`] impl. No I/O, so payload shape is
//! unit-testable via a direct protobuf decode round-trip. Unlike `logs.rs`/`traces.rs`, there is
//! no `photon_ingest::otlp_metrics_to_points` round-trip test here yet — that mapping (Task 5 of
//! the metrics-foundation plan) is built in parallel and this module builds proto directly, so it
//! has no dependency on it.
//!
//! Rotates through all five OTLP metric types (Gauge, Sum, Histogram, ExponentialHistogram,
//! Summary) by `i % 5`, spread across up to `services` distinct services — one `ResourceMetrics`
//! per service so `service.name` cardinality (a promoted, bloom-pruned column) actually varies.

use crate::payload::{Built, Payload};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{
    exponential_histogram_data_point::Buckets, metric::Data, number_data_point::Value as NumVal,
    summary_data_point::ValueAtQuantile, AggregationTemporality, ExponentialHistogram,
    ExponentialHistogramDataPoint, Gauge, Histogram, HistogramDataPoint, Metric, NumberDataPoint,
    ResourceMetrics, ScopeMetrics, Sum, Summary, SummaryDataPoint,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use rand::rngs::SmallRng;
use rand::Rng;

/// Fixed base timestamp (unix nanos). The loadgen doesn't need real wall-clock time, and a fixed
/// base keeps generated payloads deterministic given a seeded `rng`.
const BASE_TIME_NANOS: i64 = 1_700_000_000_000_000_000;
/// Nominal aggregation window used for Sum/Histogram/ExponentialHistogram start times.
const WINDOW_NANOS: i64 = 60_000_000_000;

/// The metrics load source: `metrics_per_request` data points per request (rotating through all
/// 5 OTLP metric types), spread across up to `services` distinct services.
pub struct MetricsPayload {
    pub metrics_per_request: usize,
    pub services: usize,
}

impl Payload for MetricsPayload {
    fn cost(&self) -> f64 {
        expected_datapoints(self.metrics_per_request, self.services) as f64
    }

    fn build(&self, rng: &mut SmallRng) -> Built {
        let (body, total) = build_request_bytes(self.metrics_per_request, self.services, rng);
        Built {
            body,
            units: total,
            spans: total,
        }
    }
}

/// The exact data-point count [`build_request_bytes`] emits for a given config: each of
/// `services` resource groups gets `metrics_per_request.div_ceil(services)` points. Deterministic
/// (independent of `rng`), so it doubles as `MetricsPayload::cost()`.
fn expected_datapoints(metrics_per_request: usize, services: usize) -> u64 {
    let per_service = metrics_per_request.div_ceil(services.max(1));
    (services * per_service) as u64
}

fn kv(key: &str, val: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(val.to_string())),
        }),
    }
}

/// Build one prost-encoded `ExportMetricsServiceRequest` of `metrics_per_request` data points,
/// spread across up to `services` services and rotating through all 5 OTLP metric shapes.
/// Returns `(encoded_bytes, total_data_point_count)`.
pub fn build_request_bytes(
    metrics_per_request: usize,
    services: usize,
    rng: &mut SmallRng,
) -> (Vec<u8>, u64) {
    let now = BASE_TIME_NANOS;
    let mut resource_metrics = Vec::new();
    let mut total = 0u64;
    for s in 0..services {
        let svc = format!("service-{s}");
        let mut metrics = Vec::new();
        for i in 0..metrics_per_request.div_ceil(services.max(1)) {
            // rotate through the 5 shapes by i % 5
            let m = match i % 5 {
                0 => gauge_metric(now, rng),
                1 => sum_metric(now, rng),
                2 => histogram_metric(now, rng),
                3 => exp_histogram_metric(now, rng),
                _ => summary_metric(now, rng),
            };
            metrics.push(m);
            total += 1;
        }
        resource_metrics.push(ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv("service.name", &svc)],
                dropped_attributes_count: 0,
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        });
    }
    let req = ExportMetricsServiceRequest { resource_metrics };
    let mut buf = Vec::with_capacity(req.encoded_len());
    req.encode(&mut buf)
        .expect("prost encode into a Vec is infallible");
    (buf, total)
}

fn gauge_metric(now: i64, rng: &mut SmallRng) -> Metric {
    Metric {
        name: "process.cpu.usage".into(),
        description: String::new(),
        unit: "1".into(),
        metadata: vec![],
        data: Some(Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: vec![],
                start_time_unix_nano: 0,
                time_unix_nano: now as u64,
                exemplars: vec![],
                flags: 0,
                value: Some(NumVal::AsDouble(rng.gen_range(0.0..1.0))),
            }],
        })),
    }
}

fn sum_metric(now: i64, rng: &mut SmallRng) -> Metric {
    Metric {
        name: "http.server.request_count".into(),
        description: String::new(),
        unit: "1".into(),
        metadata: vec![],
        data: Some(Data::Sum(Sum {
            data_points: vec![NumberDataPoint {
                attributes: vec![],
                start_time_unix_nano: (now - WINDOW_NANOS) as u64,
                time_unix_nano: now as u64,
                exemplars: vec![],
                flags: 0,
                value: Some(NumVal::AsInt(rng.gen_range(0..10_000))),
            }],
            aggregation_temporality: AggregationTemporality::Cumulative as i32,
            is_monotonic: true,
        })),
    }
}

fn histogram_metric(now: i64, rng: &mut SmallRng) -> Metric {
    let bucket_counts: Vec<u64> = (0..5).map(|_| rng.gen_range(0..100)).collect();
    let count: u64 = bucket_counts.iter().sum();
    Metric {
        name: "http.server.duration".into(),
        description: String::new(),
        unit: "ms".into(),
        metadata: vec![],
        data: Some(Data::Histogram(Histogram {
            data_points: vec![HistogramDataPoint {
                attributes: vec![],
                start_time_unix_nano: (now - WINDOW_NANOS) as u64,
                time_unix_nano: now as u64,
                count,
                sum: Some(rng.gen_range(0.0..1000.0)),
                bucket_counts,
                // one fewer bound than bucket_counts, per the OTLP histogram contract
                explicit_bounds: vec![5.0, 10.0, 25.0, 50.0],
                exemplars: vec![],
                flags: 0,
                min: Some(0.0),
                max: Some(120.0),
            }],
            aggregation_temporality: AggregationTemporality::Delta as i32,
        })),
    }
}

fn exp_histogram_metric(now: i64, rng: &mut SmallRng) -> Metric {
    let bucket_counts: Vec<u64> = (0..4).map(|_| rng.gen_range(0..50)).collect();
    let count: u64 = bucket_counts.iter().sum();
    Metric {
        name: "http.server.duration.exp".into(),
        description: String::new(),
        unit: "ms".into(),
        metadata: vec![],
        data: Some(Data::ExponentialHistogram(ExponentialHistogram {
            data_points: vec![ExponentialHistogramDataPoint {
                attributes: vec![],
                start_time_unix_nano: (now - WINDOW_NANOS) as u64,
                time_unix_nano: now as u64,
                count,
                sum: Some(rng.gen_range(0.0..1000.0)),
                scale: 2,
                zero_count: 0,
                positive: Some(Buckets {
                    offset: 0,
                    bucket_counts,
                }),
                negative: None,
                flags: 0,
                exemplars: vec![],
                min: Some(0.0),
                max: Some(500.0),
                zero_threshold: 0.0,
            }],
            aggregation_temporality: AggregationTemporality::Delta as i32,
        })),
    }
}

fn summary_metric(now: i64, rng: &mut SmallRng) -> Metric {
    let quantile_values = vec![
        ValueAtQuantile {
            quantile: 0.5,
            value: rng.gen_range(0.0..100.0),
        },
        ValueAtQuantile {
            quantile: 0.9,
            value: rng.gen_range(0.0..200.0),
        },
        ValueAtQuantile {
            quantile: 0.99,
            value: rng.gen_range(0.0..300.0),
        },
    ];
    Metric {
        name: "http.server.duration.summary".into(),
        description: String::new(),
        unit: "ms".into(),
        metadata: vec![],
        data: Some(Data::Summary(Summary {
            data_points: vec![SummaryDataPoint {
                attributes: vec![],
                start_time_unix_nano: (now - WINDOW_NANOS) as u64,
                time_unix_nano: now as u64,
                count: rng.gen_range(1..1000),
                sum: rng.gen_range(0.0..10_000.0),
                quantile_values,
                flags: 0,
            }],
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::BTreeSet;

    fn decode(bytes: &[u8]) -> ExportMetricsServiceRequest {
        ExportMetricsServiceRequest::decode(bytes).expect("valid OTLP protobuf")
    }

    #[test]
    fn build_request_bytes_decodes() {
        let mut rng = SmallRng::seed_from_u64(1);
        let (bytes, n) = build_request_bytes(10, 3, &mut rng);
        assert!(n > 0);
        let req = <ExportMetricsServiceRequest as prost::Message>::decode(&bytes[..]).unwrap();
        assert!(!req.resource_metrics.is_empty());
    }

    #[test]
    fn rotates_through_all_five_metric_types() {
        let mut rng = SmallRng::seed_from_u64(2);
        let (bytes, _) = build_request_bytes(25, 5, &mut rng);
        let req = decode(&bytes);

        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        for rm in &req.resource_metrics {
            for sm in &rm.scope_metrics {
                for m in &sm.metrics {
                    match m.data {
                        Some(Data::Gauge(_)) => {
                            seen.insert("gauge");
                        }
                        Some(Data::Sum(_)) => {
                            seen.insert("sum");
                        }
                        Some(Data::Histogram(_)) => {
                            seen.insert("histogram");
                        }
                        Some(Data::ExponentialHistogram(_)) => {
                            seen.insert("exp_histogram");
                        }
                        Some(Data::Summary(_)) => {
                            seen.insert("summary");
                        }
                        None => {}
                    }
                }
            }
        }
        assert_eq!(seen.len(), 5, "expected all 5 metric types, got {seen:?}");
    }

    #[test]
    fn covers_service_cardinality() {
        let mut rng = SmallRng::seed_from_u64(3);
        let (bytes, _) = build_request_bytes(30, 6, &mut rng);
        let req = decode(&bytes);

        let seen: BTreeSet<String> = req
            .resource_metrics
            .iter()
            .map(|rm| {
                let value = rm
                    .resource
                    .as_ref()
                    .expect("resource present")
                    .attributes
                    .iter()
                    .find(|kv| kv.key == "service.name")
                    .and_then(|kv| kv.value.as_ref())
                    .and_then(|v| v.value.clone());
                match value {
                    Some(Value::StringValue(s)) => s,
                    _ => panic!("expected string service.name"),
                }
            })
            .collect();
        let expected: BTreeSet<String> = (0..6).map(|i| format!("service-{i}")).collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn total_matches_expected_datapoints() {
        let mut rng = SmallRng::seed_from_u64(4);
        let (_, total) = build_request_bytes(17, 4, &mut rng);
        assert_eq!(total, expected_datapoints(17, 4));
    }
}

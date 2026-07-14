//! Prometheus remote-write `WriteRequest` → `MetricPoint` mapping.
//!
//! RW 1.0 samples carry no type tag, so metric type is classified from the metric-name suffix
//! (the convention the OTel Collector's Prometheus receiver uses). `__name__` → `metric_name`,
//! `job` → `service.name`, all other labels → attributes. The receiver is stateless: histogram
//! bucket series are stored flat (`foo_bucket` with an `le` attribute); percentile reassembly is
//! a query-time concern (Plan 2).

use std::collections::BTreeMap;

use opentelemetry_proto::tonic::metrics::v1::AggregationTemporality;
use photon_core::metric_record::MetricPoint;
use photon_core::metric_schema::metric_type;

use crate::promrw_proto::WriteRequest;

const NAME_LABEL: &str = "__name__";
const JOB_LABEL: &str = "job";

pub fn promrw_to_points(req: WriteRequest) -> Vec<MetricPoint> {
    let mut out = Vec::new();
    for ts in req.timeseries {
        // Split labels: pull out __name__, rename job → service.name, keep the rest.
        let mut attributes: BTreeMap<String, String> = BTreeMap::new();
        let mut name: Option<String> = None;
        for label in ts.labels {
            match label.name.as_str() {
                NAME_LABEL => name = Some(label.value),
                JOB_LABEL => {
                    attributes.insert("service.name".to_string(), label.value);
                }
                _ => {
                    attributes.insert(label.name, label.value);
                }
            }
        }
        // A series with no __name__ is invalid; skip it (reject-before-WAL is upstream; this
        // just drops a nonsensical series rather than storing an empty metric name).
        let Some(name) = name else { continue };

        let (mtype, temporality, is_monotonic) = classify(&name);
        for sample in ts.samples {
            out.push(MetricPoint {
                metric_name: name.clone(),
                metric_type: mtype,
                type_text: Some(type_text(mtype).to_string()),
                temporality,
                is_monotonic,
                unit: None,
                // RW timestamps are unix milliseconds; Photon stores nanoseconds.
                timestamp_nanos: sample.timestamp.saturating_mul(1_000_000),
                start_timestamp_nanos: None,
                scope_name: None,
                value: Some(sample.value),
                attributes: attributes.clone(),
                ..Default::default()
            });
        }
    }
    out
}

/// Classify a Prometheus series into a Photon metric type by name suffix. Counter-like families
/// — `_total`, and histogram/summary components `_bucket` / `_count` / `_sum` — map to cumulative
/// monotonic `SUM` so Photon's existing reset-aware `rate()`/`increase()` applies unchanged.
/// Everything else — gauges, untyped series, and summary quantile series (`foo{quantile=...}`) —
/// maps to `GAUGE`.
fn classify(name: &str) -> (i32, Option<i32>, Option<bool>) {
    let counter_like = name.ends_with("_total")
        || name.ends_with("_bucket")
        || name.ends_with("_count")
        || name.ends_with("_sum");
    if counter_like {
        (
            metric_type::SUM,
            Some(AggregationTemporality::Cumulative as i32),
            Some(true),
        )
    } else {
        (metric_type::GAUGE, None, None)
    }
}

fn type_text(mtype: i32) -> &'static str {
    if mtype == metric_type::SUM {
        "SUM"
    } else {
        "GAUGE"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::promrw_proto::{Label, Sample, TimeSeries};

    fn label(name: &str, value: &str) -> Label {
        Label {
            name: name.into(),
            value: value.into(),
        }
    }

    fn series(labels: Vec<Label>, samples: Vec<Sample>) -> WriteRequest {
        WriteRequest {
            timeseries: vec![TimeSeries { labels, samples }],
        }
    }

    #[test]
    fn maps_a_counter_to_cumulative_monotonic_sum() {
        let req = series(
            vec![
                label("__name__", "http_requests_total"),
                label("job", "api"),
            ],
            vec![Sample {
                value: 7.0,
                timestamp: 1_700_000_000_000,
            }],
        );
        let pts = promrw_to_points(req);
        assert_eq!(pts.len(), 1);
        let p = &pts[0];
        assert_eq!(p.metric_name, "http_requests_total");
        assert_eq!(p.metric_type, metric_type::SUM);
        assert_eq!(
            p.temporality,
            Some(AggregationTemporality::Cumulative as i32)
        );
        assert_eq!(p.is_monotonic, Some(true));
        assert_eq!(p.value, Some(7.0));
        // ms → ns
        assert_eq!(p.timestamp_nanos, 1_700_000_000_000_000_000);
        // job → service.name; __name__ removed from attributes
        assert_eq!(
            p.attributes.get("service.name").map(String::as_str),
            Some("api")
        );
        assert!(!p.attributes.contains_key("__name__"));
    }

    #[test]
    fn maps_a_gauge_by_default_and_keeps_labels() {
        let req = series(
            vec![
                label("__name__", "process_resident_memory_bytes"),
                label("instance", "host-1:9090"),
            ],
            vec![Sample {
                value: 1234.0,
                timestamp: 1_700_000_000_000,
            }],
        );
        let p = &promrw_to_points(req)[0];
        assert_eq!(p.metric_type, metric_type::GAUGE);
        assert_eq!(p.temporality, None);
        assert_eq!(p.is_monotonic, None);
        assert_eq!(
            p.attributes.get("instance").map(String::as_str),
            Some("host-1:9090")
        );
    }

    #[test]
    fn bucket_series_is_sum_and_keeps_le_attribute() {
        let req = series(
            vec![
                label("__name__", "http_request_duration_seconds_bucket"),
                label("job", "api"),
                label("le", "0.5"),
            ],
            vec![Sample {
                value: 300.0,
                timestamp: 1_700_000_000_000,
            }],
        );
        let p = &promrw_to_points(req)[0];
        assert_eq!(p.metric_type, metric_type::SUM);
        assert_eq!(p.attributes.get("le").map(String::as_str), Some("0.5"));
    }

    #[test]
    fn emits_one_point_per_sample() {
        let req = series(
            vec![label("__name__", "up")],
            vec![
                Sample {
                    value: 1.0,
                    timestamp: 1_700_000_000_000,
                },
                Sample {
                    value: 1.0,
                    timestamp: 1_700_000_015_000,
                },
            ],
        );
        assert_eq!(promrw_to_points(req).len(), 2);
    }

    #[test]
    fn series_without_name_is_skipped() {
        let req = series(
            vec![label("job", "api")],
            vec![Sample {
                value: 1.0,
                timestamp: 1_700_000_000_000,
            }],
        );
        assert!(promrw_to_points(req).is_empty());
    }
}

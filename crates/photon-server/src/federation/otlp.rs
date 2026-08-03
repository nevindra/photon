//! Builds the OTLP metrics payload the summary pusher POSTs to central. `kv`/`to_metric` mirror
//! `photon-agent/src/otlp.rs:15-100` (photon-agent has no lib target, so this is a copy, not a
//! dep, adapted for the `photon.federation.*` namespace).
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point::Value as NumVal, AggregationTemporality, Gauge, Metric,
    NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
};
use opentelemetry_proto::tonic::resource::v1::Resource;

use photon_core::config::FederationMode;

use super::SummarySnapshot;

fn kv(key: &str, val: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(val.to_string())),
        }),
    }
}

fn gauge_metric(name: &str, value: f64, attrs: Vec<KeyValue>, now: u64) -> Metric {
    metric(
        name,
        Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: attrs,
                start_time_unix_nano: 0,
                time_unix_nano: now,
                exemplars: vec![],
                flags: 0,
                value: Some(NumVal::AsDouble(value)),
            }],
        }),
    )
}

/// One monotonic sum metric with one datapoint per `(signal, value)` pair — used for the two
/// cumulative `ingest.rows`/`ingest.bytes` counters (central differences them across pushes).
fn sum_metric(name: &str, per_signal: &[(String, u64)], mode: FederationMode, now: u64) -> Metric {
    let data_points = per_signal
        .iter()
        .map(|(signal, v)| NumberDataPoint {
            attributes: vec![kv("signal", signal), kv("mode", mode_str(mode))],
            start_time_unix_nano: 0,
            time_unix_nano: now,
            exemplars: vec![],
            flags: 0,
            value: Some(NumVal::AsDouble(*v as f64)),
        })
        .collect();
    metric(
        name,
        Data::Sum(Sum {
            data_points,
            aggregation_temporality: AggregationTemporality::Cumulative as i32,
            is_monotonic: true,
        }),
    )
}

fn metric(name: &str, data: Data) -> Metric {
    Metric {
        name: name.to_string(),
        description: String::new(),
        unit: String::new(),
        metadata: vec![],
        data: Some(data),
    }
}

fn mode_str(mode: FederationMode) -> &'static str {
    match mode {
        FederationMode::Summary => "summary",
        FederationMode::Full => "full",
    }
}

/// Pure builder: a `SummarySnapshot` -> the OTLP `ExportMetricsServiceRequest` pushed to
/// `{endpoint}/v1/metrics`. Resource attr `service.name = "photon"`; every datapoint carries
/// `mode`.
pub fn build_summary(snapshot: &SummarySnapshot, mode: FederationMode) -> ExportMetricsServiceRequest {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let mut metrics = vec![gauge_metric(
        "photon.federation.up",
        1.0,
        vec![kv("mode", mode_str(mode))],
        now,
    )];
    metrics.push(sum_metric(
        "photon.federation.ingest.rows",
        &snapshot.rows,
        mode,
        now,
    ));
    metrics.push(sum_metric(
        "photon.federation.ingest.bytes",
        &snapshot.bytes,
        mode,
        now,
    ));
    metrics.push(gauge_metric(
        "photon.federation.incidents.open",
        snapshot.open_incidents as f64,
        vec![kv("mode", mode_str(mode))],
        now,
    ));
    let hot_bytes_points = snapshot
        .hot_bytes
        .iter()
        .map(|(signal, v)| NumberDataPoint {
            attributes: vec![kv("signal", signal), kv("mode", mode_str(mode))],
            start_time_unix_nano: 0,
            time_unix_nano: now,
            exemplars: vec![],
            flags: 0,
            value: Some(NumVal::AsDouble(*v as f64)),
        })
        .collect();
    metrics.push(metric(
        "photon.federation.disk.hot_bytes",
        Data::Gauge(Gauge {
            data_points: hot_bytes_points,
        }),
    ));

    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "photon")],
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

    fn snapshot() -> SummarySnapshot {
        SummarySnapshot {
            rows: [
                ("logs".into(), 10),
                ("traces".into(), 20),
                ("metrics".into(), 30),
            ],
            bytes: [
                ("logs".into(), 100),
                ("traces".into(), 200),
                ("metrics".into(), 300),
            ],
            open_incidents: 2,
            hot_bytes: [
                ("logs".into(), 1000),
                ("traces".into(), 2000),
                ("metrics".into(), 3000),
            ],
        }
    }

    fn metric_names(req: &ExportMetricsServiceRequest) -> Vec<String> {
        req.resource_metrics[0].scope_metrics[0]
            .metrics
            .iter()
            .map(|m| m.name.clone())
            .collect()
    }

    #[test]
    fn emits_all_five_metrics() {
        let req = build_summary(&snapshot(), FederationMode::Summary);
        assert_eq!(
            metric_names(&req),
            vec![
                "photon.federation.up",
                "photon.federation.ingest.rows",
                "photon.federation.ingest.bytes",
                "photon.federation.incidents.open",
                "photon.federation.disk.hot_bytes",
            ]
        );
    }

    #[test]
    fn up_gauge_is_one_with_mode_attribute() {
        let req = build_summary(&snapshot(), FederationMode::Full);
        let up = &req.resource_metrics[0].scope_metrics[0].metrics[0];
        let Some(Data::Gauge(g)) = &up.data else {
            panic!("up must be a Gauge")
        };
        let dp = &g.data_points[0];
        assert_eq!(dp.value, Some(NumVal::AsDouble(1.0)));
        assert!(dp
            .attributes
            .iter()
            .any(|kv| kv.key == "mode" && kv.value.as_ref().unwrap().value
                == Some(Value::StringValue("full".to_string()))));
    }

    #[test]
    fn ingest_rows_has_one_datapoint_per_signal() {
        let req = build_summary(&snapshot(), FederationMode::Summary);
        let rows = &req.resource_metrics[0].scope_metrics[0].metrics[1];
        let Some(Data::Sum(s)) = &rows.data else {
            panic!("ingest.rows must be a Sum")
        };
        assert_eq!(s.data_points.len(), 3);
        assert!(s.is_monotonic);
        let signals: Vec<_> = s
            .data_points
            .iter()
            .flat_map(|dp| dp.attributes.iter())
            .filter(|kv| kv.key == "signal")
            .map(|kv| match &kv.value.as_ref().unwrap().value {
                Some(Value::StringValue(v)) => v.clone(),
                _ => panic!("signal attr must be a string"),
            })
            .collect();
        assert_eq!(signals, vec!["logs", "traces", "metrics"]);
    }
}

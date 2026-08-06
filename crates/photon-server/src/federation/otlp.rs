//! Builds the OTLP metrics payload the summary pusher POSTs to central. `kv`/`to_metric` mirror
//! `photon-agent/src/otlp.rs:15-100` (photon-agent has no lib target, so this is a copy, not a
//! dep, adapted for the `photon.federation.*` namespace).
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord as LogRecordProto, ResourceLogs, ScopeLogs};
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
fn sum_metric(name: &str, per_signal: &[(String, u64)], mode: &str, now: u64) -> Metric {
    let data_points = per_signal
        .iter()
        .map(|(signal, v)| NumberDataPoint {
            attributes: vec![kv("signal", signal), kv("mode", mode)],
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

/// The `mode` attribute value pushed with every summary metric: `summary`, `full`, or
/// `full:traces,metrics` when full mode mirrors a subset — central's card badge shows it verbatim.
pub fn mode_label(cfg: &photon_core::config::FederationConfig) -> String {
    match (cfg.mode, cfg.signals.as_deref()) {
        (FederationMode::Summary, _) => "summary".to_string(),
        (FederationMode::Full, None) => "full".to_string(),
        (FederationMode::Full, Some(signals)) => format!(
            "full:{}",
            signals
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// Pure builder: a `SummarySnapshot` -> the OTLP `ExportMetricsServiceRequest` pushed to
/// `{endpoint}/v1/metrics`. Resource attr `service.name = "photon"`; every datapoint carries
/// `mode`.
pub fn build_summary(snapshot: &SummarySnapshot, mode: &str) -> ExportMetricsServiceRequest {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let mut metrics = vec![gauge_metric(
        "photon.federation.up",
        1.0,
        vec![kv("mode", mode)],
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
        vec![kv("mode", mode)],
        now,
    ));
    let hot_bytes_points = snapshot
        .hot_bytes
        .iter()
        .map(|(signal, v)| NumberDataPoint {
            attributes: vec![kv("signal", signal), kv("mode", mode)],
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

/// Pure reverse-map: locally-stored RUM vitals gauge points -> the OTLP request the full-mode
/// forwarder POSTs to `{endpoint}/v1/metrics`. All attributes ride on the datapoint (central's
/// mapper merges resource+datapoint attrs, datapoint wins), so the resource stays empty.
pub fn build_rum_vitals(
    points: &[photon_core::metric_record::MetricPoint],
) -> ExportMetricsServiceRequest {
    let metrics = points
        .iter()
        .map(|p| Metric {
            name: p.metric_name.clone(),
            description: String::new(),
            unit: p.unit.clone().unwrap_or_default(),
            metadata: vec![],
            data: Some(Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: p.attributes.iter().map(|(k, v)| kv(k, v)).collect(),
                    start_time_unix_nano: 0,
                    time_unix_nano: p.timestamp_nanos.max(0) as u64,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(NumVal::AsDouble(p.value.unwrap_or(0.0))),
                }],
            })),
        })
        .collect();

    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: None,
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

/// Lowercase-hex id (the `LogRecord` string form) -> OTLP bytes; empty on malformed/absent.
fn hex_to_bytes(id: Option<&str>) -> Vec<u8> {
    let s = id.unwrap_or("");
    if s.is_empty() || !s.len().is_multiple_of(2) {
        return Vec::new();
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .unwrap_or_default()
}

/// Pure reverse-map: locally-stored RUM error log rows -> the OTLP request the full-mode
/// forwarder POSTs to `{endpoint}/v1/logs`. Attributes ride on the log record; resource empty.
/// `scope` stays `None` — `beacon_to_log_records` never sets `scope_name`.
pub fn build_rum_errors(records: &[photon_core::record::LogRecord]) -> ExportLogsServiceRequest {
    let log_records = records
        .iter()
        .map(|r| LogRecordProto {
            time_unix_nano: r.timestamp_nanos.max(0) as u64,
            observed_time_unix_nano: r.observed_timestamp_nanos.map_or(0, |n| n.max(0) as u64),
            severity_number: r.severity_number.unwrap_or(0),
            severity_text: r.severity_text.clone().unwrap_or_default(),
            body: r.body.as_ref().map(|b| AnyValue {
                value: Some(Value::StringValue(b.clone())),
            }),
            trace_id: hex_to_bytes(r.trace_id.as_deref()),
            span_id: hex_to_bytes(r.span_id.as_deref()),
            attributes: r.attributes.iter().map(|(k, v)| kv(k, v)).collect(),
            ..Default::default()
        })
        .collect();

    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: None,
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records,
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
        let req = build_summary(&snapshot(), "summary");
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
    fn mode_label_encodes_signal_subset() {
        use photon_core::config::{FederationConfig, FederationSignal};
        let base = FederationConfig {
            endpoint: "https://c".into(),
            token: "tk".into(),
            mode: FederationMode::Full,
            signals: None,
            interval_secs: 30,
            queue_batches: 1024,
        };
        assert_eq!(mode_label(&base), "full");
        let subset = FederationConfig {
            signals: Some(vec![FederationSignal::Traces, FederationSignal::Metrics]),
            ..base.clone()
        };
        assert_eq!(mode_label(&subset), "full:traces,metrics");
        let with_rum = FederationConfig {
            signals: Some(vec![FederationSignal::Traces, FederationSignal::Rum]),
            ..base.clone()
        };
        assert_eq!(mode_label(&with_rum), "full:traces,rum");
        let summary = FederationConfig {
            mode: FederationMode::Summary,
            signals: None,
            ..base
        };
        assert_eq!(mode_label(&summary), "summary");
    }

    #[test]
    fn up_gauge_is_one_with_mode_attribute() {
        let req = build_summary(&snapshot(), "full");
        let up = &req.resource_metrics[0].scope_metrics[0].metrics[0];
        let Some(Data::Gauge(g)) = &up.data else {
            panic!("up must be a Gauge")
        };
        let dp = &g.data_points[0];
        assert_eq!(dp.value, Some(NumVal::AsDouble(1.0)));
        assert!(dp.attributes.iter().any(|kv| kv.key == "mode"
            && kv.value.as_ref().unwrap().value == Some(Value::StringValue("full".to_string()))));
    }

    #[test]
    fn ingest_rows_has_one_datapoint_per_signal() {
        let req = build_summary(&snapshot(), "summary");
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

    #[test]
    fn rum_vitals_round_trip_through_ingest_mapping() {
        let point = photon_core::metric_record::MetricPoint {
            metric_name: "web_vitals.lcp".into(),
            unit: Some("ms".into()),
            timestamp_nanos: 1_700_000_000_000_000_000,
            value: Some(2450.5),
            attributes: [
                ("service.name".to_string(), "shop-web".to_string()),
                ("route".to_string(), "/checkout".to_string()),
            ]
            .into(),
            ..Default::default()
        };
        let req = build_rum_vitals(std::slice::from_ref(&point));
        let mapped = photon_ingest::otlp_metrics_to_points(req);
        assert_eq!(mapped.len(), 1);
        let m = &mapped[0];
        assert_eq!(m.metric_name, point.metric_name);
        assert_eq!(m.unit, point.unit);
        assert_eq!(m.timestamp_nanos, point.timestamp_nanos);
        assert_eq!(m.value, point.value);
        assert_eq!(m.attributes, point.attributes);
    }

    #[test]
    fn rum_errors_round_trip_through_ingest_mapping() {
        let record = photon_core::record::LogRecord {
            timestamp_nanos: 1_700_000_000_000_000_000,
            severity_number: Some(17),
            severity_text: Some("ERROR".into()),
            body: Some("TypeError: x is undefined".into()),
            trace_id: Some("0af7651916cd43dd8448eb211c80319c".into()),
            span_id: Some("b7ad6b7169203331".into()),
            attributes: [
                ("service.name".to_string(), "shop-web".to_string()),
                ("rum.fingerprint".to_string(), "abc123".to_string()),
            ]
            .into(),
            ..Default::default()
        };
        let req = build_rum_errors(std::slice::from_ref(&record));
        let mapped = photon_ingest::otlp_logs_to_records(req);
        assert_eq!(mapped.len(), 1);
        let r = &mapped[0];
        assert_eq!(r.timestamp_nanos, record.timestamp_nanos);
        assert_eq!(r.severity_number, record.severity_number);
        assert_eq!(r.severity_text, record.severity_text);
        assert_eq!(r.body, record.body);
        assert_eq!(r.trace_id, record.trace_id);
        assert_eq!(r.span_id, record.span_id);
        assert_eq!(r.attributes, record.attributes);
    }
}

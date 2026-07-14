//! Shared synthetic OTLP log fixtures for the write-path benches and the allocation guard.
//! Deterministic content (no RNG) so runs are comparable. Included by the criterion bench
//! (`mod fixture;`) and by the alloc-guard integration test (`#[path=...] mod fixture;`).

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;

fn kv(key: String, value: String) -> KeyValue {
    KeyValue {
        key,
        value: Some(AnyValue {
            value: Some(Value::StringValue(value)),
        }),
    }
}

/// One resource group carrying `rows` log records. `resource_attrs` resource-level attributes
/// (always incl. `service.name` + `host.name`), `attrs_per_record` per-record attributes.
pub fn logs_request(
    rows: usize,
    resource_attrs: usize,
    attrs_per_record: usize,
) -> ExportLogsServiceRequest {
    let mut resource_kvs = vec![
        kv("service.name".into(), "checkout".into()),
        kv("host.name".into(), "node-42".into()),
    ];
    for i in resource_kvs.len()..resource_attrs {
        resource_kvs.push(kv(format!("res.attr.{i}"), format!("res-value-{i}")));
    }

    let mut records = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut attrs = Vec::with_capacity(attrs_per_record);
        for a in 0..attrs_per_record {
            attrs.push(kv(format!("http.attr.{a}"), format!("value-{r}-{a}")));
        }
        records.push(LogRecord {
            time_unix_nano: 1_700_000_000_000_000_000 + r as u64,
            observed_time_unix_nano: 0,
            severity_number: 9, // INFO
            severity_text: "INFO".into(),
            body: Some(AnyValue {
                value: Some(Value::StringValue(format!(
                    "request {r} completed in {}ms",
                    r % 250
                ))),
            }),
            attributes: attrs,
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: vec![0xAB; 16],
            span_id: vec![0xCD; 8],
        });
    }

    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: resource_kvs,
                dropped_attributes_count: 0,
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

/// Same request, protobuf-encoded — for the decode→map→build bench and the alloc guard.
pub fn logs_request_bytes(rows: usize, resource_attrs: usize, attrs_per_record: usize) -> Vec<u8> {
    prost::Message::encode_to_vec(&logs_request(rows, resource_attrs, attrs_per_record))
}

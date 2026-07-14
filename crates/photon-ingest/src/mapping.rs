//! Pure OTLP → [`LogRecord`] mapping. No I/O, fully unit-testable.
//!
//! For each `resource_logs -> scope_logs -> log_records` triple, produces one
//! [`LogRecord`]. Resource attributes and the log record's own attributes are merged into
//! `LogRecord.attributes` (scope attributes are intentionally excluded per the
//! `photon-ingest` interface contract). `service.name` therefore arrives via the resource
//! attributes and lands in `attributes`, where [`RecordBatchBuilder`](photon_core::record::RecordBatchBuilder)
//! routes it to its promoted column.

use crate::otlp_value::{any_value_into_string, bytes_to_hex_opt};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use photon_core::record::{LogFixed, LogRecord, RecordBatchBuilder};
use std::collections::BTreeMap;

/// The event timestamp Photon files a log under.
///
/// OTLP's `time_unix_nano` (a log record's own event time) is optional, and many emitters leave
/// it unset — notably the zerolog→OTEL bridge, whose hook never stamps it, so every record ships
/// with `time_unix_nano == 0`. Mapping that straight through would file the log at nanos 0
/// (1970-01-01), where it is invisible to every realistic query window even though it was just
/// ingested. Per the OTLP log data model, fall back to `observed_time_unix_nano` (which the SDK
/// stamps at emit time) when the event time is absent, so the log lands at when it was actually
/// produced. If both are 0 the record carries no time at all and stays at 0.
fn effective_timestamp_nanos(time_unix_nano: u64, observed_time_unix_nano: u64) -> i64 {
    let nanos = if time_unix_nano != 0 {
        time_unix_nano
    } else {
        observed_time_unix_nano
    };
    nanos as i64
}

/// Map an OTLP `ExportLogsServiceRequest` into the flat `LogRecord`s Photon stores.
///
/// Takes `req` by value: every call site already owns a freshly-decoded request, so mapping
/// consumes it with `into_iter()` and moves strings (keys, values, names, ...) into the output
/// records instead of cloning them.
pub fn otlp_logs_to_records(req: ExportLogsServiceRequest) -> Vec<LogRecord> {
    let total: usize = req
        .resource_logs
        .iter()
        .flat_map(|rl| &rl.scope_logs)
        .map(|sl| sl.log_records.len())
        .sum();
    let mut out = Vec::with_capacity(total);

    for resource_logs in req.resource_logs {
        let mut resource_attrs: BTreeMap<String, String> = BTreeMap::new();
        if let Some(resource) = resource_logs.resource {
            for kv in resource.attributes {
                let value = kv.value.map(any_value_into_string).unwrap_or_default();
                resource_attrs.insert(kv.key, value);
            }
        }

        // Track how many log records this resource group will emit so the map can be moved
        // (not cloned) into the last one — every earlier record still needs its own copy.
        let records_in_resource: usize = resource_logs
            .scope_logs
            .iter()
            .map(|sl| sl.log_records.len())
            .sum();
        let mut emitted = 0usize;

        for scope_logs in resource_logs.scope_logs {
            let scope_name = scope_logs.scope.map(|s| s.name).filter(|n| !n.is_empty());

            for lr in scope_logs.log_records {
                emitted += 1;
                let mut attributes = if emitted == records_in_resource {
                    std::mem::take(&mut resource_attrs)
                } else {
                    resource_attrs.clone()
                };
                for kv in lr.attributes {
                    let value = kv.value.map(any_value_into_string).unwrap_or_default();
                    attributes.insert(kv.key, value);
                }

                let body = lr.body.map(any_value_into_string);
                let severity_text = Some(lr.severity_text).filter(|s| !s.is_empty());
                let trace_id = bytes_to_hex_opt(&lr.trace_id);
                let span_id = bytes_to_hex_opt(&lr.span_id);
                let observed_timestamp_nanos = if lr.observed_time_unix_nano == 0 {
                    None
                } else {
                    Some(lr.observed_time_unix_nano as i64)
                };

                out.push(LogRecord {
                    timestamp_nanos: effective_timestamp_nanos(
                        lr.time_unix_nano,
                        lr.observed_time_unix_nano,
                    ),
                    observed_timestamp_nanos,
                    severity_number: Some(lr.severity_number),
                    severity_text,
                    body,
                    trace_id,
                    span_id,
                    scope_name: scope_name.clone(),
                    attributes,
                });
            }
        }
    }

    out
}

/// Stream an OTLP request straight into the Arrow builder — the hot ingest path. No
/// intermediate `Vec<LogRecord>` and no per-record `BTreeMap`: for each resource group the
/// resource attributes are owned once, then chained (as borrowed pairs) with each record's
/// own attributes and appended directly. Same output batch as
/// `otlp_logs_to_records` + `append`, proven equal in tests.
pub fn otlp_logs_into_builder(req: ExportLogsServiceRequest, builder: &mut RecordBatchBuilder) {
    for resource_logs in req.resource_logs {
        // Own the resource attrs once per group (OTLP moves the strings out).
        let resource_attrs: Vec<(String, String)> = resource_logs
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

        for scope_logs in resource_logs.scope_logs {
            let scope_name = scope_logs.scope.map(|s| s.name).filter(|n| !n.is_empty());
            for lr in scope_logs.log_records {
                // Own this record's attrs (distinct per record).
                let rec_attrs: Vec<(String, String)> = lr
                    .attributes
                    .into_iter()
                    .map(|kv| {
                        (
                            kv.key,
                            kv.value.map(any_value_into_string).unwrap_or_default(),
                        )
                    })
                    .collect();

                let body = lr.body.map(any_value_into_string);
                let trace_id = bytes_to_hex_opt(&lr.trace_id);
                let span_id = bytes_to_hex_opt(&lr.span_id);
                let severity_text =
                    (!lr.severity_text.is_empty()).then_some(lr.severity_text.as_str());
                let observed =
                    (lr.observed_time_unix_nano != 0).then_some(lr.observed_time_unix_nano as i64);

                let fixed = LogFixed {
                    timestamp_nanos: effective_timestamp_nanos(
                        lr.time_unix_nano,
                        lr.observed_time_unix_nano,
                    ),
                    observed_timestamp_nanos: observed,
                    severity_number: Some(lr.severity_number),
                    severity_text,
                    body: body.as_deref(),
                    trace_id: trace_id.as_deref(),
                    span_id: span_id.as_deref(),
                    scope_name: scope_name.as_deref(),
                };
                // Reproduce `BTreeMap` iteration semantics without a per-record `BTreeMap`:
                // the attrs handed to `append_streaming` must be sorted by key ascending and
                // deduped to exactly one entry per key, record-wins on a duplicate (the
                // reference/append path merges into a resource-then-record `BTreeMap`, so a
                // repeated key keeps the record value and keys come out alphabetically).
                //
                // Collect the merged (resource-then-record) pairs as borrowed &str tagged with
                // their insertion index, then sort by (key asc, index desc) and `dedup_by` key.
                // `dedup_by` keeps the FIRST of each equal-key run; with index descending that
                // first is the highest index = the record's value — so duplicates collapse to
                // one entry, record beats resource, keys ascending. No String clones: `merged`
                // borrows from the already-owned `resource_attrs`/`rec_attrs` Vecs.
                let mut merged: Vec<(usize, &str, &str)> = resource_attrs
                    .iter()
                    .chain(rec_attrs.iter())
                    .enumerate()
                    .map(|(i, (k, v))| (i, k.as_str(), v.as_str()))
                    .collect();
                merged.sort_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(&a.0)));
                merged.dedup_by(|a, b| a.1 == b.1);
                builder.append_streaming(fixed, merged.iter().map(|&(_, k, v)| (k, v)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{
        any_value::Value, AnyValue, InstrumentationScope, KeyValue,
    };
    use opentelemetry_proto::tonic::logs::v1::{
        LogRecord as OtlpLogRecord, ResourceLogs, ScopeLogs, SeverityNumber,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;

    fn any_str(s: &str) -> AnyValue {
        AnyValue {
            value: Some(Value::StringValue(s.to_string())),
        }
    }

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(any_str(value)),
        }
    }

    #[test]
    fn maps_resource_and_log_record_fields() {
        let resource = Resource {
            attributes: vec![kv("service.name", "checkout")],
            dropped_attributes_count: 0,
        };
        let log_record = OtlpLogRecord {
            time_unix_nano: 1_700_000_000_000_000_000,
            observed_time_unix_nano: 1_700_000_000_000_000_500,
            severity_number: SeverityNumber::Info as i32,
            severity_text: "INFO".to_string(),
            body: Some(any_str("hello world")),
            attributes: vec![kv("http.method", "GET")],
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: vec![0xAB; 16],
            span_id: vec![0xCD; 8],
        };
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(resource),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "my-scope".to_string(),
                        version: String::new(),
                        attributes: vec![kv("scope.only", "should-be-ignored")],
                        dropped_attributes_count: 0,
                    }),
                    log_records: vec![log_record],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };

        let records = otlp_logs_to_records(req);
        assert_eq!(records.len(), 1);
        let r = &records[0];

        assert_eq!(r.timestamp_nanos, 1_700_000_000_000_000_000);
        assert_eq!(r.observed_timestamp_nanos, Some(1_700_000_000_000_000_500));
        assert_eq!(r.severity_number, Some(SeverityNumber::Info as i32));
        assert_eq!(r.severity_text.as_deref(), Some("INFO"));
        assert_eq!(r.body.as_deref(), Some("hello world"));
        assert_eq!(
            r.trace_id.as_deref(),
            Some("abababababababababababababababab")
        );
        assert_eq!(r.span_id.as_deref(), Some("cdcdcdcdcdcdcdcd"));
        assert_eq!(r.scope_name.as_deref(), Some("my-scope"));

        // service.name (resource attribute) and http.method (log record attribute) both
        // land in `attributes`; scope attributes are excluded per the interface contract.
        assert_eq!(
            r.attributes.get("service.name"),
            Some(&"checkout".to_string())
        );
        assert_eq!(r.attributes.get("http.method"), Some(&"GET".to_string()));
        assert!(!r.attributes.contains_key("scope.only"));
    }

    #[test]
    fn empty_request_yields_no_records() {
        let req = ExportLogsServiceRequest {
            resource_logs: vec![],
        };
        assert!(otlp_logs_to_records(req).is_empty());
    }

    #[test]
    fn missing_trace_and_span_ids_are_none() {
        let log_record = OtlpLogRecord {
            time_unix_nano: 1,
            observed_time_unix_nano: 0,
            severity_number: 0,
            severity_text: String::new(),
            body: None,
            attributes: vec![],
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: vec![],
            span_id: vec![],
        };
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![log_record],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };

        let records = otlp_logs_to_records(req);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.trace_id, None);
        assert_eq!(r.span_id, None);
        assert_eq!(r.observed_timestamp_nanos, None);
        assert_eq!(r.severity_text, None);
        assert_eq!(r.body, None);
        assert_eq!(r.scope_name, None);
        assert!(r.attributes.is_empty());
    }

    #[test]
    fn missing_event_time_falls_back_to_observed_time() {
        // The zerolog→OTEL bridge (and others) emit records with the event time unset; only the
        // SDK-stamped observed time is present. Photon must file the log at the observed time,
        // not at nanos 0 (1970), or it never shows up in a normal time-window query.
        let log_record = OtlpLogRecord {
            time_unix_nano: 0,
            observed_time_unix_nano: 1_700_000_000_000_000_777,
            severity_number: SeverityNumber::Info as i32,
            severity_text: "INFO".to_string(),
            body: Some(any_str("no event time")),
            attributes: vec![],
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: vec![],
            span_id: vec![],
        };
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![log_record],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };

        let records = otlp_logs_to_records(req);
        assert_eq!(records.len(), 1);
        // Filed at the observed time...
        assert_eq!(records[0].timestamp_nanos, 1_700_000_000_000_000_777);
        // ...while the observed field itself is still populated independently.
        assert_eq!(
            records[0].observed_timestamp_nanos,
            Some(1_700_000_000_000_000_777)
        );
    }

    #[test]
    fn missing_both_timestamps_stays_zero() {
        // Genuinely timeless record (neither event nor observed time): nothing to fall back to,
        // so it stays at 0 rather than being silently stamped with an invented time.
        let log_record = OtlpLogRecord {
            time_unix_nano: 0,
            observed_time_unix_nano: 0,
            severity_number: 0,
            severity_text: String::new(),
            body: None,
            attributes: vec![],
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: vec![],
            span_id: vec![],
        };
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![log_record],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let records = otlp_logs_to_records(req);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].timestamp_nanos, 0);
        assert_eq!(records[0].observed_timestamp_nanos, None);
    }

    #[test]
    fn stringifies_non_string_any_values() {
        let log_record = OtlpLogRecord {
            time_unix_nano: 1,
            observed_time_unix_nano: 0,
            severity_number: 0,
            severity_text: String::new(),
            body: Some(AnyValue {
                value: Some(Value::IntValue(42)),
            }),
            attributes: vec![KeyValue {
                key: "flag".to_string(),
                value: Some(AnyValue {
                    value: Some(Value::BoolValue(true)),
                }),
            }],
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: vec![],
            span_id: vec![],
        };
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![log_record],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };

        let records = otlp_logs_to_records(req);
        assert_eq!(records[0].body.as_deref(), Some("42"));
        assert_eq!(records[0].attributes.get("flag"), Some(&"true".to_string()));
    }

    #[test]
    fn into_builder_matches_to_records_then_build() {
        use photon_core::record::RecordBatchBuilder;
        use photon_core::schema::LogSchema;
        let schema = LogSchema::new(&["service.name".to_string()]);

        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![kv("service.name", "checkout")],
                    dropped_attributes_count: 0,
                }),
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![OtlpLogRecord {
                        time_unix_nano: 700,
                        observed_time_unix_nano: 0,
                        severity_number: 0,
                        severity_text: String::new(),
                        body: Some(any_str("hi")),
                        attributes: vec![kv("http.method", "GET")],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: vec![],
                        span_id: vec![],
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };

        // Reference batch via the old path.
        let mut ref_b = RecordBatchBuilder::with_capacity(&schema, 1);
        for r in otlp_logs_to_records(clone_req(&req)) {
            ref_b.append(&r);
        }
        let reference = ref_b.finish().unwrap();

        // Streaming path.
        let mut b = RecordBatchBuilder::with_capacity(&schema, 1);
        otlp_logs_into_builder(req, &mut b);
        let streamed = b.finish().unwrap();

        assert_eq!(streamed.num_rows(), reference.num_rows());
        assert_eq!(format!("{:?}", streamed), format!("{:?}", reference));
    }

    /// The streaming (production) path must apply the same event-time→observed-time fallback as
    /// the reference path, so a record with `time_unix_nano == 0` lands at its observed time in
    /// the built Arrow batch too — not at nanos 0.
    #[test]
    fn into_builder_falls_back_to_observed_time() {
        use photon_core::record::RecordBatchBuilder;
        use photon_core::schema::LogSchema;
        let schema = LogSchema::new(&["service.name".to_string()]);

        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![kv("service.name", "checkout")],
                    dropped_attributes_count: 0,
                }),
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![OtlpLogRecord {
                        time_unix_nano: 0,
                        observed_time_unix_nano: 1_700_000_000_000_000_777,
                        severity_number: 0,
                        severity_text: String::new(),
                        body: Some(any_str("hi")),
                        attributes: vec![kv("http.method", "GET")],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: vec![],
                        span_id: vec![],
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };

        // Reference batch via the old path (already asserted correct by the unit test above).
        let mut ref_b = RecordBatchBuilder::with_capacity(&schema, 1);
        for r in otlp_logs_to_records(clone_req(&req)) {
            ref_b.append(&r);
        }
        let reference = ref_b.finish().unwrap();

        // Streaming path.
        let mut b = RecordBatchBuilder::with_capacity(&schema, 1);
        otlp_logs_into_builder(req, &mut b);
        let streamed = b.finish().unwrap();

        assert_eq!(streamed.num_rows(), reference.num_rows());
        assert_eq!(format!("{:?}", streamed), format!("{:?}", reference));
    }

    /// Locks the streaming path to `BTreeMap` iteration semantics: the non-promoted attribute
    /// map must come out **sorted ascending, one entry per key, record-wins on duplicates**.
    /// The fixture exercises all three cases the narrow test missed:
    ///   (a) ≥3 non-promoted keys in NON-alphabetical OTLP order (`region`, `http.method`, `env`)
    ///       — locks the sort;
    ///   (b) a non-promoted key (`env`) present in BOTH resource and record with different values
    ///       — must collapse to ONE map entry, record value wins;
    ///   (c) a promoted key (`service.name`) present in BOTH with different values
    ///       — locks record-wins for the promoted column.
    #[test]
    fn into_builder_sorts_and_dedups_attrs_record_wins() {
        use photon_core::record::RecordBatchBuilder;
        use photon_core::schema::LogSchema;
        let schema = LogSchema::new(&["service.name".to_string()]);

        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    // (c) service.name (promoted) and (b) env (non-promoted) are BOTH also
                    // present on the record below, with different values — record must win.
                    attributes: vec![
                        kv("service.name", "resource-svc"),
                        kv("env", "resource-env"),
                    ],
                    dropped_attributes_count: 0,
                }),
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![OtlpLogRecord {
                        time_unix_nano: 700,
                        observed_time_unix_nano: 0,
                        severity_number: 0,
                        severity_text: String::new(),
                        body: Some(any_str("hi")),
                        // (a) non-alphabetical OTLP order; env + service.name duplicate the
                        // resource attrs above.
                        attributes: vec![
                            kv("region", "apac"),
                            kv("http.method", "POST"),
                            kv("env", "record-env"),
                            kv("service.name", "record-svc"),
                        ],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: vec![],
                        span_id: vec![],
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };

        // Reference batch via the BTreeMap path (sorted asc, one entry/key, record-wins).
        let mut ref_b = RecordBatchBuilder::with_capacity(&schema, 1);
        for r in otlp_logs_to_records(clone_req(&req)) {
            ref_b.append(&r);
        }
        let reference = ref_b.finish().unwrap();

        // Streaming path.
        let mut b = RecordBatchBuilder::with_capacity(&schema, 1);
        otlp_logs_into_builder(req, &mut b);
        let streamed = b.finish().unwrap();

        assert_eq!(streamed.num_rows(), reference.num_rows());
        assert_eq!(format!("{:?}", streamed), format!("{:?}", reference));
    }

    // OTLP request has no Clone derive in scope; build the reference from a rebuilt copy.
    fn clone_req(req: &ExportLogsServiceRequest) -> ExportLogsServiceRequest {
        // prost messages implement Clone.
        req.clone()
    }
}

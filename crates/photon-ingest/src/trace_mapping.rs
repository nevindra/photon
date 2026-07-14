//! Pure OTLP → [`SpanRecord`] mapping. No I/O, fully unit-testable.

use crate::otlp_value::{any_value_into_string, bytes_to_hex, bytes_to_hex_opt};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use photon_core::span_record::SpanRecord;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
struct EventJson {
    name: String,
    time_unix_nano: String,
    attributes: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct LinkJson {
    trace_id: String,
    span_id: String,
    attributes: BTreeMap<String, String>,
}

/// Map an OTLP `ExportTraceServiceRequest` into flat `SpanRecord`s.
///
/// Takes `req` by value: every call site already owns a freshly-decoded request, so mapping
/// consumes it with `into_iter()` and moves strings (keys, values, names, ...) into the output
/// spans instead of cloning them.
pub fn otlp_traces_to_spans(req: ExportTraceServiceRequest) -> Vec<SpanRecord> {
    let total: usize = req
        .resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .map(|ss| ss.spans.len())
        .sum();
    let mut out = Vec::with_capacity(total);

    for resource_spans in req.resource_spans {
        let mut resource_attrs: BTreeMap<String, String> = BTreeMap::new();
        if let Some(resource) = resource_spans.resource {
            for kv in resource.attributes {
                let value = kv.value.map(any_value_into_string).unwrap_or_default();
                resource_attrs.insert(kv.key, value);
            }
        }

        // Track how many spans this resource group will emit so the map can be moved (not
        // cloned) into the last one — every earlier span still needs its own copy.
        let spans_in_resource: usize = resource_spans
            .scope_spans
            .iter()
            .map(|ss| ss.spans.len())
            .sum();
        let mut emitted = 0usize;

        for scope_spans in resource_spans.scope_spans {
            let scope_name = scope_spans.scope.map(|s| s.name).filter(|n| !n.is_empty());

            for span in scope_spans.spans {
                emitted += 1;
                let mut attributes = if emitted == spans_in_resource {
                    std::mem::take(&mut resource_attrs)
                } else {
                    resource_attrs.clone()
                };
                for kv in span.attributes {
                    let value = kv.value.map(any_value_into_string).unwrap_or_default();
                    attributes.insert(kv.key, value);
                }

                let start = span.start_time_unix_nano as i64;
                let end = if span.end_time_unix_nano == 0 {
                    None
                } else {
                    Some(span.end_time_unix_nano as i64)
                };
                let duration = end.map(|e| (e - start).max(0));

                let (status_code, status_text, status_message) = match span.status {
                    Some(s) => (
                        Some(s.code),
                        status_text(s.code),
                        Some(s.message).filter(|m| !m.is_empty()),
                    ),
                    None => (None, None, None),
                };

                out.push(SpanRecord {
                    trace_id: bytes_to_hex(&span.trace_id),
                    span_id: bytes_to_hex(&span.span_id),
                    parent_span_id: bytes_to_hex_opt(&span.parent_span_id),
                    name: Some(span.name).filter(|n| !n.is_empty()),
                    kind: Some(span.kind),
                    kind_text: kind_text(span.kind),
                    start_time_nanos: start,
                    end_time_nanos: end,
                    duration_nanos: duration,
                    status_code,
                    status_text,
                    status_message,
                    scope_name: scope_name.clone(),
                    events: events_json(span.events),
                    links: links_json(span.links),
                    attributes,
                });
            }
        }
    }

    out
}

/// OTLP `SpanKind` enum → display text. Unspecified/unknown → None.
fn kind_text(kind: i32) -> Option<String> {
    match kind {
        1 => Some("INTERNAL".into()),
        2 => Some("SERVER".into()),
        3 => Some("CLIENT".into()),
        4 => Some("PRODUCER".into()),
        5 => Some("CONSUMER".into()),
        _ => None,
    }
}

/// OTLP `StatusCode` enum → display text.
fn status_text(code: i32) -> Option<String> {
    match code {
        0 => Some("UNSET".into()),
        1 => Some("OK".into()),
        2 => Some("ERROR".into()),
        _ => None,
    }
}

fn events_json(events: Vec<opentelemetry_proto::tonic::trace::v1::span::Event>) -> Option<String> {
    if events.is_empty() {
        return None;
    }
    let items: Vec<EventJson> = events
        .into_iter()
        .map(|e| EventJson {
            name: e.name,
            time_unix_nano: e.time_unix_nano.to_string(),
            attributes: e
                .attributes
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

fn links_json(links: Vec<opentelemetry_proto::tonic::trace::v1::span::Link>) -> Option<String> {
    if links.is_empty() {
        return None;
    }
    let items: Vec<LinkJson> = links
        .into_iter()
        .map(|l| LinkJson {
            trace_id: bytes_to_hex(&l.trace_id),
            span_id: bytes_to_hex(&l.span_id),
            attributes: l
                .attributes
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

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{
        span::Event, ResourceSpans, ScopeSpans, Span, Status,
    };

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

    fn request_with(span: Span, resource_attrs: Vec<KeyValue>) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: resource_attrs,
                    dropped_attributes_count: 0,
                }),
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![span],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
    }

    fn base_span() -> Span {
        Span {
            trace_id: vec![0xAB; 16],
            span_id: vec![0xCD; 8],
            trace_state: String::new(),
            parent_span_id: vec![0xEF; 8],
            flags: 0,
            name: "charge.card".to_string(),
            kind: 3, // CLIENT
            start_time_unix_nano: 1_000,
            end_time_unix_nano: 1_500,
            attributes: vec![kv("http.status", "504")],
            dropped_attributes_count: 0,
            events: vec![Event {
                time_unix_nano: 1_490,
                name: "exception".to_string(),
                attributes: vec![kv("exception.message", "timeout")],
                dropped_attributes_count: 0,
            }],
            dropped_events_count: 0,
            links: vec![],
            dropped_links_count: 0,
            status: Some(Status {
                message: "gateway timeout".to_string(),
                code: 2,
            }),
        }
    }

    #[test]
    fn maps_core_span_fields() {
        let req = request_with(base_span(), vec![kv("service.name", "payments")]);
        let spans = otlp_traces_to_spans(req);
        assert_eq!(spans.len(), 1);
        let s = &spans[0];

        assert_eq!(s.trace_id, "abababababababababababababababab");
        assert_eq!(s.span_id, "cdcdcdcdcdcdcdcd");
        assert_eq!(s.parent_span_id.as_deref(), Some("efefefefefefefef"));
        assert_eq!(s.name.as_deref(), Some("charge.card"));
        assert_eq!(s.kind_text.as_deref(), Some("CLIENT"));
        assert_eq!(s.start_time_nanos, 1_000);
        assert_eq!(s.end_time_nanos, Some(1_500));
        assert_eq!(s.duration_nanos, Some(500));
        assert_eq!(s.status_text.as_deref(), Some("ERROR"));
        assert_eq!(s.status_message.as_deref(), Some("gateway timeout"));
        assert_eq!(
            s.attributes.get("service.name"),
            Some(&"payments".to_string())
        );
        assert_eq!(s.attributes.get("http.status"), Some(&"504".to_string()));
        assert!(s.events.as_deref().unwrap().contains("exception"));
        assert!(s.links.is_none());
    }

    #[test]
    fn missing_parent_and_end_time_are_none() {
        let mut span = base_span();
        span.parent_span_id = vec![];
        span.end_time_unix_nano = 0;
        span.status = None;
        let req = request_with(span, vec![]);
        let s = &otlp_traces_to_spans(req)[0];
        assert_eq!(s.parent_span_id, None);
        assert_eq!(s.end_time_nanos, None);
        assert_eq!(s.duration_nanos, None);
        assert_eq!(s.status_code, None);
    }
}

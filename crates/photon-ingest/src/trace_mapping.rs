//! Pure OTLP → [`SpanRecord`] mapping. No I/O, fully unit-testable.

use crate::otlp_value::{any_value_into_string, bytes_to_hex, bytes_to_hex_opt};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use photon_core::span_record::{SpanBatchBuilder, SpanFixed, SpanRecord};
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

                // OTLP nanos arrive as untrusted u64 — clamp rather than `as i64`, which wraps
                // negative for values > i64::MAX. Duration is computed via `checked_sub` +
                // `.max(0)` so a hostile/buggy `end < start` never underflows/panics (debug
                // overflow-checks) and never stores a negative duration.
                let start = i64::try_from(span.start_time_unix_nano).unwrap_or(i64::MAX);
                let end = if span.end_time_unix_nano == 0 {
                    None
                } else {
                    Some(i64::try_from(span.end_time_unix_nano).unwrap_or(i64::MAX))
                };
                let duration = end.map(|e| e.checked_sub(start).map(|d| d.max(0)).unwrap_or(0));

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

/// Total span count across every resource/scope group, used to pre-size the `SpanBatchBuilder`
/// on the streaming ingest path so its column builders don't pay for geometric reallocation.
/// Shared by both trace receivers (`grpc_trace`, `trace_http`); mirrors the metrics port's
/// centralized `metrics_mapping::estimate_rows`.
pub(crate) fn estimate_rows(req: &ExportTraceServiceRequest) -> usize {
    req.resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .map(|ss| ss.spans.len())
        .sum()
}

/// Stream an OTLP request straight into the Arrow span builder — the hot ingest path. No
/// intermediate `Vec<SpanRecord>` and no per-span `BTreeMap`: for each resource group the
/// resource attributes are owned once, then chained (as borrowed pairs) with each span's own
/// attributes and appended directly. Same output batch as `otlp_traces_to_spans` + `append`,
/// proven equal in tests. Applies the identical untrusted-nanos clamping as the reference path.
pub fn otlp_traces_into_builder(req: ExportTraceServiceRequest, builder: &mut SpanBatchBuilder) {
    for resource_spans in req.resource_spans {
        // Own the resource attrs once per group (OTLP moves the strings out).
        let resource_attrs: Vec<(String, String)> = resource_spans
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

        for scope_spans in resource_spans.scope_spans {
            let scope_name = scope_spans.scope.map(|s| s.name).filter(|n| !n.is_empty());
            for span in scope_spans.spans {
                // Own this span's attrs (distinct per span).
                let span_attrs: Vec<(String, String)> = span
                    .attributes
                    .into_iter()
                    .map(|kv| {
                        (
                            kv.key,
                            kv.value.map(any_value_into_string).unwrap_or_default(),
                        )
                    })
                    .collect();

                // OTLP nanos arrive as untrusted u64 — clamp rather than `as i64`, which wraps
                // negative for values > i64::MAX. Duration is computed via `checked_sub` +
                // `.max(0)` so a hostile/buggy `end < start` never underflows/panics (debug
                // overflow-checks) and never stores a negative duration. IDENTICAL to
                // `otlp_traces_to_spans`.
                let start = i64::try_from(span.start_time_unix_nano).unwrap_or(i64::MAX);
                let end = if span.end_time_unix_nano == 0 {
                    None
                } else {
                    Some(i64::try_from(span.end_time_unix_nano).unwrap_or(i64::MAX))
                };
                let duration = end.map(|e| e.checked_sub(start).map(|d| d.max(0)).unwrap_or(0));

                let (status_code, status_text, status_message) = match span.status {
                    Some(s) => (
                        Some(s.code),
                        status_text(s.code),
                        Some(s.message).filter(|m| !m.is_empty()),
                    ),
                    None => (None, None, None),
                };

                // Owned locals so `SpanFixed` can borrow &str from them for the row.
                let trace_id = bytes_to_hex(&span.trace_id);
                let span_id = bytes_to_hex(&span.span_id);
                let parent_span_id = bytes_to_hex_opt(&span.parent_span_id);
                let name = Some(span.name).filter(|n| !n.is_empty());
                let kind_text = kind_text(span.kind);
                let events = events_json(span.events);
                let links = links_json(span.links);

                let fixed = SpanFixed {
                    trace_id: &trace_id,
                    span_id: &span_id,
                    parent_span_id: parent_span_id.as_deref(),
                    name: name.as_deref(),
                    kind: Some(span.kind),
                    kind_text: kind_text.as_deref(),
                    start_time_nanos: start,
                    end_time_nanos: end,
                    duration_nanos: duration,
                    status_code,
                    status_text: status_text.as_deref(),
                    status_message: status_message.as_deref(),
                    scope_name: scope_name.as_deref(),
                    events: events.as_deref(),
                    links: links.as_deref(),
                };
                // Reproduce `BTreeMap` iteration semantics without a per-span `BTreeMap`: the
                // attrs handed to `append_streaming` must be sorted by key ascending and deduped
                // to exactly one entry per key, span-wins on a duplicate (the reference/append
                // path merges into a resource-then-span `BTreeMap`, so a repeated key keeps the
                // span value and keys come out alphabetically).
                //
                // Collect the merged (resource-then-span) pairs as borrowed &str tagged with
                // their insertion index, then sort by (key asc, index desc) and `dedup_by` key.
                // `dedup_by` keeps the FIRST of each equal-key run; with index descending that
                // first is the highest index = the span's value — so duplicates collapse to one
                // entry, span beats resource, keys ascending. No String clones: `merged` borrows
                // from the already-owned `resource_attrs`/`span_attrs` Vecs.
                let mut merged: Vec<(usize, &str, &str)> = resource_attrs
                    .iter()
                    .chain(span_attrs.iter())
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
    use opentelemetry_proto::tonic::common::v1::{
        any_value::Value, AnyValue, InstrumentationScope, KeyValue,
    };
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

    /// F11: a hostile/buggy exporter can send `end_time_unix_nano < start_time_unix_nano`.
    /// The naive `end - start` computation underflows i64 and panics under debug
    /// overflow-checks (the profile `cargo test` runs with); in release it would silently
    /// store a garbage negative duration. Neither is acceptable — duration must be
    /// non-negative and mapping must never panic on untrusted input.
    #[test]
    fn end_before_start_yields_non_negative_duration_without_panicking() {
        let mut span = base_span();
        span.start_time_unix_nano = 2_000;
        span.end_time_unix_nano = 1_000; // end strictly before start
        let req = request_with(span, vec![]);
        let s = &otlp_traces_to_spans(req)[0];
        assert_eq!(s.start_time_nanos, 2_000);
        assert_eq!(s.end_time_nanos, Some(1_000));
        assert_eq!(s.duration_nanos, Some(0));
    }

    /// F11 — the actual underflow-panic case. `start` sits just below the `as i64` wrap
    /// boundary (2^63 - 1, stays positive when cast) while `end` sits exactly at the
    /// boundary (2^63, wraps to `i64::MIN` under a raw `as i64` cast). The pre-fix code
    /// computed `end - start` directly on those wrapped values: `i64::MIN - i64::MAX`
    /// underflows i64 and panics under debug overflow-checks (the profile `cargo test`
    /// runs with) — this is the exact hostile-input crash F11 hardens against. Confirmed
    /// RED (panics) against the pre-fix `as i64` + direct-subtract code, GREEN after the
    /// `i64::try_from(..).unwrap_or(i64::MAX)` clamp + `checked_sub` duration fix.
    #[test]
    fn boundary_wrap_end_before_start_does_not_underflow_or_panic() {
        let mut span = base_span();
        span.start_time_unix_nano = i64::MAX as u64; // 2^63 - 1: stays positive under `as i64`
        span.end_time_unix_nano = (i64::MAX as u64) + 1; // 2^63: wraps to i64::MIN under `as i64`
        let req = request_with(span, vec![]);
        let s = &otlp_traces_to_spans(req)[0];
        assert_eq!(s.start_time_nanos, i64::MAX);
        assert_eq!(s.end_time_nanos, Some(i64::MAX)); // clamped, not wrapped to i64::MIN
        assert_eq!(s.duration_nanos, Some(0));
    }

    /// F11: `u64::MAX` nanos must clamp to `i64::MAX`, not wrap around to a negative i64 via
    /// `as i64`. Also exercises the same start/end pair through the duration computation to
    /// confirm it stays non-negative and panic-free at the extreme.
    #[test]
    fn u64_max_timestamps_clamp_to_i64_max_instead_of_wrapping_negative() {
        let mut span = base_span();
        span.start_time_unix_nano = u64::MAX;
        span.end_time_unix_nano = u64::MAX;
        let req = request_with(span, vec![]);
        let s = &otlp_traces_to_spans(req)[0];
        assert_eq!(s.start_time_nanos, i64::MAX);
        assert_eq!(s.end_time_nanos, Some(i64::MAX));
        assert_eq!(s.duration_nanos, Some(0));
    }

    // --- Streaming-path equivalence gate ------------------------------------------------------
    //
    // The streaming `otlp_traces_into_builder` (the production ingest path) must produce a
    // byte-identical Arrow batch to the reference `otlp_traces_to_spans` + `SpanBatchBuilder::
    // append` path. These mirror the two log-mapping equivalence tests
    // (`into_builder_matches_to_records_then_build` / `..._sorts_and_dedups_attrs_record_wins`).

    /// Build the reference span batch (map → append each → finish) and the streaming batch
    /// (`otlp_traces_into_builder`) from the SAME request, then assert they are byte-identical.
    /// The reference path is independently asserted correct by the direct-value tests above; the
    /// equality here proves the streaming path reproduces it exactly (attr precedence,
    /// promoted-vs-map routing, timestamp clamping, events/links JSON, ...).
    fn assert_streaming_matches_reference(req: ExportTraceServiceRequest, promoted: &[String]) {
        use photon_core::span_record::SpanBatchBuilder;
        use photon_core::span_schema::SpanSchema;
        let schema = SpanSchema::new(promoted);

        // Reference batch via the map+append path.
        let mut ref_b = SpanBatchBuilder::with_capacity(&schema, 8);
        for s in otlp_traces_to_spans(req.clone()) {
            ref_b.append(&s);
        }
        let reference = ref_b.finish().unwrap();

        // Streaming path.
        let mut b = SpanBatchBuilder::with_capacity(&schema, 8);
        otlp_traces_into_builder(req, &mut b);
        let streamed = b.finish().unwrap();

        // `RecordBatch: PartialEq` does a typed, column-by-column comparison — assert the
        // "byte-identical" claim directly rather than comparing Debug strings. This subsumes the
        // `num_rows` check (unequal row counts compare unequal).
        assert_eq!(streamed, reference);
    }

    /// Mirror of the log test: a representative request (resource attrs, span attrs, events,
    /// status) built both ways yields identical batches.
    #[test]
    fn into_builder_matches_to_records_then_build() {
        let req = request_with(base_span(), vec![kv("service.name", "payments")]);
        assert_streaming_matches_reference(req, &["service.name".to_string()]);
    }

    /// Mirror of `into_builder_sorts_and_dedups_attrs_record_wins`: a span whose attributes
    /// collide with the resource attrs. Locks the streaming path to `BTreeMap` semantics —
    /// attrs come out sorted ascending, one entry per key, SPAN wins on a duplicate:
    ///   (a) ≥3 non-promoted keys in NON-alphabetical OTLP order (`region`, `http.status`, `env`)
    ///       — locks the sort;
    ///   (b) a non-promoted key (`env`) present in BOTH resource and span with different values
    ///       — must collapse to ONE map entry, span value wins;
    ///   (c) a promoted key (`service.name`) present in BOTH with different values
    ///       — locks span-wins for the promoted column.
    #[test]
    fn into_builder_sorts_and_dedups_attrs_span_wins() {
        let mut span = base_span();
        span.attributes = vec![
            kv("region", "apac"),
            kv("http.status", "504"),
            kv("env", "span-env"),
            kv("service.name", "span-svc"),
        ];
        let req = request_with(
            span,
            vec![
                kv("service.name", "resource-svc"),
                kv("env", "resource-env"),
            ],
        );
        assert_streaming_matches_reference(req, &["service.name".to_string()]);
    }

    /// Empty request: no resource groups → zero-row batches, identical both ways.
    #[test]
    fn into_builder_matches_reference_on_empty_request() {
        let req = ExportTraceServiceRequest {
            resource_spans: vec![],
        };
        assert_streaming_matches_reference(req, &["service.name".to_string()]);
    }

    /// A span carrying no attributes at all (and no resource attrs) — the merged set is empty,
    /// so the promoted column and the map are both null/empty. Must match the reference.
    #[test]
    fn into_builder_matches_reference_for_span_without_attributes() {
        let mut span = base_span();
        span.attributes = vec![];
        let req = request_with(span, vec![]);
        assert_streaming_matches_reference(req, &["service.name".to_string()]);
    }

    /// Multiple resource groups, each with multiple scope groups and per-scope scope names,
    /// including a span that overrides the promoted `service.name` and a span with no attrs.
    /// Exercises the reference's per-resource `mem::take`-on-last-span path (across scopes) and
    /// proves the streaming path — which re-borrows the owned resource attrs per span — matches.
    #[test]
    fn into_builder_matches_reference_across_multiple_groups() {
        fn scope_span(scope: &str, spans: Vec<Span>) -> ScopeSpans {
            ScopeSpans {
                scope: Some(InstrumentationScope {
                    name: scope.to_string(),
                    version: String::new(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                spans,
                schema_url: String::new(),
            }
        }
        let mut s1 = base_span();
        s1.attributes = vec![kv("k", "1")];
        let mut s2 = base_span();
        s2.name = "second".to_string();
        s2.attributes = vec![kv("k", "2"), kv("service.name", "override")];
        let mut s3 = base_span();
        s3.attributes = vec![];

        let req = ExportTraceServiceRequest {
            resource_spans: vec![
                ResourceSpans {
                    resource: Some(Resource {
                        attributes: vec![kv("service.name", "svc-a"), kv("region", "us")],
                        dropped_attributes_count: 0,
                    }),
                    scope_spans: vec![
                        scope_span("scope-1", vec![s1]),
                        scope_span("scope-2", vec![s2]),
                    ],
                    schema_url: String::new(),
                },
                ResourceSpans {
                    resource: Some(Resource {
                        attributes: vec![kv("service.name", "svc-b")],
                        dropped_attributes_count: 0,
                    }),
                    scope_spans: vec![scope_span("scope-3", vec![s3])],
                    schema_url: String::new(),
                },
            ],
        };
        assert_streaming_matches_reference(req, &["service.name".to_string()]);
    }

    /// The streaming path must apply the IDENTICAL untrusted-nanos clamping as the reference:
    /// `u64::MAX` → `i64::MAX`, `end < start` → duration 0, and the `as i64` wrap boundary
    /// (start = 2^63-1, end = 2^63 → i64::MIN under a raw cast). Proving streaming == reference
    /// (whose clamping is asserted correct by the direct tests above) proves streaming clamps too.
    #[test]
    fn into_builder_clamps_timestamps_identically() {
        let mut span = base_span();
        span.start_time_unix_nano = u64::MAX;
        span.end_time_unix_nano = u64::MAX;
        assert_streaming_matches_reference(
            request_with(span, vec![]),
            &["service.name".to_string()],
        );

        let mut span = base_span();
        span.start_time_unix_nano = 2_000;
        span.end_time_unix_nano = 1_000; // end strictly before start
        assert_streaming_matches_reference(
            request_with(span, vec![]),
            &["service.name".to_string()],
        );

        let mut span = base_span();
        span.start_time_unix_nano = i64::MAX as u64; // 2^63-1: stays positive under `as i64`
        span.end_time_unix_nano = (i64::MAX as u64) + 1; // 2^63: wraps to i64::MIN under `as i64`
        assert_streaming_matches_reference(
            request_with(span, vec![]),
            &["service.name".to_string()],
        );
    }
}

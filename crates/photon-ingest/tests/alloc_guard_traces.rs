//! Deterministic allocation guard for the traces decode→map→build pipeline — the spans mirror
//! of `alloc_guard.rs`. A counting global allocator records alloc count + bytes; we snapshot
//! before/after the measured section and diff, so only the pipeline's allocations are counted
//! (not test-harness startup).
//!
//! This proves the streaming span path (`otlp_traces_into_builder`) decodes OTLP straight into
//! the pre-sized Arrow builder with no intermediate `Vec<SpanRecord>` and no per-span
//! `BTreeMap` — the same win the logs guard locks for `otlp_logs_into_builder`. Own separate
//! test binary (its own `#[global_allocator]`) so its counters never mix with the logs guard's.

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use photon_core::span_record::SpanBatchBuilder;
use photon_core::span_schema::SpanSchema;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(layout.size() as u64, Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const ROWS: usize = 1_000;
const RESOURCE_ATTRS: usize = 4;
const ATTRS_PER_SPAN: usize = 8;
// Regression ceiling. The streaming span path measures ~32.2k allocs (~32.2/span; see the printed
// baseline) — the bulk is prost decode (a String per attr/name/id field, an inherent floor) plus
// the per-span borrowed-merge scratch Vecs; there is NO per-span `BTreeMap` and NO
// `Vec<SpanRecord>`. 36k guards regression while staying comfortably above the baseline.
const MAX_ALLOCS: u64 = 36_000;

fn kv(key: String, value: String) -> KeyValue {
    KeyValue {
        key,
        value: Some(AnyValue {
            value: Some(Value::StringValue(value)),
        }),
    }
}

/// One resource group carrying `ROWS` spans, protobuf-encoded — the spans analogue of
/// `benches::fixture::logs_request_bytes`. Deterministic content (no RNG) so runs compare.
fn traces_request_bytes() -> Vec<u8> {
    let mut resource_kvs = vec![
        kv("service.name".into(), "checkout".into()),
        kv("host.name".into(), "node-42".into()),
    ];
    for i in resource_kvs.len()..RESOURCE_ATTRS {
        resource_kvs.push(kv(format!("res.attr.{i}"), format!("res-value-{i}")));
    }

    let mut spans = Vec::with_capacity(ROWS);
    for r in 0..ROWS {
        let mut attrs = Vec::with_capacity(ATTRS_PER_SPAN);
        for a in 0..ATTRS_PER_SPAN {
            attrs.push(kv(format!("http.attr.{a}"), format!("value-{r}-{a}")));
        }
        let start = 1_700_000_000_000_000_000u64 + r as u64;
        spans.push(Span {
            trace_id: vec![0xAB; 16],
            span_id: vec![0xCD; 8],
            trace_state: String::new(),
            parent_span_id: vec![0xEF; 8],
            flags: 0,
            name: format!("op-{}", r % 32),
            kind: 3, // CLIENT
            start_time_unix_nano: start,
            end_time_unix_nano: start + (r as u64 % 250),
            attributes: attrs,
            dropped_attributes_count: 0,
            events: vec![],
            dropped_events_count: 0,
            links: vec![],
            dropped_links_count: 0,
            status: Some(Status {
                message: String::new(),
                code: 1, // OK
            }),
        });
    }

    let req = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: resource_kvs,
                dropped_attributes_count: 0,
            }),
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    prost::Message::encode_to_vec(&req)
}

#[test]
fn alloc_count_is_recorded_and_bounded() {
    // Build the fixture BEFORE snapshotting so its allocations aren't counted.
    let bytes = traces_request_bytes();
    let schema = SpanSchema::new(&["service.name".to_string(), "host.name".to_string()]);

    let a0 = ALLOCS.load(Relaxed);
    let b0 = BYTES.load(Relaxed);

    // The measured section: exactly what the trace ingest handler does per request — the
    // streaming path, decoding OTLP straight into the pre-sized Arrow builder with no
    // intermediate `Vec<SpanRecord>` and no per-span `BTreeMap`.
    let req: ExportTraceServiceRequest = prost::Message::decode(&bytes[..]).unwrap();
    let mut builder = SpanBatchBuilder::with_capacity(&schema, ROWS);
    photon_ingest::otlp_traces_into_builder(req, &mut builder);
    let batch = builder.finish().unwrap();
    std::hint::black_box(&batch);

    let allocs = ALLOCS.load(Relaxed) - a0;
    let alloc_bytes = BYTES.load(Relaxed) - b0;

    // Printed with `--nocapture`; this number is the streaming-span baseline / proof.
    println!(
        "[alloc-guard-traces] {ROWS} rows x {ATTRS_PER_SPAN} attrs: {allocs} allocations, \
         {alloc_bytes} bytes ({:.1} allocs/span)",
        allocs as f64 / ROWS as f64
    );

    assert!(
        allocs < MAX_ALLOCS,
        "decode→map→build allocated {allocs} times (ceiling {MAX_ALLOCS}) — regression?"
    );
    assert_eq!(batch.num_rows(), ROWS);
}

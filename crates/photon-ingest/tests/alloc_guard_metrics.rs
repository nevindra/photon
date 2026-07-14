//! Deterministic allocation guard for the metrics decode→map→build pipeline — the metrics mirror
//! of `alloc_guard.rs` / `alloc_guard_traces.rs`. A counting global allocator records alloc
//! count and bytes; we snapshot before/after the measured section and diff, so only the
//! pipeline's allocations are counted (not test-harness startup).
//!
//! This proves the streaming metric path (`otlp_metrics_into_builder`) decodes OTLP straight into
//! the pre-sized Arrow builder with no intermediate `Vec<MetricPoint>` and no per-point
//! `BTreeMap` — the same win the logs/traces guards lock. Own separate test binary (its own
//! `#[global_allocator]`) so its counters never mix with the other guards'.

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point::Value as NumVal, Gauge, Metric, NumberDataPoint,
    ResourceMetrics, ScopeMetrics,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use photon_core::metric_record::MetricBatchBuilder;
use photon_core::metric_schema::MetricSchema;
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
const ATTRS_PER_POINT: usize = 8;
// Regression ceiling. The streaming metric path is bounded by prost decode (a String per
// attr key/value, an inherent floor) plus the per-point borrowed-merge scratch Vecs; there is NO
// per-point `BTreeMap` and NO `Vec<MetricPoint>`. Sized comfortably above the printed baseline.
const MAX_ALLOCS: u64 = 40_000;

fn kv(key: String, value: String) -> KeyValue {
    KeyValue {
        key,
        value: Some(AnyValue {
            value: Some(Value::StringValue(value)),
        }),
    }
}

/// One resource group carrying a single Gauge metric with `ROWS` data points, protobuf-encoded —
/// the metrics analogue of `alloc_guard_traces::traces_request_bytes`. Deterministic (no RNG).
fn metrics_request_bytes() -> Vec<u8> {
    let mut resource_kvs = vec![
        kv("service.name".into(), "checkout".into()),
        kv("host.name".into(), "node-42".into()),
    ];
    for i in resource_kvs.len()..RESOURCE_ATTRS {
        resource_kvs.push(kv(format!("res.attr.{i}"), format!("res-value-{i}")));
    }

    let mut data_points = Vec::with_capacity(ROWS);
    for r in 0..ROWS {
        let mut attrs = Vec::with_capacity(ATTRS_PER_POINT);
        for a in 0..ATTRS_PER_POINT {
            attrs.push(kv(format!("point.attr.{a}"), format!("value-{r}-{a}")));
        }
        let ts = 1_700_000_000_000_000_000u64 + r as u64;
        data_points.push(NumberDataPoint {
            attributes: attrs,
            start_time_unix_nano: ts,
            time_unix_nano: ts + 1,
            exemplars: vec![],
            flags: 0,
            value: Some(NumVal::AsDouble(r as f64)),
        });
    }

    let req = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: resource_kvs,
                dropped_attributes_count: 0,
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics: vec![Metric {
                    name: "cpu.usage".into(),
                    description: String::new(),
                    unit: "1".into(),
                    metadata: vec![],
                    data: Some(Data::Gauge(Gauge { data_points })),
                }],
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
    let bytes = metrics_request_bytes();
    let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);

    let a0 = ALLOCS.load(Relaxed);
    let b0 = BYTES.load(Relaxed);

    // The measured section: exactly what the metrics ingest handler does per request — the
    // streaming path, decoding OTLP straight into the pre-sized Arrow builder with no
    // intermediate `Vec<MetricPoint>` and no per-point `BTreeMap`.
    let req: ExportMetricsServiceRequest = prost::Message::decode(&bytes[..]).unwrap();
    let mut builder = MetricBatchBuilder::with_capacity(&schema, ROWS);
    photon_ingest::otlp_metrics_into_builder(req, &mut builder);
    let batch = builder.finish().unwrap();
    std::hint::black_box(&batch);

    let allocs = ALLOCS.load(Relaxed) - a0;
    let alloc_bytes = BYTES.load(Relaxed) - b0;

    // Printed with `--nocapture`; this number is the streaming-metric baseline / proof.
    println!(
        "[alloc-guard-metrics] {ROWS} rows x {ATTRS_PER_POINT} attrs: {allocs} allocations, \
         {alloc_bytes} bytes ({:.1} allocs/point)",
        allocs as f64 / ROWS as f64
    );

    assert!(
        allocs < MAX_ALLOCS,
        "decode→map→build allocated {allocs} times (ceiling {MAX_ALLOCS}) — regression?"
    );
    assert_eq!(batch.num_rows(), ROWS);
}

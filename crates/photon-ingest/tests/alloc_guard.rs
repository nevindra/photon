//! Deterministic allocation guard for the logs decode→map→build pipeline. A counting global
//! allocator records alloc count + bytes; we snapshot before/after the measured section and
//! diff, so only the pipeline's allocations are counted (not test-harness startup).
//!
//! Phase A: RECORD the baseline number (printed) and assert a *generous* ceiling as a
//! regression guard. Phase B (F1) tightens `MAX_ALLOCS` to prove the ~10x reduction.

#[path = "../benches/fixture.rs"]
mod fixture;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use photon_core::record::RecordBatchBuilder;
use photon_core::schema::LogSchema;
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
const ATTRS_PER_RECORD: usize = 8;
// Phase-B2 ceiling locking in the WS1-F1 win: the streaming path measures ~28.2k allocs
// (28.2/record) vs the ~37.2/record HEAD baseline — the BTreeMap + Vec<LogRecord> elimination.
// (The remainder is prost decode, which allocates a String per attr field and dominates —
// an inherent floor, not addressable by F1.) 30k guards regression while staying under baseline.
const MAX_ALLOCS: u64 = 30_000;

#[test]
fn alloc_count_is_recorded_and_bounded() {
    // Build the fixture BEFORE snapshotting so its allocations aren't counted.
    let bytes = fixture::logs_request_bytes(ROWS, RESOURCE_ATTRS, ATTRS_PER_RECORD);
    let schema = LogSchema::new(&["service.name".to_string(), "host.name".to_string()]);

    let a0 = ALLOCS.load(Relaxed);
    let b0 = BYTES.load(Relaxed);

    // The measured section: exactly what the ingest handler does per request — the streaming
    // path (WS1-F1), which decodes OTLP straight into the pre-sized Arrow builder with no
    // intermediate `Vec<LogRecord>` and no per-record `BTreeMap`.
    let req: ExportLogsServiceRequest = prost::Message::decode(&bytes[..]).unwrap();
    let mut builder = RecordBatchBuilder::with_capacity(&schema, ROWS);
    photon_ingest::otlp_logs_into_builder(req, &mut builder);
    let batch = builder.finish().unwrap();
    std::hint::black_box(&batch);

    let allocs = ALLOCS.load(Relaxed) - a0;
    let alloc_bytes = BYTES.load(Relaxed) - b0;

    // Printed with `--nocapture`; this number is the F1 baseline / proof.
    println!(
        "[alloc-guard] {ROWS} rows x {ATTRS_PER_RECORD} attrs: {allocs} allocations, \
         {alloc_bytes} bytes ({:.1} allocs/record)",
        allocs as f64 / ROWS as f64
    );

    assert!(
        allocs < MAX_ALLOCS,
        "decode→map→build allocated {allocs} times (ceiling {MAX_ALLOCS}) — regression?"
    );
    assert_eq!(batch.num_rows(), ROWS);
}

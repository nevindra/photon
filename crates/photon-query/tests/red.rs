//! Integration test for `SpanQueryEngine::red_metrics`: write a known spans corpus to a temp hot
//! dir as real `data-spans/*.parquet` + `.idx` sidecars + a spans manifest (the same artifacts the
//! `SpanCompactor` produces), then assert the RED rollups (counts, error counts, monotone
//! percentiles, per-service rollup) over the pruned + DataFusion-read path.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use tempfile::TempDir;

use photon_core::manifest::{FileEntry, Manifest, SPANS_MANIFEST_OBJECT_PATH};
use photon_core::segment::SegmentId;
use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
use photon_core::span_schema::SpanSchema;
use photon_index::SkipIndex;
use photon_query::{RedGroup, SpanQueryEngine, SpanQueryRequest, SpanSort};
use photon_storage::Storage;

fn schema() -> SpanSchema {
    SpanSchema::new(&["service.name".to_string()])
}

#[allow(clippy::too_many_arguments)]
fn span(
    trace: &str,
    span_id: &str,
    service: &str,
    name: &str,
    start: i64,
    duration: Option<i64>,
    status: Option<i32>,
) -> SpanRecord {
    let mut attributes = BTreeMap::new();
    attributes.insert("service.name".to_string(), service.to_string());
    SpanRecord {
        trace_id: trace.to_string(),
        span_id: span_id.to_string(),
        name: Some(name.to_string()),
        start_time_nanos: start,
        duration_nanos: duration,
        status_code: status,
        attributes,
        ..Default::default()
    }
}

fn build_batch(records: &[SpanRecord], schema: &SpanSchema) -> RecordBatch {
    let mut builder = SpanBatchBuilder::new(schema);
    for r in records {
        builder.append(r);
    }
    builder.finish().unwrap()
}

fn write_parquet(path: &Path, batch: &RecordBatch) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(batch).unwrap();
    writer.close().unwrap();
}

fn write_idx(path: &Path, batch: &RecordBatch) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let idx = SkipIndex::build_spans(batch).unwrap();
    fs::write(path, idx.to_bytes()).unwrap();
}

fn entry_from(records: &[SpanRecord], seg: SegmentId) -> FileEntry {
    let min_ts = records.iter().map(|r| r.start_time_nanos).min().unwrap();
    let max_ts = records.iter().map(|r| r.start_time_nanos).max().unwrap();
    let services: Vec<&str> = records
        .iter()
        .map(|r| r.attributes.get("service.name").unwrap().as_str())
        .collect();
    FileEntry {
        path: Storage::parquet_path_spans(seg),
        segment_id: seg,
        min_ts_nanos: min_ts,
        max_ts_nanos: max_ts,
        min_service: services.iter().min().unwrap().to_string(),
        max_service: services.iter().max().unwrap().to_string(),
        row_count: records.len() as u64,
        durable: false,
        attribute_keys: Vec::new(),
        bytes: 0,
    }
}

fn write_segment(root: &Path, seg: SegmentId, records: &[SpanRecord]) -> FileEntry {
    let batch = build_batch(records, &schema());
    write_parquet(&root.join(Storage::parquet_path_spans(seg)), &batch);
    write_idx(&root.join(Storage::index_path_spans(seg)), &batch);
    entry_from(records, seg)
}

fn write_manifest(root: &Path, entries: Vec<FileEntry>) {
    let mut m = Manifest::new();
    for e in entries {
        m.add(e);
    }
    let path = root.join(SPANS_MANIFEST_OBJECT_PATH);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, m.to_json().unwrap()).unwrap();
}

fn req() -> SpanQueryRequest {
    SpanQueryRequest {
        start_ts_nanos: 0,
        end_ts_nanos: i64::MAX,
        query: None,
        sort: SpanSort::Recent,
        limit: 0,
        offset: 0,
        projected_attributes: Vec::new(),
    }
}

/// A corpus across two segments:
/// - checkout/charge: 3 spans, 1 error, durations 100/200/300ms
/// - checkout/lookup: 2 spans, 0 error, durations 40/60ms
/// - payments/authorize: 2 spans, 2 errors, durations 500/700ms
fn write_corpus(root: &Path) {
    let seg0 = vec![
        span(
            "t1",
            "a1",
            "checkout",
            "charge",
            10,
            Some(100_000_000),
            Some(1),
        ),
        span(
            "t1",
            "a2",
            "checkout",
            "charge",
            20,
            Some(200_000_000),
            Some(2),
        ),
        span(
            "t2",
            "a3",
            "checkout",
            "charge",
            30,
            Some(300_000_000),
            Some(1),
        ),
        span(
            "t2",
            "a4",
            "checkout",
            "lookup",
            40,
            Some(40_000_000),
            Some(1),
        ),
    ];
    let seg1 = vec![
        span(
            "t3",
            "b1",
            "checkout",
            "lookup",
            50,
            Some(60_000_000),
            Some(1),
        ),
        span(
            "t3",
            "b2",
            "payments",
            "authorize",
            60,
            Some(500_000_000),
            Some(2),
        ),
        span(
            "t4",
            "b3",
            "payments",
            "authorize",
            70,
            Some(700_000_000),
            Some(2),
        ),
    ];
    let e0 = write_segment(root, SegmentId(0), &seg0);
    let e1 = write_segment(root, SegmentId(1), &seg1);
    write_manifest(root, vec![e0, e1]);
}

#[tokio::test]
async fn red_metrics_by_operation_over_disk_corpus() {
    let dir = TempDir::new().unwrap();
    write_corpus(dir.path());
    let engine = SpanQueryEngine::new(dir.path().to_path_buf(), schema()).unwrap();

    let rows = engine
        .red_metrics(
            req(),
            RedGroup::Operation,
            &std::collections::HashMap::new(),
            500,
        )
        .await
        .unwrap();

    let charge = rows
        .iter()
        .find(|r| r.service == "checkout" && r.operation.as_deref() == Some("charge"))
        .unwrap();
    assert_eq!(charge.count, 3);
    assert_eq!(charge.error_count, 1);
    assert!(charge.p50 <= charge.p90 && charge.p90 <= charge.p99);

    let authorize = rows
        .iter()
        .find(|r| r.service == "payments" && r.operation.as_deref() == Some("authorize"))
        .unwrap();
    assert_eq!(authorize.count, 2);
    assert_eq!(authorize.error_count, 2);

    // Ordered by count DESC: charge (3) must come before authorize (2).
    let idx = |svc: &str, op: &str| {
        rows.iter()
            .position(|r| r.service == svc && r.operation.as_deref() == Some(op))
            .unwrap()
    };
    assert!(idx("checkout", "charge") < idx("payments", "authorize"));
}

#[tokio::test]
async fn red_metrics_by_service_rolls_operations_up() {
    let dir = TempDir::new().unwrap();
    write_corpus(dir.path());
    let engine = SpanQueryEngine::new(dir.path().to_path_buf(), schema()).unwrap();

    let rows = engine
        .red_metrics(
            req(),
            RedGroup::Service,
            &std::collections::HashMap::new(),
            500,
        )
        .await
        .unwrap();

    let checkout = rows.iter().find(|r| r.service == "checkout").unwrap();
    assert_eq!(checkout.operation, None);
    assert_eq!(checkout.count, 5); // 3 charge + 2 lookup
    assert_eq!(checkout.error_count, 1);

    let payments = rows.iter().find(|r| r.service == "payments").unwrap();
    assert_eq!(payments.count, 2);
    assert_eq!(payments.error_count, 2);
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn red_metrics_empty_window_is_empty() {
    let dir = TempDir::new().unwrap();
    write_corpus(dir.path());
    let engine = SpanQueryEngine::new(dir.path().to_path_buf(), schema()).unwrap();

    let empty = SpanQueryRequest {
        start_ts_nanos: 1_000_000,
        end_ts_nanos: 2_000_000, // no spans start in this window (corpus starts are 10..70)
        ..req()
    };
    let rows = engine
        .red_metrics(
            empty,
            RedGroup::Operation,
            &std::collections::HashMap::new(),
            500,
        )
        .await
        .unwrap();
    assert!(rows.is_empty());
}

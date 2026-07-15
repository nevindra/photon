//! Integration tests for `SpanQueryEngine::get_trace`, building real spans fixtures
//! (Parquet + `.idx` skip index + spans manifest) with the `parquet`/`arrow` dev-deps.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::path::Path;

use arrow::array::{Array, StringArray};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use tempfile::TempDir;

use photon_core::manifest::{FileEntry, Manifest, SPANS_MANIFEST_OBJECT_PATH};
use photon_core::segment::SegmentId;
use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
use photon_core::span_schema::SpanSchema;
use photon_index::SkipIndex;
use photon_query::SpanQueryEngine;
use photon_storage::Storage;

fn schema() -> SpanSchema {
    SpanSchema::new(&["service.name".to_string()])
}

fn span(trace: &str, span_id: &str, service: &str, start: i64) -> SpanRecord {
    let mut attributes = BTreeMap::new();
    attributes.insert("service.name".to_string(), service.to_string());
    SpanRecord {
        trace_id: trace.to_string(),
        span_id: span_id.to_string(),
        name: Some("op".to_string()),
        start_time_nanos: start,
        end_time_nanos: Some(start + 1000),
        duration_nanos: Some(1000),
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

/// Write one spans segment (parquet + idx) under `root`, return its manifest entry.
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

/// Collect the `span_id` values from result batches.
fn span_ids(batches: &[RecordBatch]) -> HashSet<String> {
    let mut out = HashSet::new();
    for b in batches {
        let col = b
            .column_by_name("span_id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        for i in 0..col.len() {
            out.insert(col.value(i).to_string());
        }
    }
    out
}

#[tokio::test]
async fn get_trace_gathers_spans_across_files_and_excludes_other_traces() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // File A: trace t1 spans s1,s2 + trace t2 span x1. File B: trace t1 span s3.
    let a = write_segment(
        root,
        SegmentId(1),
        &[
            span("t1", "s1", "api", 100),
            span("t1", "s2", "db", 200),
            span("t2", "x1", "api", 300),
        ],
    );
    let b = write_segment(root, SegmentId(2), &[span("t1", "s3", "cache", 400)]);
    write_manifest(root, vec![a, b]);

    let engine = SpanQueryEngine::new(root.to_path_buf(), schema()).unwrap();
    let batches = engine.get_trace("t1", None).await.unwrap();
    let ids = span_ids(&batches);
    assert_eq!(
        ids,
        ["s1", "s2", "s3"].iter().map(|s| s.to_string()).collect()
    );
}

#[tokio::test]
async fn get_trace_unknown_id_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let a = write_segment(root, SegmentId(1), &[span("t1", "s1", "api", 100)]);
    write_manifest(root, vec![a]);

    let engine = SpanQueryEngine::new(root.to_path_buf(), schema()).unwrap();
    let batches = engine.get_trace("nope", None).await.unwrap();
    assert!(batches.iter().all(|b| b.num_rows() == 0));
}

#[tokio::test]
async fn get_trace_keeps_file_with_missing_idx() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // Write parquet + manifest but NO .idx sidecar → must be kept (conservative).
    let records = [span("t1", "s1", "api", 100)];
    let batch = build_batch(&records, &schema());
    write_parquet(
        &root.join(Storage::parquet_path_spans(SegmentId(1))),
        &batch,
    );
    write_manifest(root, vec![entry_from(&records, SegmentId(1))]);

    let engine = SpanQueryEngine::new(root.to_path_buf(), schema()).unwrap();
    let ids = span_ids(&engine.get_trace("t1", None).await.unwrap());
    assert!(ids.contains("s1"));
}

#[tokio::test]
async fn get_trace_empty_manifest_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let engine = SpanQueryEngine::new(tmp.path().to_path_buf(), schema()).unwrap();
    let batches = engine.get_trace("t1", Some(100)).await.unwrap();
    assert!(batches.is_empty());
}

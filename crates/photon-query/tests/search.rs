//! Integration tests for `photon-query`, building real fixtures with the `parquet` + `arrow`
//! dev-deps and the actual `photon_core` / `photon_index` types.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use arrow::array::{Array, Int64Array, StringArray, TimestampNanosecondArray};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use tempfile::TempDir;

use photon_core::manifest::{FileEntry, Manifest, MANIFEST_OBJECT_PATH};
use photon_core::record::{LogRecord, RecordBatchBuilder};
use photon_core::schema::LogSchema;
use photon_core::segment::SegmentId;
use photon_index::SkipIndex;
use photon_query::{QueryEngine, QueryRequest};
use photon_storage::Storage;

// ---- fixture helpers -------------------------------------------------------

fn schema() -> LogSchema {
    LogSchema::new(&["service.name".to_string()])
}

fn record(ts: i64, service: &str, body: &str) -> LogRecord {
    let mut attributes = BTreeMap::new();
    attributes.insert("service.name".to_string(), service.to_string());
    LogRecord {
        timestamp_nanos: ts,
        body: Some(body.to_string()),
        attributes,
        ..Default::default()
    }
}

fn record_sev(ts: i64, service: &str, body: &str, severity: i32) -> LogRecord {
    LogRecord {
        severity_number: Some(severity),
        ..record(ts, service, body)
    }
}

fn build_batch(records: &[LogRecord], schema: &LogSchema) -> RecordBatch {
    let mut builder = RecordBatchBuilder::new(schema);
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

fn write_idx(path: &Path, batch: &RecordBatch, schema: &LogSchema) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let idx = SkipIndex::build(batch, schema).unwrap();
    fs::write(path, idx.to_bytes()).unwrap();
}

fn entry_from(records: &[LogRecord], seg: SegmentId) -> FileEntry {
    let min_ts = records.iter().map(|r| r.timestamp_nanos).min().unwrap();
    let max_ts = records.iter().map(|r| r.timestamp_nanos).max().unwrap();
    let services: Vec<&str> = records
        .iter()
        .map(|r| r.attributes.get("service.name").unwrap().as_str())
        .collect();
    let min_service = services.iter().min().unwrap().to_string();
    let max_service = services.iter().max().unwrap().to_string();
    FileEntry {
        path: Storage::parquet_path(seg),
        segment_id: seg,
        min_ts_nanos: min_ts,
        max_ts_nanos: max_ts,
        min_service,
        max_service,
        row_count: records.len() as u64,
        durable: false,
        attribute_keys: Vec::new(),
        bytes: 0,
    }
}

/// Write one segment (parquet + idx) under `root` and return its manifest entry.
fn write_segment(
    root: &Path,
    seg: SegmentId,
    records: &[LogRecord],
    schema: &LogSchema,
) -> FileEntry {
    let batch = build_batch(records, schema);
    write_parquet(&root.join(Storage::parquet_path(seg)), &batch);
    write_idx(&root.join(Storage::index_path(seg)), &batch, schema);
    entry_from(records, seg)
}

fn write_manifest(root: &Path, entries: Vec<FileEntry>) {
    let mut m = Manifest::new();
    for e in entries {
        m.add(e);
    }
    let path = root.join(MANIFEST_OBJECT_PATH);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, m.to_json().unwrap()).unwrap();
}

/// Flatten result batches into `(timestamp, service, body)` rows, in batch order.
fn rows(batches: &[RecordBatch]) -> Vec<(i64, String, String)> {
    let mut out = Vec::new();
    for b in batches {
        let ts = b
            .column_by_name("timestamp")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        let svc = b
            .column_by_name("service.name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let body = b
            .column_by_name("body")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        for i in 0..b.num_rows() {
            out.push((
                ts.value(i),
                svc.value(i).to_string(),
                body.value(i).to_string(),
            ));
        }
    }
    out
}

fn engine(dir: &TempDir) -> QueryEngine {
    QueryEngine::new(dir.path().to_path_buf(), schema()).unwrap()
}

// ---- tests -----------------------------------------------------------------

/// `/api/services` delegates to this: distinct `service.name`, sorted ascending, deduped
/// across all segments (out-of-order + duplicate services collapse to one sorted set).
#[tokio::test]
async fn distinct_services_returns_sorted_distinct() {
    let dir = TempDir::new().unwrap();
    let s = schema();
    let e0 = write_segment(
        dir.path(),
        SegmentId(0),
        &[record(100, "web", "a"), record(200, "api", "b")],
        &s,
    );
    let e1 = write_segment(
        dir.path(),
        SegmentId(1),
        &[record(300, "api", "c"), record(400, "db", "d")],
        &s,
    );
    write_manifest(dir.path(), vec![e0, e1]);

    let got = engine(&dir).distinct_services().await.unwrap();
    assert_eq!(
        got.as_ref(),
        &vec!["api".to_string(), "db".to_string(), "web".to_string()]
    );
}

/// Regression (empty store): a fresh system, or one where retention/manual delete purged
/// every file, must yield an empty list with NO query error. This is the path `/api/services`
/// relies on to avoid the `No field named "service.name"` schema error when there is no
/// `logs` Parquet to plan against.
#[tokio::test]
async fn distinct_services_empty_store_returns_empty() {
    let dir = TempDir::new().unwrap();
    // No manifest at all (brand-new hot dir).
    assert!(engine(&dir)
        .distinct_services()
        .await
        .unwrap()
        .as_ref()
        .is_empty());
    // Explicit empty manifest — the exact post-purge state (`{"entries":[]}`).
    write_manifest(dir.path(), vec![]);
    assert!(engine(&dir)
        .distinct_services()
        .await
        .unwrap()
        .as_ref()
        .is_empty());
}

/// 1. A time-range search returns only the rows inside the window, newest first.
#[tokio::test]
async fn search_time_range_returns_in_range_rows_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();
    let recs = vec![
        record(100, "api", "one"),
        record(200, "api", "two"),
        record(300, "api", "three"),
        record(400, "api", "four"),
    ];
    let e = write_segment(dir.path(), SegmentId(1), &recs, &s);
    write_manifest(dir.path(), vec![e]);

    let out = engine(&dir)
        .search(QueryRequest {
            start_ts_nanos: 150,
            end_ts_nanos: 350,
            services: vec![],
            severities: vec![],
            text: None,
            query: None,
            limit: 100,
        })
        .await
        .unwrap();

    let got = rows(&out);
    let ts: Vec<i64> = got.iter().map(|(t, _, _)| *t).collect();
    assert_eq!(ts, vec![300, 200], "only in-range rows, newest first");
}

/// 2. A `service` filter prunes files whose service range excludes it, and filters rows.
#[tokio::test]
async fn search_service_prunes_files_and_filters_rows() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();

    let api = vec![record(100, "api", "a1"), record(200, "api", "a2")];
    // A file entirely outside the requested service's range ("web" > "api"): must be pruned
    // by its skip-index service range, not merely row-filtered.
    let web = vec![record(150, "web", "w1"), record(250, "web", "w2")];

    let ea = write_segment(dir.path(), SegmentId(1), &api, &s);
    let ew = write_segment(dir.path(), SegmentId(2), &web, &s);
    write_manifest(dir.path(), vec![ea, ew]);

    let out = engine(&dir)
        .search(QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 1_000,
            services: vec!["api".to_string()],
            severities: vec![],
            text: None,
            query: None,
            limit: 100,
        })
        .await
        .unwrap();

    let got = rows(&out);
    assert!(
        got.iter().all(|(_, svc, _)| svc == "api"),
        "only api rows survive, got {got:?}"
    );
    let ts: Vec<i64> = got.iter().map(|(t, _, _)| *t).collect();
    assert_eq!(ts, vec![200, 100]);
}

/// 3. A `text` search returns only matching rows; a file whose bloom lacks a *bloom-safe*
///    (interior, both-sides-delimited) token of the search string is skipped entirely. The search
///    text `"an alpha login"` has one interior token — `alpha` — so bloom pruning is exercised
///    (a single word like `"alpha"` alone would be an edge token: substring semantics forbid
///    bloom-pruning on it, since `alpha` could be a fragment of `alphabet`). The skipped file's
///    Parquet is deliberately corrupt: if the bloom prune failed and the engine tried to read it,
///    the query would error.
#[tokio::test]
async fn search_text_prunes_via_bloom_and_confirms_rows() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();

    // File A: has a matching "an alpha login" row and a non-matching "delta" row.
    let a = vec![
        record(1000, "svc", "an alpha login ok"),
        record(1100, "svc", "delta logout"),
    ];
    let ea = write_segment(dir.path(), SegmentId(1), &a, &s);

    // File B: bloom over {beta, gamma, delta} — never "alpha". Valid idx + entry (so only the
    // bloom can prune it), but a CORRUPT parquet file that would fail if ever opened.
    let b = vec![
        record(1050, "svc", "beta gamma"),
        record(1150, "svc", "gamma delta"),
    ];
    let bb = build_batch(&b, &s);
    write_idx(&dir.path().join(Storage::index_path(SegmentId(2))), &bb, &s);
    let corrupt = dir.path().join(Storage::parquet_path(SegmentId(2)));
    fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
    fs::write(&corrupt, b"NOT A PARQUET FILE").unwrap();
    let eb = entry_from(&b, SegmentId(2));

    write_manifest(dir.path(), vec![ea, eb]);

    let out = engine(&dir)
        .search(QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 100_000,
            services: vec![],
            severities: vec![],
            // Interior token `alpha` drives the bloom prune; the whole string is the row predicate.
            text: Some("an alpha login".to_string()),
            query: None,
            limit: 100,
        })
        .await
        .unwrap();

    let got = rows(&out);
    assert_eq!(
        got,
        vec![(1000, "svc".to_string(), "an alpha login ok".to_string())],
        "only the matching row, corrupt bloom-pruned file never read"
    );
}

/// A well-framed but corrupt `.idx`: valid magic and version, but `num_bits = 0`. Before the
/// decode and keep-on-error hardening this both aborted the search and panicked (divide-by-zero)
/// inside the bloom when the query engine tried to bloom-test it. Written next to a VALID parquet
/// so the file must be kept and its matching rows must come through.
fn write_corrupt_zero_bits_idx(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut b = Vec::new();
    b.extend_from_slice(b"PXSK");
    b.push(2); // version
    b.extend_from_slice(&0u64.to_le_bytes()); // num_bits = 0 (poison)
    b.extend_from_slice(&1u32.to_le_bytes()); // num_hashes
    b.extend_from_slice(&0u64.to_le_bytes()); // bits_len = 0
    b.push(0); // has_timestamp = 0
    b.push(0); // has_service = 0
    b.push(0); // has_host = 0
    fs::write(path, b).unwrap();
}

/// 3a. A corrupt/undecodable `.idx` must KEEP its file (conservative pruning), so a search over a
///     window that includes one torn sidecar still returns the good files' rows AND the corrupt-
///     sidecar file's matching rows — never a query error and never a panic. The bad sidecar here
///     is well-framed but claims `num_bits = 0`; before the fix this decoded to a poisoned bloom
///     that divided by zero on the first membership probe, panicking the whole search.
#[tokio::test]
async fn search_keeps_file_with_corrupt_idx_and_still_returns_rows() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();

    // File A: good idx + parquet, with a row matching "an alpha login".
    let a = vec![
        record(1000, "svc", "an alpha login ok"),
        record(1100, "svc", "delta logout"),
    ];
    let ea = write_segment(dir.path(), SegmentId(1), &a, &s);

    // File B: VALID parquet with a matching row, but a CORRUPT (num_bits = 0) idx. It cannot be
    // bloom-pruned; conservative pruning must keep it and its rows must be returned.
    let b = vec![
        record(1050, "svc", "please an alpha login now"),
        record(1150, "svc", "beta only"),
    ];
    let bb = build_batch(&b, &s);
    write_parquet(&dir.path().join(Storage::parquet_path(SegmentId(2))), &bb);
    write_corrupt_zero_bits_idx(&dir.path().join(Storage::index_path(SegmentId(2))));
    let eb = entry_from(&b, SegmentId(2));

    write_manifest(dir.path(), vec![ea, eb]);

    let out = engine(&dir)
        .search(QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 10_000,
            services: vec![],
            severities: vec![],
            // Interior token `alpha` drives the bloom step; the whole string is the row predicate.
            text: Some("an alpha login".to_string()),
            query: None,
            limit: 100,
        })
        .await
        .unwrap();

    // Both matching rows returned, newest first: the good file's AND the corrupt-sidecar file's.
    let ts: Vec<i64> = rows(&out).iter().map(|(t, _, _)| *t).collect();
    assert_eq!(
        ts,
        vec![1050, 1000],
        "a corrupt .idx keeps its file; both matching rows come through"
    );
}

/// 3b. A multi-service filter keeps rows for any of the requested services and prunes files
///     whose service range excludes all of them. The excluded file ("db") has a CORRUPT
///     parquet, so a pruning miss would surface as a read error.
#[tokio::test]
async fn search_multi_service_keeps_any_and_prunes_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();

    let api = vec![record(100, "api", "a1")];
    let web = vec![record(200, "web", "w1")];
    let ea = write_segment(dir.path(), SegmentId(1), &api, &s);
    let ew = write_segment(dir.path(), SegmentId(2), &web, &s);

    // "db" is outside the requested {api, web} range → must be pruned by its skip-index
    // service range. Valid idx + manifest entry, but a corrupt parquet that errors if read.
    let db = vec![record(300, "db", "d1")];
    let db_batch = build_batch(&db, &s);
    write_idx(
        &dir.path().join(Storage::index_path(SegmentId(3))),
        &db_batch,
        &s,
    );
    let corrupt = dir.path().join(Storage::parquet_path(SegmentId(3)));
    fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
    fs::write(&corrupt, b"NOT A PARQUET FILE").unwrap();
    let edb = entry_from(&db, SegmentId(3));

    write_manifest(dir.path(), vec![ea, ew, edb]);

    let out = engine(&dir)
        .search(QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 1_000,
            services: vec!["api".to_string(), "web".to_string()],
            severities: vec![],
            text: None,
            query: None,
            limit: 100,
        })
        .await
        .unwrap();

    let got = rows(&out);
    let services: std::collections::BTreeSet<&str> =
        got.iter().map(|(_, svc, _)| svc.as_str()).collect();
    assert_eq!(
        services,
        ["api", "web"].into_iter().collect(),
        "both requested services returned, corrupt 'db' file never read, got {got:?}"
    );
}

/// 3c. A severity filter returns only rows whose `severity_number` falls in a requested range.
#[tokio::test]
async fn search_severity_filters_rows() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();

    let recs = vec![
        record_sev(100, "api", "debugmsg", 5),  // debug
        record_sev(200, "api", "infomsg", 9),   // info
        record_sev(300, "api", "errormsg", 18), // error
        record_sev(400, "api", "fatalmsg", 22), // fatal
    ];
    let e = write_segment(dir.path(), SegmentId(1), &recs, &s);
    write_manifest(dir.path(), vec![e]);

    // Ask for error (17..=20) and fatal (21..=24) only.
    let out = engine(&dir)
        .search(QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 1_000,
            services: vec![],
            severities: vec![(17, 20), (21, 24)],
            text: None,
            query: None,
            limit: 100,
        })
        .await
        .unwrap();

    let bodies: Vec<String> = rows(&out).into_iter().map(|(_, _, b)| b).collect();
    assert_eq!(
        bodies,
        vec!["fatalmsg".to_string(), "errormsg".to_string()],
        "only error+fatal rows, newest first"
    );
}

/// 3d. When more rows match than `limit`, only the newest `limit` come back — even across a
///     tie on the boundary timestamp. Guards the newest-N selection (and the two-pass
///     late-materialization path must not over- or under-return at the cutoff).
#[tokio::test]
async fn search_limit_returns_newest_n_across_ties() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();
    // Six matching rows with a tie at 300; the newest 3 timestamps are [500, 400, 300].
    let recs = vec![
        record(100, "api", "a"),
        record(200, "api", "b"),
        record(300, "api", "c1"),
        record(300, "api", "c2"),
        record(400, "api", "d"),
        record(500, "api", "e"),
    ];
    let e = write_segment(dir.path(), SegmentId(1), &recs, &s);
    write_manifest(dir.path(), vec![e]);

    let out = engine(&dir)
        .search(QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 1_000,
            services: vec![],
            severities: vec![],
            text: None,
            query: None,
            limit: 3,
        })
        .await
        .unwrap();

    let ts: Vec<i64> = rows(&out).into_iter().map(|(t, _, _)| t).collect();
    assert_eq!(
        ts,
        vec![500, 400, 300],
        "newest 3 timestamps; the tie at 300 is trimmed by the limit"
    );
}

/// 3e. A filter must constrain the newest-N *selection*, not just post-filter it. "web" holds
///     the globally-newest rows and "api" the older ones; asking for api's newest 2 must return
///     api's rows, never an empty set. This is the load-bearing guard for the two-pass path:
///     the cutoff timestamp has to be computed from the *filtered* stream, not the raw newest.
#[tokio::test]
async fn search_filter_bounds_top_n_not_just_post_filters() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();
    let recs = vec![
        record(100, "api", "a1"),
        record(200, "api", "a2"),
        record(300, "api", "a3"),
        record(800, "web", "w1"),
        record(900, "web", "w2"),
        record(1000, "web", "w3"),
    ];
    let e = write_segment(dir.path(), SegmentId(1), &recs, &s);
    write_manifest(dir.path(), vec![e]);

    let out = engine(&dir)
        .search(QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 2_000,
            services: vec!["api".to_string()],
            severities: vec![],
            text: None,
            query: None,
            limit: 2,
        })
        .await
        .unwrap();

    let got = rows(&out);
    assert!(
        got.iter().all(|(_, svc, _)| svc == "api"),
        "only api rows, got {got:?}"
    );
    let ts: Vec<i64> = got.iter().map(|(t, _, _)| *t).collect();
    assert_eq!(
        ts,
        vec![300, 200],
        "api's newest 2 — the filter must bound the top-N, not web's later timestamps"
    );
}

/// 4. Raw SQL over the registered `logs` table returns the expected total count.
#[tokio::test]
async fn sql_count_returns_total_rows() {
    let dir = tempfile::tempdir().unwrap();
    let s = schema();

    let seg1 = vec![record(100, "api", "x"), record(200, "api", "y")];
    let seg2 = vec![
        record(300, "web", "z"),
        record(400, "web", "w"),
        record(500, "web", "v"),
    ];
    let e1 = write_segment(dir.path(), SegmentId(1), &seg1, &s);
    let e2 = write_segment(dir.path(), SegmentId(2), &seg2, &s);
    write_manifest(dir.path(), vec![e1, e2]);

    let out = engine(&dir)
        .sql("SELECT count(*) AS n FROM logs")
        .await
        .unwrap();

    assert_eq!(out.len(), 1);
    let n = out[0]
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .value(0);
    assert_eq!(n, 5, "total rows across both segments");
}

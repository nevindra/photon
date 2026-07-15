//! Integration tests for `SpanQueryEngine::search_traces`: build a known spans corpus, write it
//! to a temp hot dir as real `data-spans/*.parquet` + `.idx` skip-index sidecars + a spans
//! manifest (same on-disk artifacts the `SpanCompactor` produces — see the note at the bottom of
//! this file), construct an engine over it, and assert the per-trace rollups, sort orders, the
//! partial-trace (no-root) fallback, paging, grammar filtering, and `matched_count`.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use tempfile::TempDir;

use photon_core::manifest::{FileEntry, Manifest, SPANS_MANIFEST_OBJECT_PATH};
use photon_core::query::{parse, SpanFieldResolver};
use photon_core::segment::SegmentId;
use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
use photon_core::span_schema::SpanSchema;
use photon_index::SkipIndex;
use photon_query::{SpanQueryEngine, SpanQueryRequest, SpanSort, TraceSummary};
use photon_storage::Storage;

fn schema() -> SpanSchema {
    SpanSchema::new(&["service.name".to_string()])
}

/// Like [`schema`] but also promotes `host.name` — matching the default config
/// (`promoted_attributes = ["service.name", "host.name"]`). `host.name` therefore becomes its own
/// top-level Utf8 column and is excluded from the long-tail `attributes` Map.
fn schema_with_host() -> SpanSchema {
    SpanSchema::new(&["service.name".to_string(), "host.name".to_string()])
}

#[allow(clippy::too_many_arguments)]
fn span(
    trace: &str,
    span_id: &str,
    parent: Option<&str>,
    service: &str,
    name: &str,
    start: i64,
    end: Option<i64>,
    duration: Option<i64>,
    status: Option<i32>,
) -> SpanRecord {
    let mut attributes = BTreeMap::new();
    attributes.insert("service.name".to_string(), service.to_string());
    SpanRecord {
        trace_id: trace.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent.map(|s| s.to_string()),
        name: Some(name.to_string()),
        start_time_nanos: start,
        end_time_nanos: end,
        duration_nanos: duration,
        status_code: status,
        attributes,
        ..Default::default()
    }
}

/// Like [`span`], but carries extra long-tail attributes (beyond `service.name`) so the
/// projected-`root_attributes` path has something to decode. End/duration/status are fixed —
/// the projection tests don't assert on them.
fn span_with_attrs(
    trace: &str,
    span_id: &str,
    parent: Option<&str>,
    service: &str,
    name: &str,
    start: i64,
    extra: &[(&str, &str)],
) -> SpanRecord {
    let mut attributes = BTreeMap::new();
    attributes.insert("service.name".to_string(), service.to_string());
    for (k, v) in extra {
        attributes.insert(k.to_string(), v.to_string());
    }
    SpanRecord {
        trace_id: trace.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent.map(|s| s.to_string()),
        name: Some(name.to_string()),
        start_time_nanos: start,
        end_time_nanos: Some(start + 100),
        duration_nanos: Some(100),
        status_code: Some(1),
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
    write_segment_with(root, seg, records, &schema())
}

/// Like [`write_segment`] but with an explicit schema (so a caller can promote extra attributes,
/// e.g. `host.name`, into their own top-level column instead of the long-tail Map).
fn write_segment_with(
    root: &Path,
    seg: SegmentId,
    records: &[SpanRecord],
    schema: &SpanSchema,
) -> FileEntry {
    let batch = build_batch(records, schema);
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

/// A 5-trace corpus across two segments:
/// - t1: root(api "GET /") + child(db, error)               → 2 spans, 1 err, {api, db}, dur 500
/// - t2: root(api "POST /checkout", dur 5000) + child(cache) → 2 spans, 0 err, {api, cache}
/// - t3: root(web "GET /home", dur 50)                       → 1 span,  0 err, {web}
/// - t4: NO root — two parented spans (worker); earliest has no duration (fallback)
/// - t5: root(api "batch", error) + child(api, error)        → 2 spans, 2 err, {api}
fn build_corpus(root: &Path) {
    let seg1 = write_segment(
        root,
        SegmentId(1),
        &[
            span(
                "t1",
                "t1a",
                None,
                "api",
                "GET /",
                1000,
                Some(1500),
                Some(500),
                Some(1),
            ),
            span(
                "t1",
                "t1b",
                Some("t1a"),
                "db",
                "SELECT",
                1100,
                Some(1300),
                Some(200),
                Some(2),
            ),
            span(
                "t2",
                "t2a",
                None,
                "api",
                "POST /checkout",
                2000,
                Some(7000),
                Some(5000),
                Some(1),
            ),
            span(
                "t2",
                "t2b",
                Some("t2a"),
                "cache",
                "GET key",
                2100,
                Some(2200),
                Some(100),
                Some(1),
            ),
            span(
                "t5",
                "t5a",
                None,
                "api",
                "batch",
                500,
                Some(600),
                Some(100),
                Some(2),
            ),
            span(
                "t5",
                "t5b",
                Some("t5a"),
                "api",
                "batch.item",
                550,
                Some(600),
                Some(50),
                Some(2),
            ),
        ],
    );
    let seg2 = write_segment(
        root,
        SegmentId(2),
        &[
            span(
                "t3",
                "t3a",
                None,
                "web",
                "GET /home",
                3000,
                Some(3050),
                Some(50),
                Some(1),
            ),
            // t4 has no parent-less span; earliest (start 3900) has no duration → fallback used.
            span(
                "t4",
                "t4a",
                Some("p1"),
                "worker",
                "job.b",
                3900,
                Some(4200),
                None,
                Some(1),
            ),
            span(
                "t4",
                "t4b",
                Some("p2"),
                "worker",
                "job.a",
                4000,
                Some(4050),
                Some(50),
                Some(1),
            ),
        ],
    );
    write_manifest(root, vec![seg1, seg2]);
}

/// A one-trace corpus whose ROOT span carries extra long-tail attributes. Trace "T":
/// root(web "checkout", attrs {http.route:/checkout, http.method:POST}) + child(db, attrs {db.system:pg}).
fn write_corpus_with_root_attrs(root: &Path) {
    let seg = write_segment(
        root,
        SegmentId(1),
        &[
            span_with_attrs(
                "T",
                "sA",
                None,
                "web",
                "checkout",
                1000,
                &[("http.route", "/checkout"), ("http.method", "POST")],
            ),
            span_with_attrs(
                "T",
                "sB",
                Some("sA"),
                "db",
                "SELECT",
                1050,
                &[("db.system", "postgres")],
            ),
        ],
    );
    write_manifest(root, vec![seg]);
}

fn engine(root: &Path) -> SpanQueryEngine {
    SpanQueryEngine::new(root.to_path_buf(), schema()).unwrap()
}

fn req(sort: SpanSort, limit: usize, offset: usize, query: Option<&str>) -> SpanQueryRequest {
    let query = query.map(|q| {
        SpanFieldResolver::new(&["service.name".to_string()])
            .resolve(&parse(q).unwrap())
            .unwrap()
    });
    SpanQueryRequest {
        start_ts_nanos: 0,
        end_ts_nanos: i64::MAX,
        query,
        sort,
        limit,
        offset,
        projected_attributes: Vec::new(),
    }
}

fn ids(traces: &[TraceSummary]) -> Vec<String> {
    traces.iter().map(|t| t.trace_id.clone()).collect()
}

fn find<'a>(traces: &'a [TraceSummary], id: &str) -> &'a TraceSummary {
    traces
        .iter()
        .find(|t| t.trace_id == id)
        .unwrap_or_else(|| panic!("trace {id} not found in {:?}", ids(traces)))
}

#[tokio::test]
async fn rolls_up_span_and_error_counts_and_distinct_services() {
    let tmp = TempDir::new().unwrap();
    build_corpus(tmp.path());
    let res = engine(tmp.path())
        .search_traces(req(SpanSort::Recent, 100, 0, None))
        .await
        .unwrap();

    let t1 = find(&res.traces, "t1");
    assert_eq!(t1.span_count, 2);
    assert_eq!(t1.error_count, 1);
    assert_eq!(t1.services, vec!["api".to_string(), "db".to_string()]);
    assert_eq!(t1.root_service.as_deref(), Some("api"));
    assert_eq!(t1.root_name.as_deref(), Some("GET /"));
    assert_eq!(t1.start_ts_nanos, 1000);
    assert_eq!(t1.duration_nanos, Some(500));
}

#[tokio::test]
async fn sort_slowest_orders_by_duration_desc_nulls_last() {
    let tmp = TempDir::new().unwrap();
    build_corpus(tmp.path());
    let res = engine(tmp.path())
        .search_traces(req(SpanSort::Slowest, 100, 0, None))
        .await
        .unwrap();
    // durations: t2=5000, t1=500, t4=300(fallback), t5=100, t3=50.
    assert_eq!(ids(&res.traces), vec!["t2", "t1", "t4", "t5", "t3"]);
}

#[tokio::test]
async fn sort_errors_puts_error_traces_first() {
    let tmp = TempDir::new().unwrap();
    build_corpus(tmp.path());
    let res = engine(tmp.path())
        .search_traces(req(SpanSort::Errors, 100, 0, None))
        .await
        .unwrap();
    // error_count: t5=2, t1=1, then 0s by start desc: t4(3900), t3(3000), t2(2000).
    assert_eq!(ids(&res.traces), vec!["t5", "t1", "t4", "t3", "t2"]);
}

#[tokio::test]
async fn partial_trace_without_root_uses_earliest_span_and_duration_fallback() {
    let tmp = TempDir::new().unwrap();
    build_corpus(tmp.path());
    let res = engine(tmp.path())
        .search_traces(req(SpanSort::Recent, 100, 0, None))
        .await
        .unwrap();
    let t4 = find(&res.traces, "t4");
    // No parent-less span → representative is the earliest (start 3900), whose duration is None,
    // so duration falls back to max(end)=4200 - min(start)=3900 = 300.
    assert_eq!(t4.start_ts_nanos, 3900);
    assert_eq!(t4.root_name.as_deref(), Some("job.b"));
    assert_eq!(t4.duration_nanos, Some(300));
    assert_eq!(t4.span_count, 2);
    assert_eq!(t4.services, vec!["worker".to_string()]);
}

#[tokio::test]
async fn limit_and_offset_page_the_results() {
    let tmp = TempDir::new().unwrap();
    build_corpus(tmp.path());
    let eng = engine(tmp.path());
    // Recent order by start desc: t4(3900), t3(3000), t2(2000), t1(1000), t5(500).
    let page1 = eng
        .search_traces(req(SpanSort::Recent, 2, 0, None))
        .await
        .unwrap();
    assert_eq!(ids(&page1.traces), vec!["t4", "t3"]);
    assert_eq!(page1.matched_count, 5);

    let page2 = eng
        .search_traces(req(SpanSort::Recent, 2, 2, None))
        .await
        .unwrap();
    assert_eq!(ids(&page2.traces), vec!["t2", "t1"]);
    assert_eq!(page2.matched_count, 5);
}

#[tokio::test]
async fn grammar_filter_selects_matching_traces_but_rolls_up_all_their_spans() {
    let tmp = TempDir::new().unwrap();
    build_corpus(tmp.path());
    // Only t1 has a span whose service is "db"; the rollup must still include t1's api span.
    let res = engine(tmp.path())
        .search_traces(req(SpanSort::Recent, 100, 0, Some("service:db")))
        .await
        .unwrap();
    assert_eq!(ids(&res.traces), vec!["t1"]);
    assert_eq!(res.matched_count, 1);
    let t1 = find(&res.traces, "t1");
    assert_eq!(t1.span_count, 2); // whole trace, not just the matching "db" span
    assert_eq!(t1.services, vec!["api".to_string(), "db".to_string()]);
    assert_eq!(t1.error_count, 1);
}

#[tokio::test]
async fn matched_count_is_full_distinct_trace_count() {
    let tmp = TempDir::new().unwrap();
    build_corpus(tmp.path());
    let res = engine(tmp.path())
        .search_traces(req(SpanSort::Recent, 100, 0, None))
        .await
        .unwrap();
    assert_eq!(res.matched_count, 5);
    assert_eq!(res.traces.len(), 5);
}

#[tokio::test]
async fn empty_manifest_yields_empty_result() {
    let tmp = TempDir::new().unwrap();
    let res = engine(tmp.path())
        .search_traces(req(SpanSort::Recent, 100, 0, None))
        .await
        .unwrap();
    assert!(res.traces.is_empty());
    assert_eq!(res.matched_count, 0);
}

#[tokio::test]
async fn free_text_query_rolls_up_spans_from_name_bloom_pruned_files() {
    let tmp = TempDir::new().unwrap();
    // Segment 1: trace "T"'s ROOT span. name "checkout handler" tokenizes to include "checkout",
    // so this file's name-bloom contains the token and it survives free-text pruning.
    let seg1 = write_segment(
        tmp.path(),
        SegmentId(1),
        &[span(
            "T",
            "sA",
            None,
            "web",
            "checkout handler",
            1000,
            Some(1200),
            Some(200),
            None, // UNSET
        )],
    );
    // Segment 2: trace "T"'s CHILD (error) span. name "db query" does NOT tokenize to "checkout",
    // so this file's name-bloom lacks the token (confirmed by
    // `temp_bloom_sanity_check_checkout_token_not_in_segment2` during development) — a free-text
    // search for "checkout" prunes this file out of the pruned survivor set `df`. On the buggy
    // code step 3 rolled up from that same pruned `df`, so this error span silently vanished from
    // the rollup (span_count 1, error_count 0) even though it belongs to a matched trace.
    let seg2 = write_segment(
        tmp.path(),
        SegmentId(2),
        &[span(
            "T",
            "sB",
            Some("sA"),
            "db",
            "db query",
            1050,
            Some(1100),
            Some(50),
            Some(2), // ERROR
        )],
    );
    write_manifest(tmp.path(), vec![seg1, seg2]);

    let res = engine(tmp.path())
        .search_traces(req(SpanSort::Recent, 100, 0, Some("checkout")))
        .await
        .unwrap();

    assert_eq!(res.matched_count, 1);
    let t = find(&res.traces, "T");
    assert_eq!(t.span_count, 2);
    assert_eq!(t.error_count, 1);
    assert_eq!(t.services, vec!["db".to_string(), "web".to_string()]);
}

#[tokio::test]
async fn root_attributes_projects_only_requested_keys() {
    let tmp = TempDir::new().unwrap();
    // Root span of trace "T" has attributes { "http.route": "/checkout", "http.method": "POST" }.
    write_corpus_with_root_attrs(tmp.path());
    let eng = engine(tmp.path());

    // Request only http.route.
    let mut r = req(SpanSort::Recent, 10, 0, None);
    r.projected_attributes = vec!["http.route".to_string()];
    let out = eng.search_traces(r).await.unwrap();

    let t = find(&out.traces, "T");
    assert_eq!(
        t.root_attributes.get("http.route").map(String::as_str),
        Some("/checkout")
    );
    assert!(
        !t.root_attributes.contains_key("http.method"),
        "unrequested key must not be projected"
    );
}

#[tokio::test]
async fn no_columns_means_empty_root_attributes() {
    let tmp = TempDir::new().unwrap();
    write_corpus_with_root_attrs(tmp.path());
    let eng = engine(tmp.path());
    let out = eng
        .search_traces(req(SpanSort::Recent, 10, 0, None))
        .await
        .unwrap();
    assert!(out.traces.iter().all(|t| t.root_attributes.is_empty()));
}

#[tokio::test]
async fn root_attributes_surfaces_promoted_columns() {
    // With the default config `host.name` is a *promoted* attribute — stored in its own top-level
    // column and excluded from the `attributes` Map by `SpanBatchBuilder`. Decoding only the Map
    // would silently miss it, so a `columns: ["host.name"]` request must still surface it (and
    // merge cleanly with a long-tail key like `http.route` read from the Map).
    let tmp = TempDir::new().unwrap();
    let schema = schema_with_host();
    let records = [
        span_with_attrs(
            "T",
            "sA",
            None,
            "web",
            "checkout",
            1000,
            &[("host.name", "node-7"), ("http.route", "/checkout")],
        ),
        span_with_attrs(
            "T",
            "sB",
            Some("sA"),
            "db",
            "SELECT",
            1050,
            &[("host.name", "node-9"), ("db.system", "postgres")],
        ),
    ];
    let entry = write_segment_with(tmp.path(), SegmentId(1), &records, &schema);
    write_manifest(tmp.path(), vec![entry]);

    let eng = SpanQueryEngine::new(tmp.path().to_path_buf(), schema).unwrap();
    let mut r = req(SpanSort::Recent, 10, 0, None);
    r.projected_attributes = vec!["host.name".to_string(), "http.route".to_string()];
    let out = eng.search_traces(r).await.unwrap();

    let t = find(&out.traces, "T");
    // Promoted column (root span "sA") — the false negative this fix addresses.
    assert_eq!(
        t.root_attributes.get("host.name").map(String::as_str),
        Some("node-7"),
        "a requested promoted attribute must surface in root_attributes"
    );
    // Long-tail Map key, requested — the two sources merge.
    assert_eq!(
        t.root_attributes.get("http.route").map(String::as_str),
        Some("/checkout")
    );
    // The child span's promoted value must not leak into the representative's attributes.
    assert_ne!(
        t.root_attributes.get("host.name").map(String::as_str),
        Some("node-9")
    );
    // Present-but-unrequested keys are still excluded.
    assert!(!t.root_attributes.contains_key("db.system"));
}

#[tokio::test]
async fn straddling_trace_spans_before_the_window_are_rolled_up_fully() {
    // F10 regression: a trace whose *matching* span is inside the request window, but one of whose
    // spans lives in a segment ENTIRELY before the window. The old step 3 re-scanned the original
    // `[start, end]` window and pruned that segment (min/max miss), undercounting the trace
    // (span_count 1, error_count 0). The ±1h-padded, narrowed step-3 window now admits the segment,
    // so the whole trace is rolled up. This dataset makes the fix observable: on the OLD path this
    // test would see span_count == 1.
    let tmp = TempDir::new().unwrap();
    // seg1: T's root span at start 1500 — inside the query window [1000, 2000].
    let seg1 = write_segment(
        tmp.path(),
        SegmentId(1),
        &[span(
            "T",
            "sA",
            None,
            "api",
            "GET /",
            1500,
            Some(1600),
            Some(100),
            Some(1),
        )],
    );
    // seg2: T's child (error) span at start 500 — a segment whose [min_ts, max_ts] = [500, 500] is
    // entirely before the window, so the old whole-window step 3 pruned it (the undercount).
    let seg2 = write_segment(
        tmp.path(),
        SegmentId(2),
        &[span(
            "T",
            "sB",
            Some("sA"),
            "db",
            "SELECT",
            500,
            Some(600),
            Some(100),
            Some(2), // ERROR
        )],
    );
    write_manifest(tmp.path(), vec![seg1, seg2]);

    let r = SpanQueryRequest {
        start_ts_nanos: 1000,
        end_ts_nanos: 2000,
        query: None,
        sort: SpanSort::Recent,
        limit: 100,
        offset: 0,
        projected_attributes: Vec::new(),
    };
    let res = engine(tmp.path()).search_traces(r).await.unwrap();

    assert_eq!(res.matched_count, 1);
    let t = find(&res.traces, "T");
    // The straddling child span (in the pre-window segment) is now included.
    assert_eq!(
        t.span_count, 2,
        "the straddling child span must be rolled up"
    );
    assert_eq!(
        t.error_count, 1,
        "the straddling error span must be counted"
    );
    assert_eq!(t.services, vec!["api".to_string(), "db".to_string()]);
    // The representative root is unchanged (the in-window parent-less span).
    assert_eq!(t.root_name.as_deref(), Some("GET /"));
    assert_eq!(t.start_ts_nanos, 1500);
}

#[tokio::test]
async fn ranking_caps_rollup_at_max_candidate_traces_while_matched_count_is_full() {
    // Bounded-memory boundary: with 2001 distinct traces (> the 2000 cap), `matched_count` still
    // reports the full distinct total, but only the newest 2000 are ranked/rolled up (DataFusion
    // `LIMIT` — never an unbounded Rust Vec). `limit` is set above the cap so paging can't be the
    // limiter; the oldest trace (smallest start) is the one dropped by the cap.
    let tmp = TempDir::new().unwrap();
    let count = MAX_TRACES_OVER_CAP;
    let mut records = Vec::with_capacity(count);
    for i in 0..count as i64 {
        records.push(span(
            &format!("t{i:05}"),
            &format!("s{i}"),
            None,
            "api",
            "op",
            1000 + i, // distinct, increasing starts → deterministic newest-2000
            Some(1000 + i + 10),
            Some(10),
            Some(1),
        ));
    }
    let seg = write_segment(tmp.path(), SegmentId(1), &records);
    write_manifest(tmp.path(), vec![seg]);

    let r = SpanQueryRequest {
        start_ts_nanos: 0,
        end_ts_nanos: i64::MAX,
        query: None,
        sort: SpanSort::Recent,
        limit: 5000, // above the cap, so the 2000 ceiling comes from the ranking, not paging
        offset: 0,
        projected_attributes: Vec::new(),
    };
    let res = engine(tmp.path()).search_traces(r).await.unwrap();

    assert_eq!(
        res.matched_count, count as u64,
        "matched_count reports the full distinct total (COUNT(DISTINCT trace_id))"
    );
    assert_eq!(
        res.traces.len(),
        2000,
        "only the newest MAX_CANDIDATE_TRACES (2000) are ranked/rolled up"
    );
    // The oldest trace (start 1000, id t00000) is the one the cap drops.
    assert!(
        res.traces.iter().all(|t| t.trace_id != "t00000"),
        "the oldest trace must be excluded by the newest-first cap"
    );
}

/// One past the `MAX_CANDIDATE_TRACES` (2000) cap — a local mirror so this test needs no access to
/// the crate-private constant.
const MAX_TRACES_OVER_CAP: usize = 2001;

// NOTE: These fixtures write the same on-disk artifacts (`data-spans/seg-*.parquet` + `.idx`
// spans skip index + `manifest/spans-manifest.json`) that `photon_compact::SpanCompactor::run_once`
// produces — via the exact `write_segment`/`write_parquet`/`write_idx`/`write_manifest` pattern
// already used by `tests/trace.rs`. Driving the real compactor here would require adding
// `photon-compact` + `photon-wal` (and a `Wal` fake) as `photon-query` dev-dependencies, which is
// out of scope for this task; the artifacts, and therefore the query path under test, are identical.

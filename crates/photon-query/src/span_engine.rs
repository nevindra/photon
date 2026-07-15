//! Spans query engine: SQL over `data-spans/` plus `get_trace` (prune candidate files by the
//! `trace_id` bloom, then read the surviving spans Parquet). Later phases add trace/span
//! search, RED, and the service map.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::{lit, lit_timestamp_nano, Expr, ParquetReadOptions};
use tokio::task::spawn_blocking;

use photon_core::manifest::{FileEntry, Manifest, SPANS_MANIFEST_OBJECT_PATH};
use photon_core::query::SpanResolvedQuery;
use photon_core::span_schema::{self, SpanSchema};
use photon_core::PhotonError;
use photon_index::{interior_tokens, SkipIndex};
use photon_storage::Storage;

use crate::{cached_manifest, col_ref, session, ManifestCache};

/// ±window applied to a `get_trace` time hint when selecting manifest candidates. A trace's
/// spans cluster within seconds of the originating row, so one hour is comfortably conservative:
/// it prunes files without risking a real span. A trace longer than this — or a hint far from the
/// spans — is a documented v1 edge; the `trace_id` bloom is the correctness pruner for the
/// in-window files, and a hint-less call scans every candidate (fully correct, just less pruned).
const TRACE_TIME_HINT_PADDING_NANOS: i64 = 3_600_000_000_000;

/// Sort order for `SpanQueryEngine::search_spans`. `Recent` is the default — newest spans
/// first, matching the UI's default trace/span explorer view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpanSort {
    /// `start_time_nanos DESC` — newest first.
    #[default]
    Recent,
    /// `duration_nanos DESC` (nulls last) — longest spans first.
    Slowest,
    /// `status_code DESC` then `start_time_nanos DESC` — error spans first, newest within a
    /// status tier.
    Errors,
}

/// A structured span search: a time window plus an optional grammar query, a sort mode, and
/// paging. Mirrors `crate::QueryRequest` (the logs request) but for spans.
#[derive(Debug, Clone)]
pub struct SpanQueryRequest {
    /// Inclusive lower bound of the `start_time_nanos` window, in epoch nanoseconds.
    pub start_ts_nanos: i64,
    /// Inclusive upper bound of the `start_time_nanos` window, in epoch nanoseconds.
    pub end_ts_nanos: i64,
    /// Optional parsed+resolved grammar query. Compiled to a DataFusion predicate. `None` means
    /// no grammar filter.
    pub query: Option<SpanResolvedQuery>,
    /// Sort order for results.
    pub sort: SpanSort,
    /// Maximum number of rows to return.
    pub limit: usize,
    /// Number of rows to skip (applied after sorting), for paging.
    pub offset: usize,
    /// Attribute keys to project onto trace-rollup rows (root-span attributes).
    /// Empty (default) ⇒ no attribute-map decode, hot path unchanged.
    pub projected_attributes: Vec<String>,
}

#[derive(Clone)]
pub struct SpanQueryEngine {
    hot_dir: PathBuf,
    schema: SpanSchema,
    /// See `crate::ManifestCache`. `Arc`-wrapped so a cheap `SpanQueryEngine::clone` — taken to
    /// move into `spawn_blocking` — shares the cache with `self` rather than forking it.
    manifest_cache: Arc<RwLock<Option<ManifestCache>>>,
}

impl SpanQueryEngine {
    pub fn new(hot_dir: PathBuf, schema: SpanSchema) -> Result<SpanQueryEngine, PhotonError> {
        Ok(SpanQueryEngine {
            hot_dir,
            schema,
            manifest_cache: Arc::new(RwLock::new(None)),
        })
    }

    /// The configured promoted-attribute names — used by callers to build a
    /// `photon_core::query::SpanFieldResolver` for the grammar.
    pub fn promoted_attributes(&self) -> &[String] {
        &self.schema.promoted
    }

    /// All spans of one trace, unordered. Prunes candidate files via the spans manifest (time
    /// overlap when a `time_hint` is given) then each file's `trace_id` bloom, and reads only the
    /// survivors — the caller assembles the tree from `parent_span_id`.
    ///
    /// Returns an empty vec when no file holds the trace (the API maps that to 404).
    ///
    /// Pruning (manifest `stat`/read + per-candidate `.idx` reads) is synchronous `std::fs` I/O;
    /// it runs in `spawn_blocking` (`trace_candidates`) so it never blocks a tokio worker thread.
    pub async fn get_trace(
        &self,
        trace_id: &str,
        time_hint: Option<i64>,
    ) -> Result<Vec<RecordBatch>, PhotonError> {
        let engine = self.clone();
        let trace_id_owned = trace_id.to_string();
        let surviving = spawn_blocking(move || engine.trace_candidates(&trace_id_owned, time_hint))
            .await
            .map_err(|e| PhotonError::Query(format!("get_trace prune task panicked: {e}")))??;
        if surviving.is_empty() {
            return Ok(Vec::new());
        }

        let ctx = session();
        let df = ctx
            .read_parquet(
                surviving,
                ParquetReadOptions::default().schema(self.schema.arrow.as_ref()),
            )
            .await
            .map_err(|e| PhotonError::Query(format!("failed to read spans parquet: {e}")))?;
        df.filter(col_ref(span_schema::TRACE_ID).eq(lit(trace_id.to_string())))
            .map_err(|e| PhotonError::Query(format!("failed to filter spans by trace_id: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("failed to collect trace spans: {e}")))
    }

    /// The `get_trace` prune step: spans-manifest time-overlap candidates (when `time_hint` is
    /// given) whose `trace_id` bloom admits `trace_id`. Synchronous; called via `spawn_blocking`.
    fn trace_candidates(
        &self,
        trace_id: &str,
        time_hint: Option<i64>,
    ) -> Result<Vec<String>, PhotonError> {
        let manifest = self.load_spans_manifest()?;
        let (start, end) = match time_hint {
            Some(t) => (
                t.saturating_sub(TRACE_TIME_HINT_PADDING_NANOS),
                t.saturating_add(TRACE_TIME_HINT_PADDING_NANOS),
            ),
            None => (i64::MIN, i64::MAX),
        };

        let mut surviving: Vec<String> = Vec::new();
        for entry in manifest.candidates(start, end) {
            if self.keep_span_candidate(entry, trace_id)? {
                surviving.push(
                    self.hot_dir
                        .join(&entry.path)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        Ok(surviving)
    }

    /// Keep a candidate iff its `trace_id` bloom reports the id as possibly-present. The bloom
    /// never false-negatives, so this only drops files that truly cannot hold the trace; a missing,
    /// unreadable, OR corrupt `.idx` is kept (correctness over pruning), exactly as the logs engine
    /// does — a torn sidecar never aborts the lookup or panics.
    fn keep_span_candidate(&self, entry: &FileEntry, trace_id: &str) -> Result<bool, PhotonError> {
        let idx_path = self
            .hot_dir
            .join(Storage::index_path_spans(entry.segment_id));
        let bytes = match std::fs::read(&idx_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            // Any other read error → keep (conservative pruning, log once per bad file).
            Err(e) => {
                eprintln!(
                    "photon-query: warning: keeping {idx_path:?}, spans skip index unreadable: {e}"
                );
                return Ok(true);
            }
        };
        match SkipIndex::from_bytes(&bytes) {
            Ok(index) => Ok(index.might_contain_token(trace_id)),
            // Corrupt/undecodable sidecar → keep the file (same rule as a missing one).
            Err(e) => {
                eprintln!(
                    "photon-query: warning: keeping {idx_path:?}, spans skip index corrupt: {e}"
                );
                Ok(true)
            }
        }
    }

    /// Manifest-only storage summary (no data scan): file/row counts, timestamp span, and the
    /// on-disk byte size of the Parquet files. A missing/empty manifest yields all-zero stats.
    pub fn storage_stats(&self) -> Result<crate::StorageStats, PhotonError> {
        let manifest = self.load_spans_manifest()?;
        let entries = manifest.entries();
        if entries.is_empty() {
            return Ok(crate::StorageStats::default());
        }
        let mut s = crate::StorageStats {
            file_count: entries.len() as u64,
            total_rows: 0,
            min_ts_nanos: i64::MAX,
            max_ts_nanos: i64::MIN,
            bytes: 0,
        };
        for e in entries {
            s.total_rows += e.row_count;
            s.min_ts_nanos = s.min_ts_nanos.min(e.min_ts_nanos);
            s.max_ts_nanos = s.max_ts_nanos.max(e.max_ts_nanos);
            // Prefer the Parquet size the compactor recorded at write time — pure manifest
            // arithmetic, no syscall. Legacy entries (written before `FileEntry.bytes`) carry
            // `bytes == 0`; stat() ONLY those so the footprint stays exact during the transition.
            if e.bytes > 0 {
                s.bytes += e.bytes;
            } else if let Ok(md) = std::fs::metadata(self.hot_dir.join(&e.path)) {
                s.bytes += md.len();
            }
        }
        Ok(s)
    }

    /// Load the spans manifest from the local hot store, or an empty one if absent. Cached —
    /// see `crate::ManifestCache` / `crate::cached_manifest`.
    pub(crate) fn load_spans_manifest(&self) -> Result<Arc<Manifest>, PhotonError> {
        let path = self.hot_dir.join(SPANS_MANIFEST_OBJECT_PATH);
        cached_manifest(&path, &self.manifest_cache)
    }

    /// Raw SQL over the full (unpruned) `spans` table: all `seg-*.parquet` under
    /// `hot_dir/data-spans/` are registered as one table named `spans`.
    pub async fn sql(&self, sql: &str) -> Result<Vec<RecordBatch>, PhotonError> {
        let ctx = session();
        let mut dir = self
            .hot_dir
            .join("data-spans")
            .to_string_lossy()
            .into_owned();
        if !dir.ends_with('/') {
            dir.push('/');
        }
        ctx.register_parquet("spans", &dir, ParquetReadOptions::default())
            .await
            .map_err(|e| PhotonError::Query(format!("failed to register spans table: {e}")))?;
        let df = ctx
            .sql(sql)
            .await
            .map_err(|e| PhotonError::Query(format!("failed to plan sql: {e}")))?;
        df.collect()
            .await
            .map_err(|e| PhotonError::Query(format!("failed to execute sql: {e}")))
    }

    /// The surviving Parquet file paths for a span search: spans-manifest time-overlap
    /// candidates that also pass skip-index pruning (`start_time_nanos` range, and — when the
    /// grammar has positive free-text — the `name`-token bloom). Conservative: a missing `.idx`
    /// sidecar or an unknown range keeps the file, so pruning can only ever drop files that
    /// definitely cannot match — never a real result.
    ///
    /// No service filter param: spans filter service via the grammar (`service:x`), and the
    /// skip index's service range is a min/max, not something the grammar's arbitrary predicate
    /// can safely prune against here — correctness over pruning, per the design brief.
    pub(crate) fn span_prune(&self, req: &SpanQueryRequest) -> Result<Vec<String>, PhotonError> {
        let manifest = self.load_spans_manifest()?;
        let text_tokens = span_text_tokens(req);
        let mut surviving: Vec<String> = Vec::new();
        for entry in manifest.candidates(req.start_ts_nanos, req.end_ts_nanos) {
            if !self.keep_span_search_candidate(entry, req, text_tokens.as_deref())? {
                continue;
            }
            surviving.push(
                self.hot_dir
                    .join(&entry.path)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        Ok(surviving)
    }

    /// Prune, then open the surviving spans Parquet files as one DataFrame (unfiltered — the
    /// caller applies `span_base_predicate`). `None` when nothing survives pruning, so the
    /// caller can return an empty/zero result without touching DataFusion.
    ///
    /// Pruning (manifest `stat`/read + per-candidate `.idx` reads) is synchronous `std::fs` I/O;
    /// it runs in `spawn_blocking` so it never blocks a tokio worker thread. `self` is cheap to
    /// clone (a `PathBuf`, a small `SpanSchema`, and an `Arc`-shared manifest cache), so the
    /// clone moved into the blocking closure still shares the manifest cache with `self`.
    pub(crate) async fn span_survivors_df(
        &self,
        req: &SpanQueryRequest,
    ) -> Result<Option<DataFrame>, PhotonError> {
        let engine = self.clone();
        let req = req.clone();
        let surviving = spawn_blocking(move || engine.span_prune(&req))
            .await
            .map_err(|e| PhotonError::Query(format!("span prune task panicked: {e}")))??;
        if surviving.is_empty() {
            return Ok(None);
        }
        let ctx = session();
        let df = ctx
            .read_parquet(
                surviving,
                ParquetReadOptions::default().schema(self.schema.arrow.as_ref()),
            )
            .await
            .map_err(|e| PhotonError::Query(format!("failed to read spans parquet files: {e}")))?;
        Ok(Some(df))
    }

    /// Decide whether a candidate file survives skip-index pruning for `search_spans` /
    /// `count_matching_spans`. Distinct from `keep_span_candidate` (used by `get_trace`, which
    /// prunes on the `trace_id` bloom — a value the manifest does not summarize, so it always
    /// needs the `.idx` sidecar): this checks the `start_time_nanos` overlap directly against
    /// the manifest `FileEntry` — no I/O, since the compactor populates
    /// `min_ts_nanos`/`max_ts_nanos` from the exact same skip-index range at write time
    /// (`SpanCompactor::write_file`; see `crate::QueryEngine::keep_candidate`'s doc comment for
    /// the logs-side equivalent) — plus, only when there is positive free-text, the
    /// `name`-token bloom, which the manifest does not carry. Conservative: a missing, unreadable,
    /// or corrupt `.idx` (when a bloom check is needed) keeps the file — never aborts the search.
    fn keep_span_search_candidate(
        &self,
        entry: &FileEntry,
        req: &SpanQueryRequest,
        text_tokens: Option<&[String]>,
    ) -> Result<bool, PhotonError> {
        // Timestamp overlap with [start, end]. `manifest.candidates()` already filtered on this
        // exact pair, so this is a cheap belt-and-suspenders re-check, not new pruning power.
        if entry.max_ts_nanos < req.start_ts_nanos || entry.min_ts_nanos > req.end_ts_nanos {
            return Ok(false);
        }

        // Every search token must be possibly-present in the bloom. No tokens to search → keep
        // without opening the sidecar at all.
        let Some(tokens) = text_tokens else {
            return Ok(true);
        };

        let idx_path = self
            .hot_dir
            .join(Storage::index_path_spans(entry.segment_id));
        let bytes = match std::fs::read(&idx_path) {
            Ok(b) => b,
            // No sidecar → cannot bloom-check → keep the file (correctness over pruning).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            // Any other read error → also keep, never abort the search (log once per bad file).
            Err(e) => {
                eprintln!(
                    "photon-query: warning: keeping {idx_path:?}, spans skip index unreadable: {e}"
                );
                return Ok(true);
            }
        };
        match SkipIndex::from_bytes(&bytes) {
            Ok(index) => Ok(index.might_contain_all(tokens)),
            // Corrupt/undecodable sidecar → keep the file (same rule as a missing one).
            Err(e) => {
                eprintln!(
                    "photon-query: warning: keeping {idx_path:?}, spans skip index corrupt: {e}"
                );
                Ok(true)
            }
        }
    }
}

/// The bloom-search tokens for a span request: the *interior* (both-sides-delimited) tokens of
/// every positive free-text term of the grammar query (e.g. bare words / quoted phrases matching
/// `name`), via `photon_index::interior_tokens`. `None` when there is nothing safe to bloom-test,
/// so pruning skips the bloom check and keeps the file.
///
/// Free-text over the span `name` is *substring* (`strpos(name, text) > 0`), identical to the logs
/// body case, so it shares the same interior-token rule as `crate::text_tokens` — only tokens
/// delimited on both sides within the search term are guaranteed whole in a matching name, so
/// bloom-testing an edge token would false-*negative* and drop a real result.
pub(crate) fn span_text_tokens(req: &SpanQueryRequest) -> Option<Vec<String>> {
    let mut tokens: Vec<String> = Vec::new();
    if let Some(rq) = &req.query {
        for t in rq.positive_freetext() {
            tokens.extend(interior_tokens(t));
        }
    }
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

/// The row predicate shared by `search_spans` and `count_matching_spans`: the
/// `start_time_nanos` window AND the grammar predicate (if any). No sort/cutoff/limit — callers
/// add their own. Bound literals only (no SQL string interpolation → no injection surface).
pub(crate) fn span_base_predicate(req: &SpanQueryRequest) -> Expr {
    let mut predicate = col_ref(span_schema::START_TIME).between(
        lit_timestamp_nano(req.start_ts_nanos),
        lit_timestamp_nano(req.end_ts_nanos),
    );
    if let Some(rq) = &req.query {
        predicate = predicate.and(crate::span_resolved_query_to_expr(rq));
    }
    predicate
}

#[cfg(test)]
mod tests {
    use super::*;

    use photon_core::query::{parse, SpanFieldResolver};

    #[test]
    fn constructs_over_a_hot_dir() {
        let engine = SpanQueryEngine::new(
            PathBuf::from("/tmp/does-not-exist"),
            SpanSchema::new(&["service.name".into()]),
        )
        .unwrap();
        assert!(engine.hot_dir.ends_with("does-not-exist"));
    }

    fn req_query(q: &str) -> SpanQueryRequest {
        let rq = SpanFieldResolver::new(&["service.name".to_string()])
            .resolve(&parse(q).unwrap())
            .unwrap();
        SpanQueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: i64::MAX,
            query: Some(rq),
            sort: SpanSort::Recent,
            limit: 10,
            offset: 0,
            projected_attributes: Vec::new(),
        }
    }

    #[test]
    fn storage_stats_prefers_recorded_bytes_and_falls_back_for_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let hot = tmp.path().to_path_buf();
        std::fs::create_dir_all(hot.join("manifest")).unwrap();
        std::fs::create_dir_all(hot.join("data-spans")).unwrap();
        // seg-a on disk is 100 bytes but its entry records 999 — a correct sum uses the recorded
        // field, not a stat. seg-b is legacy (bytes == 0) → its real 250 bytes come via stat().
        std::fs::write(hot.join("data-spans/seg-a.parquet"), vec![0u8; 100]).unwrap();
        std::fs::write(hot.join("data-spans/seg-b.parquet"), vec![0u8; 250]).unwrap();
        let mut m = Manifest::new();
        for (path, seg, bytes) in [
            ("data-spans/seg-a.parquet", 1u64, 999u64),
            ("data-spans/seg-b.parquet", 2, 0),
        ] {
            m.add(FileEntry {
                path: path.into(),
                segment_id: photon_core::segment::SegmentId(seg),
                min_ts_nanos: 0,
                max_ts_nanos: 100,
                min_service: "api".into(),
                max_service: "web".into(),
                row_count: 1,
                durable: false,
                attribute_keys: vec![],
                bytes,
            });
        }
        std::fs::write(hot.join(SPANS_MANIFEST_OBJECT_PATH), m.to_json().unwrap()).unwrap();

        let engine =
            SpanQueryEngine::new(hot, SpanSchema::new(&["service.name".to_string()])).unwrap();
        let s = engine.storage_stats().unwrap();
        assert_eq!(s.file_count, 2);
        // 999 (recorded field, proving no stat of the 100-byte file) + 250 (legacy fallback stat).
        assert_eq!(s.bytes, 999 + 250);
    }

    /// Spans free-text over `name` is substring semantics too, so `span_text_tokens` must share the
    /// interior-token rule: only both-sides-delimited tokens are safe to bloom-test.
    #[test]
    fn span_text_tokens_returns_only_interior_tokens() {
        // A quoted phrase is one free-text term → interior token is `checkout` (edges dropped).
        assert_eq!(
            span_text_tokens(&req_query("\"a checkout b\"")).unwrap(),
            vec!["checkout"]
        );
        // A single-word free-text term is edge-to-edge → nothing safe to bloom-test.
        assert!(span_text_tokens(&req_query("\"tim\"")).is_none());
        // A bare word is also a single edge-to-edge free-text term → no safe token.
        assert!(span_text_tokens(&req_query("checkout")).is_none());
    }
}

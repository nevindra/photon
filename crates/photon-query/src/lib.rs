//! photon-query: prune (manifest + per-file skip index) then read the surviving Parquet
//! files with DataFusion.
//!
//! Implemented per the `photon-query` section of
//! `docs/superpowers/plans/2026-07-01-photon-interface-contracts.md`, as refined by the
//! Wave-2 dispatch instructions.
//!
//! ## Milestone simplification — local filesystem only
//!
//! The hot store is always a local directory, so this engine reads the manifest, the
//! skip-index sidecars, and the Parquet files directly from the local filesystem. It does
//! **not** register an `object_store::ObjectStore` with DataFusion. Object-store-based
//! reads (for a future cold/durable tier) are intentionally out of scope here; when that
//! tier is added, `search`/`sql` would gain an object-store path selected per `FileEntry`.
//!
//! ## Design
//!
//! Pruning is our own code (manifest time-overlap + skip-index bloom/min-max); DataFusion is
//! used only on the well-trodden path of reading a chosen set of local Parquet files and
//! applying predicates. Only surviving files are ever opened.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use arrow::array::{Array, StringArray};
use arrow::record_batch::RecordBatch;
use datafusion::execution::memory_pool::GreedyMemoryPool;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::{
    lit, lit_timestamp_nano, strpos, Column, Expr, ParquetReadOptions, SessionConfig,
    SessionContext,
};
use tokio::task::spawn_blocking;

use photon_core::manifest::{Manifest, MANIFEST_OBJECT_PATH};
use photon_core::schema::LogSchema;
use photon_core::PhotonError;
use photon_index::{interior_tokens, SkipIndex};
use photon_storage::Storage;

mod predicate;
pub use predicate::resolved_query_to_expr;

mod bucket_math;

mod count;
mod facet;
mod fields;
mod histogram;
pub use facet::{FacetResult, FacetValue};
pub use fields::{FieldInfo, FieldKind};
pub use histogram::HistogramBucket;

mod span_engine;
pub use span_engine::{SpanQueryEngine, SpanQueryRequest, SpanSort};

mod metric_engine;
pub use metric_engine::MetricsQueryEngine;

mod metric_catalog;
pub use metric_catalog::{LabelsResult, MetricCatalogEntry, MetricMetadata};

mod metric_query;
pub use metric_query::{MetricSeriesRequest, QuerySeriesResult, SeriesPoint, SeriesResult};

mod metric_dist;

mod metric_classic_hist;

mod span_facet;

mod span_fields;

mod span_histogram;
pub use span_histogram::SpanHistogramBucket;

mod span_latency;
pub use span_latency::{LatencyBucket, LatencyHistogram};

mod span_predicate;
pub use span_predicate::span_resolved_query_to_expr;

mod metric_predicate;
pub use metric_predicate::metric_resolved_query_to_expr;

mod red;
pub use red::{RedGroup, RedRow};

mod rum_vitals;
pub use rum_vitals::{BreakdownRow, LcpAttribution, VitalSummary};

mod infra;
pub use infra::{HostDetail, HostSeries, HostSummary, InfraResource};

mod rum_errors;
pub use rum_errors::ErrorIssue;
pub use rum_errors::{CountBucket, ErrorDetail, ErrorEvent, TagBreakdown};

mod service_dependencies;
pub use service_dependencies::{DepRow, Dependencies};

mod red_timeseries;
pub use red_timeseries::RedBucket;

mod span_search;

mod trace_list;
pub use trace_list::{TraceSearchResult, TraceSummary};

/// Manifest-derived storage summary for one signal (no data scan). `bytes` is the on-disk
/// size of the Parquet files (skip-index sidecars excluded); an empty manifest yields zeros.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct StorageStats {
    pub file_count: u64,
    pub total_rows: u64,
    pub min_ts_nanos: i64,
    pub max_ts_nanos: i64,
    pub bytes: u64,
}

/// A structured log search: a time window plus optional service, severity, and free-text
/// filters. This mirrors the UI's search form; all filters are optional (an empty `services`
/// / `severities` means "no filter on that dimension").
#[derive(Debug, Clone)]
pub struct QueryRequest {
    /// Inclusive lower bound of the timestamp window, in epoch nanoseconds.
    pub start_ts_nanos: i64,
    /// Inclusive upper bound of the timestamp window, in epoch nanoseconds.
    pub end_ts_nanos: i64,
    /// Exact `service.name` filter — the row must match one of these. Empty means "any
    /// service". Also prunes files whose skip-index service range excludes *all* of them.
    pub services: Vec<String>,
    /// `severity_number` filter as a set of inclusive `[lo, hi]` ranges (already resolved
    /// from the UI's severity buckets by the caller). A row matches if it falls in any range.
    /// Empty means "any severity". Not used for file pruning (the skip index carries no
    /// severity stat), only as a row predicate.
    pub severities: Vec<(i32, i32)>,
    /// Optional free-text filter. Its *interior* (both-sides-delimited) tokens drive bloom
    /// pruning via `photon_index::interior_tokens` — edge tokens are excluded because substring
    /// semantics may match them as fragments of longer body words — then every row is confirmed
    /// with a `strpos(body, text) > 0` substring match.
    pub text: Option<String>,
    /// Optional parsed+resolved grammar query. Compiled to a DataFusion predicate and ANDed
    /// with the structured filters above. `None` means no grammar filter.
    pub query: Option<photon_core::query::ResolvedQuery>,
    /// Maximum number of rows to return (newest first).
    pub limit: usize,
}

/// A parsed manifest cached against the `(mtime, len)` of the file it was read from. The
/// compactor (same process) rewrites `manifest.json` roughly every 2s, and one UI page load can
/// fire 6-10 engine calls (search, count, facet, histogram, fields) that each used to re-read
/// and re-JSON-parse the manifest. `stat`-ing the file first and reparsing only when it changed
/// keeps results correct within one compactor tick of staleness — acceptable per the compactor's
/// own cadence — while turning most calls into a cheap `stat` instead of a read + JSON parse.
struct ManifestCache {
    mtime: Option<SystemTime>,
    len: u64,
    manifest: Arc<Manifest>,
}

/// Shared by `QueryEngine::load_manifest` and `SpanQueryEngine::load_spans_manifest`: `stat`
/// `path`, compare against `cache`, and reparse only when the file's `(mtime, len)` changed (or
/// the cache is empty). A missing file yields a fresh empty `Manifest`, uncached — cheap to
/// reconstruct, and caching "absent" would need its own invalidation signal (the manifest first
/// appears once the compactor drains its first WAL segment).
fn cached_manifest(
    path: &Path,
    cache: &RwLock<Option<ManifestCache>>,
) -> Result<Arc<Manifest>, PhotonError> {
    let stat = match std::fs::metadata(path) {
        Ok(m) => Some((m.len(), m.modified().ok())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(PhotonError::Query(format!(
                "failed to stat manifest {path:?}: {e}"
            )))
        }
    };
    let Some((len, mtime)) = stat else {
        return Ok(Arc::new(Manifest::new()));
    };

    if let Some(cached) = cache.read().unwrap().as_ref() {
        if cached.len == len && cached.mtime == mtime {
            return Ok(cached.manifest.clone());
        }
    }

    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        // Torn by a concurrent compactor write: treat like "absent" (self-heals next call, once
        // the stat and the read agree again).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Arc::new(Manifest::new())),
        Err(e) => {
            return Err(PhotonError::Query(format!(
                "failed to read manifest {path:?}: {e}"
            )))
        }
    };
    let manifest = Arc::new(Manifest::from_json(&s)?);
    *cache.write().unwrap() = Some(ManifestCache {
        mtime,
        len,
        manifest: manifest.clone(),
    });
    Ok(manifest)
}

/// Cached distinct `service.name` list, invalidated by comparing the `Arc<Manifest>` it was
/// computed from against the current (cached) manifest by pointer identity — see
/// `QueryEngine::distinct_services`.
struct ServicesCache {
    manifest: Arc<Manifest>,
    services: Arc<Vec<String>>,
}

/// Query engine over a local hot directory. Reads the manifest + skip indexes to prune the
/// Parquet files that could match, then runs the query over just those files with DataFusion.
#[derive(Clone)]
pub struct QueryEngine {
    hot_dir: PathBuf,
    /// The log schema. Retained for the future cold-tier / object-store read path (out of
    /// scope for this milestone, which reads a self-describing Parquet layout directly) and to
    /// hand DataFusion an explicit `read_parquet` schema, skipping per-query schema inference.
    schema: LogSchema,
    /// See `ManifestCache`. `Arc`-wrapped (not just the `RwLock`) so a cheap `QueryEngine::clone`
    /// — taken to move into `spawn_blocking` — shares the same cache rather than forking it.
    manifest_cache: Arc<RwLock<Option<ManifestCache>>>,
    /// See `ServicesCache`.
    services_cache: Arc<RwLock<Option<ServicesCache>>>,
}

impl QueryEngine {
    /// Construct an engine rooted at a local hot directory.
    ///
    /// `hot_dir` is the same directory the compactor writes to: it contains
    /// `manifest/manifest.json`, and a `data/` folder of `seg-*.parquet` + `seg-*.idx`
    /// sidecars. Reads go straight to the local filesystem — no object store is wired to
    /// DataFusion (see the module docs; the cold/durable tier is out of scope here).
    pub fn new(hot_dir: PathBuf, schema: LogSchema) -> Result<QueryEngine, PhotonError> {
        Ok(QueryEngine {
            hot_dir,
            schema,
            manifest_cache: Arc::new(RwLock::new(None)),
            services_cache: Arc::new(RwLock::new(None)),
        })
    }

    /// The configured promoted-attribute names — used by callers to build a
    /// `photon_core::query::FieldResolver` for the grammar.
    pub fn promoted_attributes(&self) -> &[String] {
        &self.schema.promoted
    }

    /// Prune → read → run.
    ///
    /// 1. Load the manifest from `hot_dir/manifest/manifest.json` (empty if absent) and take
    ///    the time-overlapping `candidates`.
    /// 2. For each candidate, load its `.idx` skip index and keep the file only if its
    ///    timestamp range overlaps the request window, its `service.name` range admits
    ///    `req.service` (when set), and its bloom `might_contain_all` the tokenized text
    ///    (when set).
    /// 3. Read the surviving Parquet in two passes (late materialization). Pass 1 applies the
    ///    time / service / severity / text predicates but projects **only** `timestamp`, sorts
    ///    it `DESC`, and takes `limit` — cheap, because the wide `attributes` map is never
    ///    decoded. The smallest timestamp of that top-`limit` is the `cutoff`. Pass 2 re-runs
    ///    the same predicates plus `timestamp >= cutoff` and returns full rows: the expensive
    ///    columns are now decoded for only ~`limit` rows instead of every scanned row.
    ///
    /// The result is identical to a single `... ORDER BY timestamp DESC LIMIT limit` — pass 2
    /// re-sorts and re-limits, so a tie on `cutoff` is trimmed exactly as before — but roughly
    /// an order of magnitude faster on broad windows where the sort/limit, not pruning, is what
    /// bounds the scan.
    ///
    /// Returns an empty vec when no file survives pruning, or when nothing matches pass 1.
    pub async fn search(&self, req: QueryRequest) -> Result<Vec<RecordBatch>, PhotonError> {
        let df = match self.survivors_df(&req).await? {
            Some(df) => df,
            None => return Ok(Vec::new()),
        };
        let predicate = base_predicate(&req);
        search_over(df, predicate, req.limit).await
    }

    /// `search` plus the true `matched_count` over the full pruned set, from ONE prune/open
    /// instead of two. The UI's search response needs both the (row-limited) page and the total
    /// match count; calling `search` then `count_matching` back to back each independently
    /// re-prunes the manifest/skip-indexes and re-opens every surviving Parquet file. This
    /// shares one `survivors_df` between the same two-pass `search` body (`search_over`, used by
    /// `search` itself) and `count_matching`'s `COUNT(*)` aggregate (`count::count_over`) by
    /// cloning the cheap `DataFrame` handle (a logical plan, not the data) rather than the prune.
    pub async fn search_with_count(
        &self,
        req: QueryRequest,
    ) -> Result<(Vec<RecordBatch>, u64), PhotonError> {
        let df = match self.survivors_df(&req).await? {
            Some(df) => df,
            None => return Ok((Vec::new(), 0)),
        };
        let predicate = base_predicate(&req);
        let rows = search_over(df.clone(), predicate.clone(), req.limit).await?;
        let matched = count::count_over(df, predicate).await?;
        Ok((rows, matched))
    }

    /// The surviving Parquet file paths for a request: manifest time-overlap candidates that also
    /// pass skip-index pruning (timestamp/service range + bloom). Conservative: a missing `.idx`
    /// keeps the file. Shared by all aggregations so they scan exactly what `search` would.
    pub(crate) fn prune(&self, req: &QueryRequest) -> Result<Vec<String>, PhotonError> {
        let manifest = self.load_manifest()?;
        let text_tokens = text_tokens(req);
        let mut surviving: Vec<String> = Vec::new();
        for entry in manifest.candidates(req.start_ts_nanos, req.end_ts_nanos) {
            if !self.keep_candidate(entry, req, text_tokens.as_deref())? {
                continue;
            }
            let parquet_path = self.hot_dir.join(&entry.path);
            surviving.push(parquet_path.to_string_lossy().into_owned());
        }
        Ok(surviving)
    }

    /// Prune, then open the surviving Parquet files as one DataFrame (unfiltered — the caller
    /// applies `base_predicate`). `None` when nothing survives pruning (so the caller returns an
    /// empty/zero result without touching DataFusion).
    ///
    /// Pruning (manifest `stat`/read + per-candidate `.idx` reads) is synchronous `std::fs` I/O;
    /// it runs in `spawn_blocking` so it never blocks a tokio worker thread. `self` is cheap to
    /// clone (a `PathBuf`, a small `LogSchema`, and two `Arc`-shared caches), so the clone moved
    /// into the blocking closure still shares the manifest/services caches with `self`.
    pub(crate) async fn survivors_df(
        &self,
        req: &QueryRequest,
    ) -> Result<Option<datafusion::dataframe::DataFrame>, PhotonError> {
        let engine = self.clone();
        let req = req.clone();
        let surviving = spawn_blocking(move || engine.prune(&req))
            .await
            .map_err(|e| PhotonError::Query(format!("prune task panicked: {e}")))??;
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
            .map_err(|e| PhotonError::Query(format!("failed to read parquet files: {e}")))?;
        Ok(Some(df))
    }

    /// Power-user raw SQL over the full (unpruned) `logs` table. All `seg-*.parquet` files
    /// under `hot_dir/data/` are registered as a single table named `logs`. Pruning applies
    /// only to `search`.
    pub async fn sql(&self, sql: &str) -> Result<Vec<RecordBatch>, PhotonError> {
        let ctx = session();

        // Register the whole `data/` directory as a listing table. A trailing slash marks it
        // as a directory (collection); `ParquetReadOptions::default()` filters to `.parquet`,
        // so the `.idx` sidecars in the same folder are ignored.
        let mut dir = self.hot_dir.join("data").to_string_lossy().into_owned();
        if !dir.ends_with('/') {
            dir.push('/');
        }
        ctx.register_parquet("logs", &dir, ParquetReadOptions::default())
            .await
            .map_err(|e| PhotonError::Query(format!("failed to register logs table: {e}")))?;

        let df = ctx
            .sql(sql)
            .await
            .map_err(|e| PhotonError::Query(format!("failed to plan sql: {e}")))?;
        df.collect()
            .await
            .map_err(|e| PhotonError::Query(format!("failed to execute sql: {e}")))
    }

    /// Manifest-only storage summary (no data scan): file/row counts, timestamp span, and the
    /// on-disk byte size of the Parquet files. A missing/empty manifest yields all-zero stats.
    pub fn storage_stats(&self) -> Result<StorageStats, PhotonError> {
        let manifest = self.load_manifest()?;
        let entries = manifest.entries();
        if entries.is_empty() {
            return Ok(StorageStats::default());
        }
        let mut s = StorageStats {
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

    /// Load the manifest from the local hot store, or an empty one if the file is absent.
    /// Cached — see `ManifestCache` / `cached_manifest`.
    fn load_manifest(&self) -> Result<Arc<Manifest>, PhotonError> {
        let path = self.hot_dir.join(MANIFEST_OBJECT_PATH);
        cached_manifest(&path, &self.manifest_cache)
    }

    /// `load_manifest`, off the async runtime (a `stat` and, on a cache miss, a file read +
    /// JSON parse — all synchronous `std::fs`/`serde_json` work). Used by callers, like
    /// `distinct_services`, that need the manifest without going through `prune`.
    async fn load_manifest_async(&self) -> Result<Arc<Manifest>, PhotonError> {
        let engine = self.clone();
        spawn_blocking(move || engine.load_manifest())
            .await
            .map_err(|e| PhotonError::Query(format!("manifest load task panicked: {e}")))?
    }

    /// Decide whether a candidate file survives pruning. Timestamp overlap and the requested
    /// service(s) are checked directly against the manifest `FileEntry` — no I/O — because the
    /// compactor populates `min_ts_nanos`/`max_ts_nanos`/`min_service`/`max_service` from the
    /// exact same skip-index range at write time (`Compactor::write_file`), so re-deriving them
    /// from the `.idx` sidecar here would just recompute values the manifest already carries.
    /// Only the bloom filter — which the manifest does not summarize — needs the `.idx` file,
    /// and only when there is text to search. Conservative: a missing, unreadable, OR corrupt
    /// `.idx` (when a bloom check is needed) keeps the file — pruning can only ever drop files
    /// that definitely cannot match, never a real result, and a torn sidecar never aborts the
    /// search or panics.
    fn keep_candidate(
        &self,
        entry: &photon_core::manifest::FileEntry,
        req: &QueryRequest,
        text_tokens: Option<&[String]>,
    ) -> Result<bool, PhotonError> {
        // Timestamp overlap with [start, end]. `manifest.candidates()` already filtered on this
        // exact pair, so this is a cheap belt-and-suspenders re-check, not new pruning power.
        if entry.max_ts_nanos < req.start_ts_nanos || entry.min_ts_nanos > req.end_ts_nanos {
            return Ok(false);
        }

        // At least one requested service must fall within the file's [min, max] service
        // range, else the file cannot contain any of them. The range is a min/max, not the
        // exact set, so this can only false-*positive* (keep a file that turns out empty),
        // never drop a real match.
        if !req.services.is_empty() {
            let any_in_range = req.services.iter().any(|s| {
                s.as_str() >= entry.min_service.as_str() && s.as_str() <= entry.max_service.as_str()
            });
            if !any_in_range {
                return Ok(false);
            }
        }

        // Every search token must be possibly-present in the bloom. No tokens to search → keep
        // without opening the sidecar at all.
        let Some(tokens) = text_tokens else {
            return Ok(true);
        };

        let idx_path = self.hot_dir.join(Storage::index_path(entry.segment_id));
        let bytes = match std::fs::read(&idx_path) {
            Ok(b) => b,
            // No sidecar → cannot bloom-check → keep the file (correctness over pruning).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            // Any OTHER read error (torn file, permissions, transient I/O) → also keep. A bad
            // sidecar must never drop a real result or abort the whole search — pruning stays
            // conservative. Log once per bad file (this runs once per candidate, not per row).
            Err(e) => {
                eprintln!(
                    "photon-query: warning: keeping {idx_path:?}, skip index unreadable: {e}"
                );
                return Ok(true);
            }
        };
        match SkipIndex::from_bytes(&bytes) {
            Ok(index) => Ok(index.might_contain_all(tokens)),
            // Corrupt/undecodable sidecar → keep the file, same conservative rule as a missing one
            // (decode already rejects the frames that used to panic the bloom at query time).
            Err(e) => {
                eprintln!("photon-query: warning: keeping {idx_path:?}, skip index corrupt: {e}");
                Ok(true)
            }
        }
    }

    /// Distinct `service.name` values across the whole (unpruned — there is no time window)
    /// dataset, cached and invalidated exactly when the manifest cache refreshes: compared by
    /// `Arc` pointer identity against `load_manifest`'s result, which only allocates a new `Arc`
    /// when the manifest file's `(mtime, len)` changed. An empty manifest short-circuits to an
    /// empty list with no Parquet I/O at all. Replaces the API layer's previous pattern of
    /// running `SELECT DISTINCT service.name FROM logs` (an unpruned full-directory scan) on
    /// every call with one scan per manifest change.
    ///
    /// A file's skip-index only carries a `[min, max]` service range, not the exact distinct
    /// set, so — unlike `keep_candidate` — this cannot be answered from the manifest alone on a
    /// cache miss; scanning the actual columns is the only correct way to get the true set.
    pub async fn distinct_services(&self) -> Result<Arc<Vec<String>>, PhotonError> {
        let manifest = self.load_manifest_async().await?;
        if manifest.entries().is_empty() {
            return Ok(Arc::new(Vec::new()));
        }
        if let Some(cached) = self.services_cache.read().unwrap().as_ref() {
            if Arc::ptr_eq(&cached.manifest, &manifest) {
                return Ok(cached.services.clone());
            }
        }

        let paths: Vec<String> = manifest
            .entries()
            .iter()
            .map(|e| self.hot_dir.join(&e.path).to_string_lossy().into_owned())
            .collect();
        let ctx = session();
        let df = ctx
            .read_parquet(
                paths,
                ParquetReadOptions::default().schema(self.schema.arrow.as_ref()),
            )
            .await
            .map_err(|e| {
                PhotonError::Query(format!("failed to read parquet for distinct services: {e}"))
            })?;
        let batches = df
            .select(vec![col_ref("service.name")])
            .map_err(|e| PhotonError::Query(format!("distinct services select: {e}")))?
            .distinct()
            .map_err(|e| PhotonError::Query(format!("distinct services distinct: {e}")))?
            .sort(vec![col_ref("service.name").sort(true, false)])
            .map_err(|e| PhotonError::Query(format!("distinct services sort: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("distinct services collect: {e}")))?;

        let mut services = Vec::new();
        for b in &batches {
            let col = b
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PhotonError::Query("distinct services column not Utf8".into()))?;
            for i in 0..col.len() {
                if !col.is_null(i) {
                    services.push(col.value(i).to_string());
                }
            }
        }

        let services = Arc::new(services);
        *self.services_cache.write().unwrap() = Some(ServicesCache {
            manifest,
            services: services.clone(),
        });
        Ok(services)
    }
}

/// The shared two-pass late-materialized search body — see `QueryEngine::search`'s doc comment
/// for the full rationale. Extracted so `QueryEngine::search_with_count` can run it against the
/// same survivors `DataFrame` it also feeds to `count::count_over`, instead of `search` and
/// `count_matching` each re-pruning and re-opening the candidate set independently.
async fn search_over(
    df: datafusion::dataframe::DataFrame,
    predicate: Expr,
    limit: usize,
) -> Result<Vec<RecordBatch>, PhotonError> {
    let ts_sort = || col_ref(photon_core::schema::TIMESTAMP).sort(false, false);

    // Pass 1 — cheap cutoff probe. Project only `timestamp`, so the wide `attributes` map (and
    // other columns) are never decoded for the millions of rows we won't return.
    let cutoff_batches = df
        .clone()
        .filter(predicate.clone())
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .select(vec![col_ref(photon_core::schema::TIMESTAMP)])
        .map_err(|e| PhotonError::Query(format!("failed to project timestamp: {e}")))?
        .sort(vec![ts_sort()])
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(0, Some(limit))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect cutoff: {e}")))?;

    // The cutoff is the smallest timestamp among the newest `limit` matches. Nothing matched
    // (or `limit` was 0) → no rows.
    let cutoff = match min_timestamp(&cutoff_batches) {
        Some(c) => c,
        None => return Ok(Vec::new()),
    };

    // Pass 2 — full rows, but only from `[cutoff, end]`. Re-applying the predicate plus
    // `timestamp >= cutoff` and re-sorting/limiting yields exactly the single-pass result (ties
    // on `cutoff` trimmed by the limit), while decoding the heavy columns for only the rows at
    // or above the cutoff.
    let predicate =
        predicate.and(col_ref(photon_core::schema::TIMESTAMP).gt_eq(lit_timestamp_nano(cutoff)));
    df.filter(predicate)
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .sort(vec![ts_sort()])
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(0, Some(limit))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect results: {e}")))
}

/// Smallest timestamp (epoch nanos) across single-column `timestamp` batches, or `None` when
/// there are no rows. Reads back the pass-1 cutoff (the `limit`-th newest matching timestamp).
fn min_timestamp(batches: &[RecordBatch]) -> Option<i64> {
    use arrow::array::TimestampNanosecondArray;
    let mut min: Option<i64> = None;
    for batch in batches {
        let col = batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()?;
        for i in 0..col.len() {
            if !col.is_null(i) {
                let v = col.value(i);
                min = Some(min.map_or(v, |m| m.min(v)));
            }
        }
    }
    min
}

/// The bloom-search tokens for a request: the *interior* (both-sides-delimited) tokens of `req.text`
/// plus those of every positive free-text term of the grammar query. `None` when there is nothing
/// safe to bloom-test (so pruning skips the bloom check and keeps the file).
///
/// Free-text search is *substring* (`strpos(body, text) > 0`), so only tokens delimited on both
/// sides *within the search string itself* are guaranteed to appear as whole tokens in a matching
/// body — see `photon_index::interior_tokens`. Bloom-testing an edge token (which may be a fragment
/// of a longer body word, e.g. `tim` in `timeout`) would false-*negative* and silently drop a real
/// result, violating the pruning-is-conservative invariant. A single-word search therefore
/// contributes no bloom token and is confirmed entirely by the row predicate.
pub(crate) fn text_tokens(req: &QueryRequest) -> Option<Vec<String>> {
    let mut tokens: Vec<String> = req.text.as_deref().map(interior_tokens).unwrap_or_default();
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

/// The row predicate shared by `search`, `count`, `facet`, and `histogram`: timestamp window AND
/// optional service / severity / free-text / grammar filters. No sort/cutoff/limit — callers add
/// their own. Bound literals only (no SQL string interpolation → no injection surface).
pub(crate) fn base_predicate(req: &QueryRequest) -> Expr {
    let mut predicate = col_ref(photon_core::schema::TIMESTAMP).between(
        lit_timestamp_nano(req.start_ts_nanos),
        lit_timestamp_nano(req.end_ts_nanos),
    );
    if !req.services.is_empty() {
        let list = req.services.iter().map(|s| lit(s.clone())).collect();
        predicate = predicate.and(col_ref("service.name").in_list(list, false));
    }
    if !req.severities.is_empty() {
        let mut sev: Option<Expr> = None;
        for (lo, hi) in &req.severities {
            let range = col_ref(photon_core::schema::SEVERITY_NUMBER).between(lit(*lo), lit(*hi));
            sev = Some(match sev {
                Some(acc) => acc.or(range),
                None => range,
            });
        }
        if let Some(sev) = sev {
            predicate = predicate.and(sev);
        }
    }
    if let Some(text) = &req.text {
        predicate = predicate
            .and(strpos(col_ref(photon_core::schema::BODY), lit(text.clone())).gt(lit(0_i64)));
    }
    if let Some(rq) = &req.query {
        predicate = predicate.and(crate::predicate::resolved_query_to_expr(rq));
    }
    predicate
}

/// Reference a column by its exact field name, without qualifier parsing.
///
/// `datafusion::prelude::col` splits on `.`, so `col("service.name")` would resolve to
/// relation `service` / column `name` and never match the field literally named
/// `service.name`. `Column::new_unqualified` keeps the whole string as the field name.
pub(crate) fn col_ref(name: &str) -> Expr {
    Expr::Column(Column::new_unqualified(name))
}

/// A `SessionContext` configured to read Parquet Utf8 columns as `Utf8` (`StringArray`)
/// rather than DataFusion 43's default `Utf8View` (`StringViewArray`). This keeps the
/// engine's output arrays aligned with `LogSchema` (which is Utf8) so downstream consumers
/// get conventional, stable array types. Also tunes DataFusion 43's Parquet scan defaults,
/// which are conservative out of the box:
/// - `pushdown_filters` + `reorder_filters`: apply (and heuristically order) filter
///   expressions during Parquet decoding — late materialization — instead of decoding every
///   column for every row before filtering. `reorder_filters` only matters once pushdown is on,
///   so the two are set together.
/// - `metadata_size_hint`: speculatively fetch the last 512KiB of each file in one read instead
///   of a separate footer-length round trip followed by a second read for the metadata.
///
/// It also installs a **bounded** [`GreedyMemoryPool`] (`QUERY_MEMORY_POOL_BYTES`) on the
/// `RuntimeEnv`, so the RAM any single query's *tracked* operators may reserve is capped. This is
/// the fail-loud guard for the unbounded-memory query paths — the facet `GROUP BY value` (holds
/// every distinct value in a hash table before the `LIMIT` trims it) and the metrics
/// pointwise/distribution scans (`filter → sort → collect()` every matching row) — which on a
/// low-memory single node could otherwise OOM-kill the process. With the pool they error with a
/// DataFusion `ResourcesExhausted` instead. Every engine shares this one factory (logs/spans/
/// metrics), so the ceiling applies everywhere.
pub(crate) fn session() -> SessionContext {
    session_with_memory_pool_bytes(QUERY_MEMORY_POOL_BYTES)
}

/// Per-`SessionContext` DataFusion memory-pool budget, in bytes. 512 MiB: generous enough that
/// any legitimate single-query working set fits (the whole `photon-query` test suite runs real
/// queries on KB-scale data, orders of magnitude below this) yet small enough to cap a runaway
/// high-cardinality query on a low-memory single node. There is no per-deployment config seam yet
/// — `session()` is a zero-arg factory shared by every engine — so this is a deliberate constant
/// for this pass; a future change could thread a configurable `[query].memory_pool_bytes` through
/// the engine constructors.
pub(crate) const QUERY_MEMORY_POOL_BYTES: usize = 512 * 1024 * 1024;

/// [`session()`] with an explicit memory-pool budget — the seam `session()` delegates through, so
/// tests can build a tiny-pool context and prove the unbounded paths fail loud (rather than having
/// to actually allocate hundreds of MB). All the Parquet tuning above is applied identically.
pub(crate) fn session_with_memory_pool_bytes(pool_bytes: usize) -> SessionContext {
    let config = SessionConfig::new()
        .set_bool(
            "datafusion.execution.parquet.schema_force_view_types",
            false,
        )
        .set_bool("datafusion.execution.parquet.pushdown_filters", true)
        .set_bool("datafusion.execution.parquet.reorder_filters", true)
        .set_usize(
            "datafusion.execution.parquet.metadata_size_hint",
            512 * 1024,
        );
    // `build_arc` only fails if the default disk/cache managers can't initialize, which does not
    // happen with the default config — hence `expect` rather than threading a Result through every
    // engine's `session()` call site.
    let runtime = RuntimeEnvBuilder::new()
        .with_memory_pool(Arc::new(GreedyMemoryPool::new(pool_bytes)))
        .build_arc()
        .expect("RuntimeEnv with a memory pool builds under the default disk/cache config");
    SessionContext::new_with_config_rt(config, runtime)
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use datafusion::execution::memory_pool::MemoryConsumer;

    /// Part 1 of the memory-bound fix: `session()` must ADD a bounded memory pool while
    /// PRESERVING every existing Parquet tuning flag. A regression that dropped the flags (or
    /// left the pool unbounded) is exactly the failure this pins.
    #[test]
    fn session_carries_parquet_tuning_and_a_bounded_memory_pool() {
        let ctx = session();
        let cfg = ctx.copied_config();
        let parquet = &cfg.options().execution.parquet;

        // Existing config — must be untouched by the memory-pool addition.
        assert!(
            !parquet.schema_force_view_types,
            "Utf8 not Utf8View must be preserved"
        );
        assert!(parquet.pushdown_filters, "pushdown_filters must stay on");
        assert!(parquet.reorder_filters, "reorder_filters must stay on");
        assert_eq!(
            parquet.metadata_size_hint,
            Some(512 * 1024),
            "metadata_size_hint must be preserved"
        );

        // New: a *bounded* pool. An `UnboundedMemoryPool` accepts any reservation; ours must
        // reject one larger than its budget. `try_grow` is pure arithmetic — nothing is actually
        // allocated — so probing the boundary is cheap even at the 512 MiB default.
        let pool = ctx.runtime_env().memory_pool.clone();
        let mut res = MemoryConsumer::new("session_bound_probe").register(&pool);
        assert!(
            res.try_grow(QUERY_MEMORY_POOL_BYTES + 1).is_err(),
            "memory pool must be bounded (reject a reservation above its budget), not unbounded"
        );
    }
}

#[cfg(test)]
mod grammar_wiring_tests {
    use super::*;
    use photon_core::schema::LogSchema;

    #[test]
    fn exposes_promoted_attributes() {
        let schema = LogSchema::new(&["service.name".into(), "host.name".into()]);
        let engine =
            QueryEngine::new(std::path::PathBuf::from("/tmp/does-not-exist"), schema).unwrap();
        assert_eq!(
            engine.promoted_attributes(),
            &["service.name".to_string(), "host.name".to_string()]
        );
    }
}

#[cfg(test)]
mod storage_stats_tests {
    use super::*;
    use photon_core::schema::LogSchema;

    #[tokio::test]
    async fn storage_stats_summarizes_manifest_and_bytes() {
        use photon_core::manifest::{FileEntry, Manifest, MANIFEST_OBJECT_PATH};
        let tmp = tempfile::tempdir().unwrap();
        let hot = tmp.path().to_path_buf();
        std::fs::create_dir_all(hot.join("manifest")).unwrap();
        std::fs::create_dir_all(hot.join("data")).unwrap();
        // Two fake parquet files with known byte sizes.
        std::fs::write(hot.join("data/seg-a.parquet"), vec![0u8; 100]).unwrap();
        std::fs::write(hot.join("data/seg-b.parquet"), vec![0u8; 250]).unwrap();
        let mut m = Manifest::new();
        for (path, seg, mn, mx, rows) in [
            ("data/seg-a.parquet", 1u64, 100i64, 200i64, 10u64),
            ("data/seg-b.parquet", 2, 5000, 6000, 4),
        ] {
            m.add(FileEntry {
                path: path.into(),
                segment_id: photon_core::segment::SegmentId(seg),
                min_ts_nanos: mn,
                max_ts_nanos: mx,
                min_service: "api".into(),
                max_service: "web".into(),
                row_count: rows,
                durable: false,
                attribute_keys: vec![],
                // Legacy entries (bytes == 0) → the footprint comes from the stat() fallback.
                bytes: 0,
            });
        }
        std::fs::write(hot.join(MANIFEST_OBJECT_PATH), m.to_json().unwrap()).unwrap();

        let engine = QueryEngine::new(hot, LogSchema::new(&["service.name".to_string()])).unwrap();
        let s = engine.storage_stats().unwrap();
        assert_eq!(s.file_count, 2);
        assert_eq!(s.total_rows, 14);
        assert_eq!(s.min_ts_nanos, 100);
        assert_eq!(s.max_ts_nanos, 6000);
        // Both entries are legacy (bytes == 0), so the total is the sum of the real on-disk sizes
        // via the stat() fallback: 100 + 250 = 350.
        assert_eq!(s.bytes, 350);
    }

    #[tokio::test]
    async fn storage_stats_prefers_recorded_bytes_and_falls_back_for_legacy() {
        use photon_core::manifest::{FileEntry, Manifest, MANIFEST_OBJECT_PATH};
        let tmp = tempfile::tempdir().unwrap();
        let hot = tmp.path().to_path_buf();
        std::fs::create_dir_all(hot.join("manifest")).unwrap();
        std::fs::create_dir_all(hot.join("data")).unwrap();
        // seg-a on disk is 100 bytes but its entry records 999 — so a correct sum must use the
        // recorded field (999), NOT a stat() of the file (100). seg-b is legacy (bytes == 0) and
        // must contribute its real 250-byte on-disk size via the fallback.
        std::fs::write(hot.join("data/seg-a.parquet"), vec![0u8; 100]).unwrap();
        std::fs::write(hot.join("data/seg-b.parquet"), vec![0u8; 250]).unwrap();
        let mut m = Manifest::new();
        for (path, seg, rows, bytes) in [
            ("data/seg-a.parquet", 1u64, 10u64, 999u64),
            ("data/seg-b.parquet", 2, 4, 0),
        ] {
            m.add(FileEntry {
                path: path.into(),
                segment_id: photon_core::segment::SegmentId(seg),
                min_ts_nanos: 0,
                max_ts_nanos: 100,
                min_service: "api".into(),
                max_service: "web".into(),
                row_count: rows,
                durable: false,
                attribute_keys: vec![],
                bytes,
            });
        }
        std::fs::write(hot.join(MANIFEST_OBJECT_PATH), m.to_json().unwrap()).unwrap();

        let engine = QueryEngine::new(hot, LogSchema::new(&["service.name".to_string()])).unwrap();
        let s = engine.storage_stats().unwrap();
        // 999 (recorded field, proving no stat of the 100-byte file) + 250 (legacy fallback stat).
        assert_eq!(s.bytes, 999 + 250);
    }
}

#[cfg(test)]
mod text_token_pruning_tests {
    //! `text_tokens` must return only *interior* (both-sides-delimited) tokens, and the bloom
    //! step of `keep_candidate` (`might_contain_all`) must NEVER prune a file whose body actually
    //! contains the search text as a substring. This guards the platform's #1 invariant: pruning
    //! may false-*positive* but must never false-*negative*.
    use super::*;
    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;
    use photon_index::SkipIndex;

    fn req_text(text: &str) -> QueryRequest {
        QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: i64::MAX,
            services: Vec::new(),
            severities: Vec::new(),
            text: Some(text.to_string()),
            query: None,
            limit: 10,
        }
    }

    /// The safe (interior) tokens `text_tokens` derives for a plain free-text search string.
    fn tokens_of(text: &str) -> Vec<String> {
        text_tokens(&req_text(text)).unwrap_or_default()
    }

    #[test]
    fn single_word_search_has_no_interior_token() {
        // The whole string is one edge-to-edge token: it could be a fragment of a longer body
        // word (`tim` ⊂ `timeout`, `timeout` ⊂ `timeouts`), so nothing is safe to bloom-test.
        assert!(text_tokens(&req_text("tim")).is_none());
        assert!(text_tokens(&req_text("timeout")).is_none());
    }

    #[test]
    fn two_words_are_both_edge_tokens() {
        // `foo` is first (left edge continues into the body), `bar` is last (right edge) → neither
        // is interior → no safe token.
        assert!(text_tokens(&req_text("foo bar")).is_none());
    }

    #[test]
    fn keeps_only_the_interior_delimited_tokens() {
        assert_eq!(tokens_of("a foo b"), vec!["foo"]);
        assert_eq!(tokens_of("error timeout id:5"), vec!["timeout", "id"]);
        // Leading/trailing punctuation delimits the outer tokens too, so both become interior.
        assert_eq!(tokens_of("  --foo__bar--  "), vec!["foo", "bar"]);
        // A fully-delimited single interior word is safe.
        assert_eq!(tokens_of(" foo "), vec!["foo"]);
    }

    #[test]
    fn interior_tokens_are_lowercased_like_the_index() {
        assert_eq!(tokens_of("x TiMeOut y"), vec!["timeout"]);
    }

    /// Splitmix64 — a tiny deterministic PRNG so a failure reproduces exactly (no `rand` dep).
    struct SplitMix64(u64);
    impl SplitMix64 {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// Anti-false-negative property test (hand-rolled, seeded — this crate has no `proptest`).
    /// For many random bodies and many random substrings *of those bodies*, the file whose `.idx`
    /// bloom was built from the body must NEVER be pruned by the `text_tokens` → `might_contain_all`
    /// path. Mirrors `photon-index`'s `bloom_never_reports_a_false_negative`, but exercises the
    /// query-side token narrowing (the layer that regressed) rather than the raw bloom.
    #[test]
    fn pruning_never_drops_a_real_substring_match() {
        // Mixes letters, digits, spaces and punctuation so both tokens and delimiters occur, and
        // words frequently abut so substrings routinely slice through the middle of a token.
        let alphabet: Vec<char> = "abcde012 _-.:/".chars().collect();
        let schema = LogSchema::new(&[]);
        let mut rng = SplitMix64(0x0DDB_1A5E_5EED_1234);

        for _ in 0..500 {
            let body_len = rng.below(40);
            let body: String = (0..body_len)
                .map(|_| alphabet[rng.below(alphabet.len())])
                .collect();

            let mut builder = RecordBatchBuilder::new(&schema);
            builder.append(&LogRecord {
                timestamp_nanos: 0,
                body: Some(body.clone()),
                ..Default::default()
            });
            let batch = builder.finish().unwrap();
            let index = SkipIndex::build(&batch, &schema).unwrap();

            let chars: Vec<char> = body.chars().collect();
            if chars.is_empty() {
                continue;
            }
            for _ in 0..8 {
                let a = rng.below(chars.len());
                let b = rng.below(chars.len());
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                let substr: String = chars[lo..=hi].iter().collect();

                // Exactly what `keep_candidate` does for the bloom step: no safe tokens ⇒ keep.
                let keep = match text_tokens(&req_text(&substr)) {
                    None => true,
                    Some(tokens) => index.might_contain_all(&tokens),
                };
                assert!(
                    keep,
                    "pruned a real substring match: body={body:?} substr={substr:?} tokens={:?}",
                    text_tokens(&req_text(&substr))
                );
            }
        }
    }
}

#[cfg(test)]
mod corrupt_idx_keep_tests {
    //! A corrupt/undecodable `.idx` must KEEP its file, never drop it, abort the prune, or panic —
    //! the platform's #1 conservative-pruning invariant applied to the error/decode path.
    use super::*;
    use photon_core::manifest::{FileEntry, Manifest, MANIFEST_OBJECT_PATH};
    use photon_core::schema::LogSchema;
    use photon_core::segment::SegmentId;

    /// A well-framed but corrupt `.idx`: valid magic + version, but `num_bits = 0`. Before the
    /// decode hardening this decoded to a poisoned bloom whose first membership probe divided by
    /// zero — so `keep_candidate`'s `might_contain_all` panicked at query time.
    fn corrupt_zero_bits_idx() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"PXSK");
        b.push(2); // version
        b.extend_from_slice(&0u64.to_le_bytes()); // num_bits = 0 (poison)
        b.extend_from_slice(&1u32.to_le_bytes()); // num_hashes
        b.extend_from_slice(&0u64.to_le_bytes()); // bits_len = 0
        b.push(0); // has_timestamp = 0
        b.push(0); // has_service = 0
        b.push(0); // has_host = 0
        b
    }

    #[test]
    fn prune_keeps_a_candidate_whose_idx_fails_to_decode() {
        let tmp = tempfile::tempdir().unwrap();
        let hot = tmp.path().to_path_buf();
        let seg = SegmentId(7);

        // Corrupt sidecar on disk (no parquet needed — prune stops at keep_candidate).
        let idx_path = hot.join(Storage::index_path(seg));
        std::fs::create_dir_all(idx_path.parent().unwrap()).unwrap();
        std::fs::write(&idx_path, corrupt_zero_bits_idx()).unwrap();

        // Manifest with one entry pointing at that segment.
        let mut m = Manifest::new();
        m.add(FileEntry {
            path: Storage::parquet_path(seg),
            segment_id: seg,
            min_ts_nanos: 0,
            max_ts_nanos: 1_000,
            min_service: "svc".into(),
            max_service: "svc".into(),
            row_count: 1,
            durable: false,
            attribute_keys: vec![],
            bytes: 0,
        });
        let man_path = hot.join(MANIFEST_OBJECT_PATH);
        std::fs::create_dir_all(man_path.parent().unwrap()).unwrap();
        std::fs::write(&man_path, m.to_json().unwrap()).unwrap();

        let engine =
            QueryEngine::new(hot.clone(), LogSchema::new(&["service.name".to_string()])).unwrap();

        // `"x alpha y"` has one interior token (`alpha`), which forces `keep_candidate` to open —
        // and fail to decode — the sidecar. Conservative pruning must KEEP the file, returning it
        // in the survivor set rather than erroring or panicking.
        let req = QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 1_000,
            services: vec![],
            severities: vec![],
            text: Some("x alpha y".to_string()),
            query: None,
            limit: 10,
        };
        let surviving = engine.prune(&req).unwrap();
        let expected = hot
            .join(Storage::parquet_path(seg))
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            surviving,
            vec![expected],
            "a candidate with a corrupt .idx must be kept, not pruned or errored"
        );
    }
}

//! Metrics query engine: raw SQL over `hot_dir/data-metrics/` (registered as the DataFusion table
//! `metrics`) plus the Phase-2 pruning path — manifest time-overlap → per-file `metric_name` bloom
//! → `read_parquet(surviving, schema)`. Mirrors `QueryEngine::prune`/`survivors_df` and the spans
//! `trace_id`-bloom path, differing only in the single-token (`metric_name`) bloom check.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use arrow::array::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::{lit, lit_timestamp_nano, Expr, ParquetReadOptions};
use tokio::task::spawn_blocking;

use photon_core::manifest::{FileEntry, Manifest, METRICS_MANIFEST_OBJECT_PATH};
use photon_core::metric_schema::{self, MetricSchema};
use photon_core::query::MetricResolvedQuery;
use photon_core::PhotonError;
use photon_index::SkipIndex;
use photon_storage::Storage;

use crate::metric_predicate::metric_resolved_query_to_expr;
use crate::metric_query::ProbeMeta;
use crate::{cached_manifest, col_ref, session, ManifestCache};

#[derive(Clone)]
pub struct MetricsQueryEngine {
    pub(crate) hot_dir: PathBuf,
    /// The metrics schema. Handed to DataFusion as the explicit `read_parquet` schema (skipping
    /// per-query inference) and exposes the promoted-attribute list for grammar resolution.
    schema: MetricSchema,
    /// See `crate::ManifestCache`. `Arc`-wrapped (not just the `RwLock`) so a cheap
    /// `MetricsQueryEngine::clone` — taken to move into `spawn_blocking` — shares the same cache
    /// rather than forking it.
    manifest_cache: Arc<RwLock<Option<ManifestCache>>>,
    /// See `MetricMetaCache`. Read on every `metric_query::query_series` probe, written only on a
    /// cache miss; `Arc`-wrapped for the same clone-sharing reason as `manifest_cache`.
    metric_meta_cache: Arc<RwLock<Option<MetricMetaCache>>>,
    /// Test-only instrument: counts real `metric_meta_probe` calls, so a test can assert a second
    /// `query_series` call for the same metric didn't re-probe (cache hit) without reaching into
    /// cache internals.
    #[cfg(test)]
    pub(crate) probe_calls: Arc<std::sync::atomic::AtomicUsize>,
    /// Test-only seam: when set, `survivors_df` serves this in-memory batch (registered as the
    /// `metrics` table) instead of pruning + reading Parquet, so the SQL query path
    /// (`metric_query::query_series`) can be unit-tested without running a compaction.
    #[cfg(test)]
    test_batch: Option<RecordBatch>,
}

/// Cached per-metric probe metadata (type/temporality/monotonicity/unit), invalidated by manifest
/// `Arc` pointer-equality — the same rule `crate::ServicesCache` (`lib.rs`) uses for the
/// distinct-services cache. A metric's type/temporality/monotonicity/unit is stable, so once probed
/// for a manifest generation it never needs re-probing; there is no "confirmed absent" entry — a
/// metric not (yet) in `entries` (new/unseen, or a prior probe found no rows for it in that window)
/// is always probed fresh, never assumed absent.
struct MetricMetaCache {
    manifest: Arc<Manifest>,
    entries: HashMap<String, ProbeMeta>,
}

/// A pruned metrics read: one metric name, a time window, and an optional compiled label filter.
#[derive(Clone)]
pub(crate) struct MetricRequest {
    pub metric: String,
    pub start_ts_nanos: i64,
    pub end_ts_nanos: i64,
    pub filter: Option<MetricResolvedQuery>,
    /// When set, prune files whose skip-index host range provably excludes this host.
    pub host: Option<String>,
}

/// The row predicate every metrics read shares: `metric_name = ?` AND `timestamp` in the window AND
/// the compiled grammar filter. Applied after pruning so DataFusion's pushdown does the per-row
/// work during Parquet decode. Bound literals only (no SQL string interpolation → no injection
/// surface). Timestamp bounds use `lit_timestamp_nano` — matching the `Timestamp(ns)` column type,
/// exactly as logs' `base_predicate` does — rather than a bare `lit(i64)` that would compare Int64
/// to Timestamp and force a cast.
pub(crate) fn metric_base_predicate(req: &MetricRequest) -> Expr {
    let name_col = col_ref(metric_schema::METRIC_NAME);
    let ts = col_ref(metric_schema::TIMESTAMP);
    let mut pred = name_col
        .eq(lit(req.metric.clone()))
        .and(ts.clone().gt_eq(lit_timestamp_nano(req.start_ts_nanos)))
        .and(ts.lt_eq(lit_timestamp_nano(req.end_ts_nanos)));
    if let Some(filter) = &req.filter {
        pred = pred.and(metric_resolved_query_to_expr(filter));
    }
    pred
}

impl MetricsQueryEngine {
    /// Construct an engine rooted at a local hot directory. `hot_dir` is the same directory the
    /// metrics compactor writes to: it will contain `manifest/metrics-manifest.json` and a
    /// `data-metrics/` folder of `seg-*.parquet` + `seg-*.idx` sidecars once metrics have been
    /// compacted. Succeeds even when `hot_dir` (or `data-metrics/`) does not exist yet — nothing
    /// is read until a query method is called.
    pub fn new(hot_dir: PathBuf, schema: MetricSchema) -> Result<MetricsQueryEngine, PhotonError> {
        Ok(MetricsQueryEngine {
            hot_dir,
            schema,
            manifest_cache: Arc::new(RwLock::new(None)),
            metric_meta_cache: Arc::new(RwLock::new(None)),
            #[cfg(test)]
            probe_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            test_batch: None,
        })
    }

    /// Test-only constructor: an engine whose `survivors_df` serves a single in-memory `metrics`
    /// batch (no pruning, no Parquet). Used by `metric_query`'s SQL-path unit tests.
    #[cfg(test)]
    pub(crate) fn from_batch(schema: MetricSchema, batch: RecordBatch) -> MetricsQueryEngine {
        MetricsQueryEngine {
            hot_dir: PathBuf::from("/nonexistent-metric-query-test"),
            schema,
            manifest_cache: Arc::new(RwLock::new(None)),
            metric_meta_cache: Arc::new(RwLock::new(None)),
            probe_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            test_batch: Some(batch),
        }
    }

    /// The configured promoted-attribute names — used by callers to build a
    /// `photon_core::query::MetricFieldResolver` for the grammar.
    pub fn promoted_attributes(&self) -> &[String] {
        &self.schema.promoted
    }

    /// Raw SQL over the full (unpruned) `metrics` table: all `seg-*.parquet` files under
    /// `hot_dir/data-metrics/` are registered as a single table named `metrics`. No pruning —
    /// that is the pruned query methods' job.
    pub async fn sql(&self, sql: &str) -> Result<Vec<RecordBatch>, PhotonError> {
        let ctx = session();

        let mut dir = self
            .hot_dir
            .join("data-metrics")
            .to_string_lossy()
            .into_owned();
        if !dir.ends_with('/') {
            dir.push('/');
        }
        ctx.register_parquet("metrics", &dir, ParquetReadOptions::default())
            .await
            .map_err(|e| PhotonError::Query(format!("failed to register metrics table: {e}")))?;

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
    pub fn storage_stats(&self) -> Result<crate::StorageStats, PhotonError> {
        let manifest = self.load_metrics_manifest()?;
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
            if let Ok(md) = std::fs::metadata(self.hot_dir.join(&e.path)) {
                s.bytes += md.len();
            }
        }
        Ok(s)
    }

    /// Load the metrics manifest from `hot_dir/manifest/metrics-manifest.json`, or an empty one if
    /// the file is absent. Cached — see `crate::ManifestCache` / `crate::cached_manifest`.
    pub(crate) fn load_metrics_manifest(&self) -> Result<Arc<Manifest>, PhotonError> {
        let path = self.hot_dir.join(METRICS_MANIFEST_OBJECT_PATH);
        cached_manifest(&path, &self.manifest_cache)
    }

    /// Cached per-metric probe lookup, scoped to the manifest `Arc` passed in — mirrors
    /// `distinct_services`' use of `ServicesCache` in `lib.rs`. Returns `None` on any miss: either
    /// the cache is stale (the manifest changed under us — new segments could have introduced or
    /// changed the metric) or the metric simply hasn't been probed yet this generation. A `None`
    /// here is never a signal that the metric doesn't exist — callers must run `metric_meta_probe`
    /// and record a `Some` result with `cache_metric_meta`.
    pub(crate) fn cached_metric_meta(
        &self,
        manifest: &Arc<Manifest>,
        metric: &str,
    ) -> Option<ProbeMeta> {
        let guard = self.metric_meta_cache.read().unwrap();
        let cache = guard.as_ref()?;
        if !Arc::ptr_eq(&cache.manifest, manifest) {
            return None;
        }
        cache.entries.get(metric).cloned()
    }

    /// Record a freshly-probed metric's metadata for `manifest`'s generation. A cache built under
    /// an earlier manifest `Arc` is replaced wholesale rather than merged into — the same
    /// generation-replace behavior `distinct_services` uses when it writes a fresh `ServicesCache`.
    pub(crate) fn cache_metric_meta(
        &self,
        manifest: &Arc<Manifest>,
        metric: &str,
        meta: ProbeMeta,
    ) {
        let mut guard = self.metric_meta_cache.write().unwrap();
        match guard.as_mut() {
            Some(cache) if Arc::ptr_eq(&cache.manifest, manifest) => {
                cache.entries.insert(metric.to_string(), meta);
            }
            _ => {
                let mut entries = HashMap::new();
                entries.insert(metric.to_string(), meta);
                *guard = Some(MetricMetaCache {
                    manifest: manifest.clone(),
                    entries,
                });
            }
        }
    }

    /// Manifest time-overlap → per-file `metric_name` bloom → surviving absolute Parquet paths.
    /// Conservative: a missing `.idx` keeps the file, so pruning can only ever drop files that
    /// definitely cannot match — never a real result. Runs synchronous `std::fs` I/O (call under
    /// `spawn_blocking`, as `survivors_df` does).
    pub(crate) fn prune(&self, req: &MetricRequest) -> Result<Vec<String>, PhotonError> {
        let manifest = self.load_metrics_manifest()?;
        let mut surviving: Vec<String> = Vec::new();
        for entry in manifest.candidates(req.start_ts_nanos, req.end_ts_nanos) {
            if !self.keep_candidate(entry, &req.metric, req.host.as_deref())? {
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

    /// Decide whether a candidate file survives pruning. `manifest.candidates()` already filtered
    /// on time overlap; the extra pruning power is the `metric_name` bloom — a single-token check,
    /// like the spans `trace_id` bloom — plus, when a host is requested, the skip-index `host.name`
    /// range. A missing, unreadable, OR corrupt `.idx` keeps the file (correctness over pruning) —
    /// a torn sidecar never aborts the query or panics. Host pruning is conservative: an unknown
    /// host range keeps the file, so pruning can only ever drop files that provably cannot match —
    /// never a real result.
    fn keep_candidate(
        &self,
        entry: &FileEntry,
        metric: &str,
        host: Option<&str>,
    ) -> Result<bool, PhotonError> {
        let idx_path = self
            .hot_dir
            .join(Storage::index_path_metrics(entry.segment_id));
        let bytes = match std::fs::read(&idx_path) {
            Ok(b) => b,
            // No sidecar → cannot bloom-check → keep the file.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            // Any other read error → also keep, never abort the query (log once per bad file).
            Err(e) => {
                eprintln!(
                    "photon-query: warning: keeping {idx_path:?}, metrics skip index unreadable: {e}"
                );
                return Ok(true);
            }
        };
        let index = match SkipIndex::from_bytes(&bytes) {
            Ok(index) => index,
            // Corrupt/undecodable sidecar → keep the file (same rule as a missing one).
            Err(e) => {
                eprintln!(
                    "photon-query: warning: keeping {idx_path:?}, metrics skip index corrupt: {e}"
                );
                return Ok(true);
            }
        };
        if !index.might_contain_token(metric) {
            return Ok(false);
        }
        if let Some(h) = host {
            if let Some((lo, hi)) = index.host_range() {
                // Provably excluded only when the host is outside [lo, hi]; unknown range → keep.
                if h < lo.as_str() || h > hi.as_str() {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Prune (off-thread) then open the surviving Parquet files with an explicit schema. `None`
    /// when nothing survives — callers return an empty/zeroed result, never an error.
    ///
    /// Pruning (manifest `stat`/read + per-candidate `.idx` reads) is synchronous `std::fs` I/O; it
    /// runs in `spawn_blocking` so it never blocks a tokio worker thread. `self` is cheap to clone
    /// (a `PathBuf`, a small `MetricSchema`, and an `Arc`-shared cache), so the clone moved into the
    /// blocking closure still shares the manifest cache with `self`.
    pub(crate) async fn survivors_df(
        &self,
        req: &MetricRequest,
    ) -> Result<Option<DataFrame>, PhotonError> {
        #[cfg(test)]
        if let Some(batch) = &self.test_batch {
            let ctx = session();
            ctx.register_batch("metrics", batch.clone())
                .map_err(|e| PhotonError::Query(format!("register test metrics batch: {e}")))?;
            let df = ctx
                .table("metrics")
                .await
                .map_err(|e| PhotonError::Query(format!("read test metrics batch: {e}")))?;
            return Ok(Some(df));
        }
        let engine = self.clone();
        let req_c = req.clone();
        let surviving = spawn_blocking(move || engine.prune(&req_c))
            .await
            .map_err(|e| PhotonError::Query(format!("metrics prune task panicked: {e}")))??;
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
            .map_err(|e| PhotonError::Query(format!("failed to read metrics parquet: {e}")))?;
        Ok(Some(df))
    }

    /// Open every candidate file overlapping `[start, end]`, WITHOUT the metric_name bloom
    /// (catalog enumerates all metrics, so there is no single name to prune on). Time-pruned
    /// only.
    pub(crate) async fn time_survivors_df(
        &self,
        start_ts_nanos: i64,
        end_ts_nanos: i64,
    ) -> Result<Option<DataFrame>, PhotonError> {
        let engine = self.clone();
        let surviving = spawn_blocking(move || -> Result<Vec<String>, PhotonError> {
            let manifest = engine.load_metrics_manifest()?;
            Ok(manifest
                .candidates(start_ts_nanos, end_ts_nanos)
                .into_iter()
                .map(|e| engine.hot_dir.join(&e.path).to_string_lossy().into_owned())
                .collect())
        })
        .await
        .map_err(|e| PhotonError::Query(format!("metrics time-prune task panicked: {e}")))??;
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
            .map_err(|e| PhotonError::Query(format!("failed to read metrics parquet: {e}")))?;
        Ok(Some(df))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    use object_store::local::LocalFileSystem;
    use photon_compact::MetricsCompactor;
    use photon_core::metric_agg::Agg;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::segment::SegmentId;
    use photon_storage::{Replicator, Storage};
    use photon_wal::Wal;

    use crate::MetricSeriesRequest;

    /// Minimal in-memory WAL that hands the compactor pre-built segments, so the test controls
    /// segment ids (and therefore the on-disk `data-metrics/<stem>.{parquet,idx}` stems)
    /// deterministically. Mirrors the `FakeWal` in `metrics_compactor.rs`'s own tests.
    struct FakeWal {
        segments: Mutex<Vec<(SegmentId, Vec<RecordBatch>)>>,
    }
    #[allow(clippy::manual_async_fn)]
    impl Wal for FakeWal {
        fn append(
            &self,
            _b: RecordBatch,
        ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!() }
        }
        fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!() }
        }
        fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
            let mut ids: Vec<SegmentId> = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .map(|(id, _)| *id)
                .collect();
            ids.sort();
            Ok(ids)
        }
        fn read_segment(
            &self,
            id: SegmentId,
        ) -> impl std::future::Future<Output = Result<Vec<RecordBatch>, PhotonError>> + Send
        {
            let batches = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .find(|(sid, _)| *sid == id)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            async move { Ok(batches) }
        }
        fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError> {
            self.segments.lock().unwrap().retain(|(sid, _)| *sid != id);
            Ok(())
        }
    }

    fn point(name: &str, ts: i64) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), "checkout".to_string());
        MetricPoint {
            metric_name: name.to_string(),
            timestamp_nanos: ts,
            value: Some(1.0),
            attributes,
            ..Default::default()
        }
    }

    /// Like `point`, but also sets the promoted `host.name` attribute so a segment carries a
    /// single host — used to build per-host segments for host-range pruning tests.
    fn point_host(name: &str, host: &str, ts: i64) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), "checkout".to_string());
        attributes.insert("host.name".to_string(), host.to_string());
        MetricPoint {
            metric_name: name.to_string(),
            timestamp_nanos: ts,
            value: Some(1.0),
            attributes,
            ..Default::default()
        }
    }

    fn batch(schema: &MetricSchema, points: &[MetricPoint]) -> RecordBatch {
        let mut b = MetricBatchBuilder::new(schema);
        for p in points {
            b.append(p);
        }
        b.finish().unwrap()
    }

    /// Drive the real `MetricsCompactor` over the given `FakeWal` segments to produce genuine
    /// `data-metrics/<stem>.parquet` + `.idx` sidecars (real `metric_name` bloom) and a metrics
    /// manifest under `hot`.
    async fn compact(
        hot: &std::path::Path,
        schema: &MetricSchema,
        segments: Vec<(SegmentId, Vec<RecordBatch>)>,
    ) {
        let storage = Storage {
            hot: Arc::new(LocalFileSystem::new_with_prefix(hot).unwrap()),
            durable: None,
            hot_dir: Some(hot.to_path_buf()),
        };
        let wal = Arc::new(FakeWal {
            segments: Mutex::new(segments),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal, storage, replicator, schema.clone());
        while compactor.run_once().await.unwrap().is_some() {}
    }

    /// Compact two single-host segments (seg 0 → host `web-1`, seg 1 → host `web-2`), both with
    /// metric `m`, so the skip-index host ranges are disjoint per file — the setup a host-scoped
    /// prune must narrow to exactly one file.
    async fn two_host_fixture() -> (tempfile::TempDir, MetricsQueryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        compact(
            &hot,
            &schema,
            vec![
                (
                    SegmentId(0),
                    vec![batch(&schema, &[point_host("m", "web-1", 10)])],
                ),
                (
                    SegmentId(1),
                    vec![batch(&schema, &[point_host("m", "web-2", 20)])],
                ),
            ],
        )
        .await;
        let engine = MetricsQueryEngine::new(hot, schema).unwrap();
        (dir, engine)
    }

    #[tokio::test]
    async fn prune_keeps_only_files_whose_host_range_covers_the_request() {
        let (dir, engine) = two_host_fixture().await;
        let req = MetricRequest {
            metric: "m".to_string(),
            start_ts_nanos: 0,
            end_ts_nanos: i64::MAX,
            filter: None,
            host: Some("web-1".to_string()),
        };
        let surviving = engine.prune(&req).unwrap();
        assert_eq!(
            surviving.len(),
            1,
            "only the web-1 file should survive host pruning"
        );
        assert!(surviving[0].ends_with(&Storage::parquet_path_metrics(SegmentId(0))));
        drop(dir);
    }

    #[tokio::test]
    async fn prune_keeps_only_files_whose_bloom_has_the_metric() {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);

        // Segment 0 → file with metric "cpu.usage"; segment 1 → file with metric "mem.usage".
        compact(
            &hot,
            &schema,
            vec![
                (
                    SegmentId(0),
                    vec![batch(
                        &schema,
                        &[point("cpu.usage", 10), point("cpu.usage", 20)],
                    )],
                ),
                (
                    SegmentId(1),
                    vec![batch(&schema, &[point("mem.usage", 30)])],
                ),
            ],
        )
        .await;

        let engine = MetricsQueryEngine::new(hot.clone(), schema).unwrap();

        // Bloom keeps only the file whose skip index carries cpu.usage.
        let req = MetricRequest {
            metric: "cpu.usage".to_string(),
            start_ts_nanos: 0,
            end_ts_nanos: 100,
            filter: None,
            host: None,
        };
        let kept = engine.prune(&req).unwrap();
        assert_eq!(kept.len(), 1, "only segment 0's bloom contains cpu.usage");
        assert!(kept[0].ends_with(&Storage::parquet_path_metrics(SegmentId(0))));

        // The other metric keeps only the other file — disjoint from cpu.usage.
        let req_mem = MetricRequest {
            metric: "mem.usage".to_string(),
            start_ts_nanos: 0,
            end_ts_nanos: 100,
            filter: None,
            host: None,
        };
        let kept_mem = engine.prune(&req_mem).unwrap();
        assert_eq!(kept_mem.len(), 1);
        assert!(kept_mem[0].ends_with(&Storage::parquet_path_metrics(SegmentId(1))));

        // A metric in neither bloom prunes everything away.
        let req_none = MetricRequest {
            metric: "disk.io".to_string(),
            start_ts_nanos: 0,
            end_ts_nanos: 100,
            filter: None,
            host: None,
        };
        assert!(engine.prune(&req_none).unwrap().is_empty());

        // Time prune: window past both files → nothing (manifest candidates already excludes them).
        let req2 = MetricRequest {
            metric: "cpu.usage".to_string(),
            start_ts_nanos: 1000,
            end_ts_nanos: 2000,
            filter: None,
            host: None,
        };
        assert!(engine.prune(&req2).unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_idx_keeps_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        compact(
            &hot,
            &schema,
            vec![(
                SegmentId(0),
                vec![batch(&schema, &[point("cpu.usage", 10)])],
            )],
        )
        .await;

        // Remove the .idx sidecar so the bloom is unavailable → keep_candidate must conservatively
        // keep the file.
        std::fs::remove_file(hot.join(Storage::index_path_metrics(SegmentId(0)))).unwrap();

        let engine = MetricsQueryEngine::new(hot, schema).unwrap();
        let req = MetricRequest {
            metric: "anything".to_string(),
            start_ts_nanos: 0,
            end_ts_nanos: 100,
            filter: None,
            host: None,
        };
        assert_eq!(
            engine.prune(&req).unwrap().len(),
            1,
            "missing .idx must keep the file"
        );
    }

    /// Direct unit test on the cache primitives (audit item 9 / Task 7): a cached lookup must
    /// return exactly what a fresh probe would, and a manifest change (new segment compacted) must
    /// invalidate the cached entry — proven by pointer-inequality between the manifest `Arc` before
    /// and after, and a subsequent `cached_metric_meta` miss under the new `Arc`.
    #[tokio::test]
    async fn cached_metric_meta_matches_fresh_probe_and_invalidates_on_manifest_change() {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        compact(
            &hot,
            &schema,
            vec![(SegmentId(0), vec![batch(&schema, &[point("g", 0)])])],
        )
        .await;
        let engine = MetricsQueryEngine::new(hot.clone(), schema.clone()).unwrap();
        let req = MetricRequest {
            metric: "g".to_string(),
            start_ts_nanos: 0,
            end_ts_nanos: 200,
            filter: None,
            host: None,
        };

        // First call: cache miss, populates the cache as a side effect.
        let first = engine
            .metric_meta_probe_cached(&req)
            .await
            .unwrap()
            .unwrap();
        // Second call: must be a genuine cache hit (no unseen-metric fallback path involved).
        let cached = engine
            .metric_meta_probe_cached(&req)
            .await
            .unwrap()
            .unwrap();
        // Bypass the cache entirely for a ground-truth comparison.
        let fresh = engine.metric_meta_probe(&req).await.unwrap().unwrap();
        assert_eq!(
            first, fresh,
            "the first (uncached) probe must match a raw fresh probe"
        );
        assert_eq!(
            cached, fresh,
            "a cached probe must equal a fresh probe for the same metric+manifest generation"
        );

        let manifest_before = engine.load_metrics_manifest().unwrap();
        assert!(
            engine.cached_metric_meta(&manifest_before, "g").is_some(),
            "the probe above must have populated the cache for the current manifest generation"
        );

        // Compact a new segment: the manifest file changes on disk, so the next
        // `load_metrics_manifest` call must allocate a fresh `Arc<Manifest>`.
        compact(
            &hot,
            &schema,
            vec![(SegmentId(1), vec![batch(&schema, &[point("g", 150)])])],
        )
        .await;
        let manifest_after = engine.load_metrics_manifest().unwrap();
        assert!(
            !Arc::ptr_eq(&manifest_before, &manifest_after),
            "compacting a new segment must allocate a fresh manifest Arc"
        );
        assert!(
            engine.cached_metric_meta(&manifest_after, "g").is_none(),
            "a manifest-Arc change must invalidate the cached entry (never serve stale meta)"
        );
    }

    /// End-to-end proof that `query_series` (metric_query.rs) actually skips the redundant probe on
    /// a cache hit, and re-probes once the manifest changes — the behavior audit item 9 asks for.
    /// Uses the `probe_calls` test instrument (incremented inside `metric_meta_probe` itself) so the
    /// assertion doesn't depend on cache internals.
    #[tokio::test]
    async fn query_series_caches_metric_meta_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        compact(
            &hot,
            &schema,
            vec![(
                SegmentId(0),
                vec![batch(&schema, &[point("g", 0), point("g", 100)])],
            )],
        )
        .await;
        let engine = MetricsQueryEngine::new(hot.clone(), schema.clone()).unwrap();

        let req = || MetricSeriesRequest {
            metric: "g".to_string(),
            agg: Some(Agg::Avg),
            group_by: vec![],
            filter: None,
            start_ts_nanos: 0,
            end_ts_nanos: 200,
            buckets: 2,
        };

        let first = engine.query_series(req()).await.unwrap();
        assert_eq!(
            engine.probe_calls.load(Ordering::SeqCst),
            1,
            "the first query_series call must probe exactly once"
        );

        let second = engine.query_series(req()).await.unwrap();
        assert_eq!(
            engine.probe_calls.load(Ordering::SeqCst),
            1,
            "a second call for the same metric within the same manifest generation must hit the \
             cache, not re-probe"
        );
        assert_eq!(
            first.series, second.series,
            "caching the probe must never change the query result"
        );

        // Compacting a new segment rewrites the manifest, allocating a fresh `Arc<Manifest>` — the
        // cache's pointer-equality guard must treat that as a new generation and probe again.
        compact(
            &hot,
            &schema,
            vec![(SegmentId(1), vec![batch(&schema, &[point("g", 150)])])],
        )
        .await;
        engine.query_series(req()).await.unwrap();
        assert_eq!(
            engine.probe_calls.load(Ordering::SeqCst),
            2,
            "a manifest change must invalidate the cache and probe again"
        );
    }

    #[tokio::test]
    async fn new_over_missing_dir_succeeds() {
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        let engine = MetricsQueryEngine::new("/nonexistent/hot".into(), schema).unwrap();
        // sql over a missing data dir errors gracefully (no panic).
        let _ = engine.sql("SELECT 1").await;
    }
}

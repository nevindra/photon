//! Metrics query engine: raw SQL over `hot_dir/data-metrics/` (registered as the DataFusion table
//! `metrics`) plus the Phase-2 pruning path — manifest time-overlap → per-file `metric_name` bloom
//! → `read_parquet(surviving, schema)`. Mirrors `QueryEngine::prune`/`survivors_df` and the spans
//! `trace_id`-bloom path, differing only in the single-token (`metric_name`) bloom check.

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
    /// Test-only seam: when set, `survivors_df` serves this in-memory batch (registered as the
    /// `metrics` table) instead of pruning + reading Parquet, so the SQL query path
    /// (`metric_query::query_series`) can be unit-tested without running a compaction.
    #[cfg(test)]
    test_batch: Option<RecordBatch>,
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
    /// range. A missing `.idx` keeps the file (correctness over pruning); any other I/O error is
    /// surfaced. Host pruning is conservative: an unknown host range keeps the file, so pruning can
    /// only ever drop files that provably cannot match — never a real result.
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
            Err(e) => {
                return Err(PhotonError::Query(format!(
                    "failed to read metrics skip index {idx_path:?}: {e}"
                )))
            }
        };
        let index = SkipIndex::from_bytes(&bytes)?;
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
    use std::sync::Mutex;

    use object_store::local::LocalFileSystem;
    use photon_compact::MetricsCompactor;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::segment::SegmentId;
    use photon_storage::{Replicator, Storage};
    use photon_wal::Wal;

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

    #[tokio::test]
    async fn new_over_missing_dir_succeeds() {
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        let engine = MetricsQueryEngine::new("/nonexistent/hot".into(), schema).unwrap();
        // sql over a missing data dir errors gracefully (no panic).
        let _ = engine.sql("SELECT 1").await;
    }
}

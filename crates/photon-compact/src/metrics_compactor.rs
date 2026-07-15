//! [`MetricsCompactor`]: drains closed metrics-WAL segments into `(metric_name, service.name,
//! timestamp)`-sorted zstd Parquet under `data-metrics/` with a metrics skip-index sidecar,
//! recorded in the metrics manifest. Mirrors [`Compactor`](crate::Compactor) and
//! [`SpanCompactor`](crate::SpanCompactor); kept separate so the logs and spans paths are
//! untouched. Per-signal compactor duplication (this file mirrors `span_compactor.rs`) is an
//! accepted structural cost — see the tracing/metrics design docs.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Array, MapArray, StringArray};
use arrow::compute::{concat_batches, lexsort_to_indices, take_record_batch, SortColumn};
use arrow::record_batch::RecordBatch;
use bytes::Bytes;
use object_store::path::Path as ObjectPath;
use object_store::PutPayload;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use photon_core::manifest::{FileEntry, Manifest, METRICS_MANIFEST_OBJECT_PATH};
use photon_core::metric_schema::{self, MetricSchema, METRIC_NAME, TIMESTAMP};
use photon_core::retention::PurgeReport;
use photon_core::segment::SegmentId;
use photon_core::PhotonError;
use photon_index::SkipIndex;
use photon_storage::{Replicator, Storage};
use photon_wal::Wal;

use crate::stream::{fsync_manifest, hot_local_path, write_parquet_streamed, DEFAULT_ZSTD_LEVEL};

const SERVICE_NAME_COLUMN: &str = "service.name";
const HOST_NAME_COLUMN: &str = "host.name";
const MERGE_ROW_THRESHOLD: u64 = 10_000;

/// Cap on how many small files a single `merge_once` pass consolidates. Without a cap, a pass's
/// peak memory is bounded by NOTHING — after downtime, a merge-failure streak, or a burst of tiny
/// segments, one pass would hold ~2x the uncompressed union of every sub-threshold file. Any
/// remainder beyond the cap is carried forward into the new manifest untouched (not merged, not
/// deleted) and folds in over subsequent passes at the compactor's merge cadence.
const MERGE_MAX_FILES_PER_PASS: usize = 32;

pub struct MetricsCompactor<W: Wal> {
    wal: Arc<W>,
    storage: Storage,
    replicator: Arc<Replicator>,
    schema: MetricSchema,
    zstd_level: i32,
}

impl<W: Wal> MetricsCompactor<W> {
    pub fn new(
        wal: Arc<W>,
        storage: Storage,
        replicator: Arc<Replicator>,
        schema: MetricSchema,
    ) -> MetricsCompactor<W> {
        MetricsCompactor {
            wal,
            storage,
            replicator,
            schema,
            zstd_level: DEFAULT_ZSTD_LEVEL,
        }
    }

    /// Override the zstd compression level used when streaming Parquet files (default:
    /// [`DEFAULT_ZSTD_LEVEL`], byte-identical to the pre-config hardcoded default). Wired from
    /// `[storage] zstd_level` by `photon-server`.
    pub fn with_zstd_level(mut self, zstd_level: i32) -> Self {
        self.zstd_level = zstd_level;
        self
    }

    pub async fn run_once(&self) -> Result<Option<SegmentId>, PhotonError> {
        let closed = self.wal.list_closed_segments()?;
        let Some(seg) = closed.into_iter().next() else {
            return Ok(None);
        };

        let batches = self.wal.read_segment(seg).await?;
        // A closed segment recovered with 0 valid frames (e.g. a torn WAL tail on crash recovery)
        // has nothing to compact. Skip the Parquet write + manifest `add` entirely — writing a
        // 0-row Parquet + a `FileEntry{min_ts:0, max_ts:0}` would wedge `merge_once` forever
        // trying to fold it in — and just drop the drained WAL segment.
        if batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            self.wal.remove_segment(seg)?;
            return Ok(Some(seg));
        }
        let schema = self.schema.clone();
        let parquet_file = hot_local_path(&self.storage, &Storage::parquet_path_metrics(seg))?;
        let zstd_level = self.zstd_level;
        let out = tokio::task::spawn_blocking(move || {
            compact_segment(&schema, batches, parquet_file, zstd_level)
        })
        .await
        .map_err(|e| PhotonError::Arrow(format!("compaction task panicked: {e}")))??;

        let entry = out.entry(seg);
        self.put_object(&Storage::index_path_metrics(seg), out.index)
            .await?;

        let mut manifest = self.load_manifest().await?;
        manifest.add(entry);
        self.save_manifest(&manifest).await?;
        // Durability barrier: pin the manifest before removing the WAL segment (the only other copy).
        fsync_manifest(&self.storage, METRICS_MANIFEST_OBJECT_PATH).await?;

        self.replicator.enqueue(Storage::parquet_path_metrics(seg));
        self.replicator.enqueue(Storage::index_path_metrics(seg));

        self.wal.remove_segment(seg)?;
        Ok(Some(seg))
    }

    pub async fn merge_once(&self) -> Result<usize, PhotonError> {
        let manifest = self.load_manifest().await?;
        let all: Vec<FileEntry> = manifest
            .candidates(i64::MIN, i64::MAX)
            .into_iter()
            .cloned()
            .collect();
        let (mut small, large): (Vec<FileEntry>, Vec<FileEntry>) = all
            .into_iter()
            .partition(|e| e.row_count < MERGE_ROW_THRESHOLD);
        if small.len() < 2 {
            return Ok(0);
        }

        // Cap peak memory: sort oldest-first (also nudges toward time-adjacency) and merge only
        // the first MERGE_MAX_FILES_PER_PASS. The remainder is `carry` — NOT merged this pass, but
        // it MUST be added back into the new manifest (and its objects left alone) below, or those
        // files' rows are silently lost. The 10s merge cadence folds `carry` in over subsequent
        // passes.
        small.sort_by_key(|e| e.min_ts_nanos);
        let carry = small.split_off(small.len().min(MERGE_MAX_FILES_PER_PASS));
        let selected = small;

        // Merged id from the disjoint high-bit namespace (see the logs `Compactor` / the spans
        // compactor for the full rationale — prevents the merge-id-collision data-loss bug).
        let merged_seg = manifest
            .entries()
            .iter()
            .map(|e| e.segment_id)
            .filter(|s| s.is_merged())
            .max()
            .map(|s| s.next())
            .unwrap_or_else(SegmentId::first_merged);

        // Async-FETCH each selected file's raw bytes only (I/O). Decoding is pure CPU and happens
        // below, inside the SAME spawn_blocking that already runs compact_segment, so a merge pass
        // never decodes Parquet inline on a tokio async worker.
        let mut byte_bufs = Vec::with_capacity(selected.len());
        for e in &selected {
            byte_bufs.push(self.fetch_parquet_bytes(&e.path).await?);
        }
        let schema = self.schema.clone();
        let parquet_file =
            hot_local_path(&self.storage, &Storage::parquet_path_metrics(merged_seg))?;
        let zstd_level = self.zstd_level;
        let out = tokio::task::spawn_blocking(move || {
            let mut batches = Vec::new();
            for bytes in byte_bufs {
                batches.extend(decode_parquet(bytes)?);
            }
            compact_segment(&schema, batches, parquet_file, zstd_level)
        })
        .await
        .map_err(|e| PhotonError::Arrow(format!("compaction task panicked: {e}")))??;

        let entry = out.entry(merged_seg);
        self.put_object(&Storage::index_path_metrics(merged_seg), out.index)
            .await?;

        // Single commit point: keep every `large` entry, carry every un-merged `small` entry
        // forward UNTOUCHED (CRITICAL — dropping these would silently lose their rows), add the
        // fresh merged entry, save once.
        let mut new_manifest = Manifest::new();
        for e in large {
            new_manifest.add(e);
        }
        for e in carry {
            new_manifest.add(e);
        }
        new_manifest.add(entry);
        self.save_manifest(&new_manifest).await?;
        fsync_manifest(&self.storage, METRICS_MANIFEST_OBJECT_PATH).await?;

        self.replicator
            .enqueue(Storage::parquet_path_metrics(merged_seg));
        self.replicator
            .enqueue(Storage::index_path_metrics(merged_seg));

        // Delete ONLY the selected input objects — carried-forward small entries stay on disk;
        // they are still referenced by the manifest just saved above. Mirror each hot delete with a
        // durable delete enqueued on the replicator, keyed on the EXACT per-entry
        // parquet_path/index_path(segment_id) (never a prefix) — otherwise the durable replica keeps
        // every superseded merge input forever. Enqueue BEFORE the hot delete so the durable delete
        // is registered even if a hot delete errors mid-loop (the entry is already dropped from the
        // committed manifest above, so this is its only chance to enqueue it). Both are async, off
        // the ack/query path; a durable NotFound is a no-op success.
        for e in &selected {
            let parquet = Storage::parquet_path_metrics(e.segment_id);
            let index = Storage::index_path_metrics(e.segment_id);
            self.replicator.enqueue_delete(parquet.clone());
            self.replicator.enqueue_delete(index.clone());
            self.delete_object(&parquet).await?;
            self.delete_object(&index).await?;
        }

        Ok(selected.len())
    }

    /// Fetch the raw Parquet bytes for an object in the hot store. Async I/O only — decoding is
    /// pure CPU and happens later, off the async runtime, inside [`decode_parquet`].
    async fn fetch_parquet_bytes(&self, path: &str) -> Result<Bytes, PhotonError> {
        self.storage
            .hot
            .get(&ObjectPath::from(path))
            .await
            .map_err(|e| PhotonError::Storage(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| PhotonError::Storage(e.to_string()))
    }

    async fn load_manifest(&self) -> Result<Manifest, PhotonError> {
        let path = ObjectPath::from(METRICS_MANIFEST_OBJECT_PATH);
        match self.storage.hot.get(&path).await {
            Ok(result) => {
                let bytes = result
                    .bytes()
                    .await
                    .map_err(|e| PhotonError::Storage(e.to_string()))?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|e| PhotonError::Serde(format!("metrics manifest not UTF-8: {e}")))?;
                Manifest::from_json(text)
            }
            Err(object_store::Error::NotFound { .. }) => Ok(Manifest::new()),
            Err(e) => Err(PhotonError::Storage(e.to_string())),
        }
    }

    async fn save_manifest(&self, manifest: &Manifest) -> Result<(), PhotonError> {
        let json = manifest.to_json()?;
        self.put_object(METRICS_MANIFEST_OBJECT_PATH, json.into_bytes())
            .await
    }

    async fn put_object(&self, path: &str, bytes: Vec<u8>) -> Result<(), PhotonError> {
        self.storage
            .hot
            .put(&ObjectPath::from(path), PutPayload::from(bytes))
            .await
            .map_err(|e| PhotonError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn delete_object(&self, path: &str) -> Result<(), PhotonError> {
        self.storage
            .hot
            .delete(&ObjectPath::from(path))
            .await
            .map_err(|e| PhotonError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Delete every manifest entry entirely older than `cutoff_nanos` (`max_ts_nanos < cutoff`),
    /// remove its Parquet + `.idx` from the hot store, and save the trimmed manifest. A straddling
    /// file is kept (conservative — never drops newer data). `cutoff_nanos == i64::MAX` deletes all.
    /// The compactor remains the sole manifest writer.
    pub async fn purge_before(&self, cutoff_nanos: i64) -> Result<PurgeReport, PhotonError> {
        let manifest = self.load_manifest().await?;
        let all: Vec<FileEntry> = manifest
            .candidates(i64::MIN, i64::MAX)
            .into_iter()
            .cloned()
            .collect();
        let (drop, keep): (Vec<FileEntry>, Vec<FileEntry>) =
            all.into_iter().partition(|e| e.max_ts_nanos < cutoff_nanos);
        if drop.is_empty() {
            return Ok(PurgeReport::default());
        }
        let rows_removed: u64 = drop.iter().map(|e| e.row_count).sum();

        let mut new_manifest = Manifest::new();
        for e in keep {
            new_manifest.add(e);
        }
        self.save_manifest(&new_manifest).await?;
        // Durability barrier: pin the trimmed manifest (already reflecting the drop) before
        // deleting the superseded Parquet + idx below — mirrors run_once/merge_once (doc-04
        // Finding 4). Without this, a crash in the writeback window leaves the pre-purge manifest
        // referencing already-unlinked objects: queries overlapping the ghost entry fail at
        // DataFusion `read_parquet`, and merge_once (which reads every small entry) errors every
        // tick — a wedged merge loop.
        fsync_manifest(&self.storage, METRICS_MANIFEST_OBJECT_PATH).await?;

        // Delete each expired file from hot AND enqueue a durable delete for the SAME exact
        // parquet_path/index_path(segment_id) (never a prefix) so durable-tier retention is enforced.
        // Enqueue BEFORE the hot delete so it is registered even if a hot delete errors (the entry is
        // already dropped from the committed manifest above). item 10c: keying the durable delete on
        // the exact per-segment path — not a prefix — is what makes it safe for a merged Parquet PATH
        // to be reused later, since any stale durable object at that path is removed here at purge
        // time. Async, off the ack/query path; a durable NotFound is success.
        for e in &drop {
            let parquet = Storage::parquet_path_metrics(e.segment_id);
            let index = Storage::index_path_metrics(e.segment_id);
            self.replicator.enqueue_delete(parquet.clone());
            self.replicator.enqueue_delete(index.clone());
            self.delete_object_if_present(&parquet).await?;
            self.delete_object_if_present(&index).await?;
        }
        Ok(PurgeReport {
            files_removed: drop.len() as u64,
            rows_removed,
        })
    }

    /// Like `delete_object` but treats an already-absent object as success.
    async fn delete_object_if_present(&self, path: &str) -> Result<(), PhotonError> {
        match self.storage.hot.delete(&ObjectPath::from(path)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(PhotonError::Storage(e.to_string())),
        }
    }
}

/// Everything the metrics compaction pipeline produces off the async runtime that the caller still
/// needs: the small skip-index body to `put` plus the manifest metadata. The Parquet file itself is
/// streamed straight to disk inside `compact_segment`.
struct CompactedOut {
    index: Vec<u8>,
    min_ts: i64,
    max_ts: i64,
    min_service: String,
    max_service: String,
    row_count: u64,
    attribute_keys: Vec<String>,
    /// On-disk size of the streamed Parquet file, `stat`ed right after the write on the blocking
    /// thread. Recorded into `FileEntry.bytes` so `storage_stats` is manifest arithmetic.
    bytes: u64,
}

impl CompactedOut {
    fn entry(&self, seg: SegmentId) -> FileEntry {
        FileEntry {
            path: Storage::parquet_path_metrics(seg),
            segment_id: seg,
            min_ts_nanos: self.min_ts,
            max_ts_nanos: self.max_ts,
            min_service: self.min_service.clone(),
            max_service: self.max_service.clone(),
            row_count: self.row_count,
            durable: false,
            attribute_keys: self.attribute_keys.clone(),
            bytes: self.bytes,
        }
    }
}

/// concat -> sort by `(metric_name, service.name, timestamp)` -> STREAM the zstd Parquet encode
/// straight to `parquet_file` -> build the metrics skip index. Runs on a `spawn_blocking` thread.
fn compact_segment(
    schema: &MetricSchema,
    batches: Vec<RecordBatch>,
    parquet_file: PathBuf,
    zstd_level: i32,
) -> Result<CompactedOut, PhotonError> {
    let concatenated = concat(schema, &batches)?;
    drop(batches);
    let sorted = sort_metrics(&concatenated)?;
    drop(concatenated);

    write_parquet_streamed(&parquet_file, &sorted, zstd_level)?;
    // Capture the exact on-disk Parquet size now, on this blocking thread, straight after the
    // write — this is what `storage_stats` used to `stat()` per entry every tick. A metadata error
    // degrades to `0`, which makes `storage_stats` fall back to a `stat()` for this entry.
    let bytes = std::fs::metadata(&parquet_file)
        .map(|m| m.len())
        .unwrap_or(0);

    let index = SkipIndex::build_metrics(&sorted)?;
    let (min_ts, max_ts) = index.timestamp_range().unwrap_or((0, 0));
    let (min_service, max_service) = index.service_range().unwrap_or_default();
    let attribute_keys = attribute_keys(&sorted);

    Ok(CompactedOut {
        index: index.to_bytes(),
        min_ts,
        max_ts,
        min_service,
        max_service,
        row_count: sorted.num_rows() as u64,
        attribute_keys,
        bytes,
    })
}

/// Decode a Parquet byte buffer into its constituent [`RecordBatch`]es. Pure CPU work — every
/// caller runs this inside a `spawn_blocking` closure (never inline on the async runtime), so a
/// merge pass's decode of potentially many small files never blocks a tokio worker.
fn decode_parquet(bytes: Bytes) -> Result<Vec<RecordBatch>, PhotonError> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes)
        .map_err(|e| PhotonError::Arrow(e.to_string()))?
        .build()
        .map_err(|e| PhotonError::Arrow(e.to_string()))?;

    let mut batches = Vec::new();
    for batch in reader {
        batches.push(batch.map_err(|e| PhotonError::Arrow(e.to_string()))?);
    }
    Ok(batches)
}

fn concat(schema: &MetricSchema, batches: &[RecordBatch]) -> Result<RecordBatch, PhotonError> {
    if batches.is_empty() {
        return Ok(RecordBatch::new_empty(schema.arrow.clone()));
    }
    concat_batches(&batches[0].schema(), batches).map_err(|e| PhotonError::Arrow(e.to_string()))
}

/// Sort by `(metric_name, service.name, host.name, timestamp)`. `service.name` and `host.name`
/// are the promoted sort keys for metrics (config validation guarantees `service.name` is
/// promoted; `host.name` is always promoted too — `photon-server`'s wiring layer injects it into
/// the metrics schema's promoted-attributes list regardless of operator config, so it is always
/// present as a column). A missing column errors loudly rather than silently skipping the sort.
fn sort_metrics(batch: &RecordBatch) -> Result<RecordBatch, PhotonError> {
    let by = |name: &str| -> Result<SortColumn, PhotonError> {
        Ok(SortColumn {
            values: batch
                .column_by_name(name)
                .ok_or_else(|| PhotonError::Arrow(format!("batch is missing the {name} column")))?
                .clone(),
            options: None,
        })
    };
    let sort_columns = vec![
        by(METRIC_NAME)?,
        by(SERVICE_NAME_COLUMN)?,
        by(HOST_NAME_COLUMN)?,
        by(TIMESTAMP)?,
    ];
    let indices =
        lexsort_to_indices(&sort_columns, None).map_err(|e| PhotonError::Arrow(e.to_string()))?;
    take_record_batch(batch, &indices).map_err(|e| PhotonError::Arrow(e.to_string()))
}

fn attribute_keys(batch: &RecordBatch) -> Vec<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    if let Some(map) = batch
        .column_by_name(metric_schema::ATTRIBUTES)
        .and_then(|c| c.as_any().downcast_ref::<MapArray>())
    {
        if let Some(k) = map
            .entries()
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
        {
            for i in 0..k.len() {
                if !k.is_null(i) {
                    keys.insert(k.value(i).to_string());
                }
            }
        }
    }
    keys.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use arrow::array::TimestampNanosecondArray;
    use object_store::local::LocalFileSystem;
    use object_store::ObjectStore;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};

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

    impl FakeWal {
        /// Append a closed segment after construction — simulates a NEW WAL segment rotating
        /// closed (and reusing a freed id) after a merge has already run.
        fn add_segment(&self, id: SegmentId, batches: Vec<RecordBatch>) {
            self.segments.lock().unwrap().push((id, batches));
        }
    }

    fn test_storage(dir: &std::path::Path) -> Storage {
        Storage {
            hot: Arc::new(LocalFileSystem::new_with_prefix(dir).unwrap()),
            durable: None,
            hot_dir: Some(dir.to_path_buf()),
        }
    }

    fn point(name: &str, svc: &str, ts: i64) -> MetricPoint {
        let mut attrs = std::collections::BTreeMap::new();
        attrs.insert("service.name".to_string(), svc.to_string());
        MetricPoint {
            metric_name: name.into(),
            timestamp_nanos: ts,
            value: Some(1.0),
            attributes: attrs,
            ..Default::default()
        }
    }

    /// Like `point`, but also sets the promoted `host.name` attribute so the batch exercises the
    /// `(metric_name, service.name, host.name, timestamp)` sort key.
    fn point_host(name: &str, svc: &str, host: &str, ts: i64) -> MetricPoint {
        let mut attrs = std::collections::BTreeMap::new();
        attrs.insert("service.name".to_string(), svc.to_string());
        attrs.insert("host.name".to_string(), host.to_string());
        MetricPoint {
            metric_name: name.into(),
            timestamp_nanos: ts,
            value: Some(1.0),
            attributes: attrs,
            ..Default::default()
        }
    }

    async fn read_back(storage: &Storage, path: &str) -> RecordBatch {
        let data = storage
            .hot
            .get(&ObjectPath::from(path))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(data)
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<RecordBatch> = reader.map(|b| b.unwrap()).collect();
        concat_batches(&batches[0].schema(), &batches).unwrap()
    }

    #[tokio::test]
    async fn run_once_writes_sorted_metrics_parquet_index_and_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = MetricSchema::new(&[
            SERVICE_NAME_COLUMN.to_string(),
            HOST_NAME_COLUMN.to_string(),
        ]);

        let mut b = MetricBatchBuilder::new(&schema);
        b.append(&point("cpu.usage", "web", 300));
        b.append(&point("cpu.usage", "api", 100));
        b.append(&point("http.rps", "api", 200));
        let batch = b.finish().unwrap();

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![(SegmentId(0), vec![batch])]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal, storage.clone(), replicator, schema);

        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));

        let parquet_path = Storage::parquet_path_metrics(SegmentId(0));
        assert!(storage
            .hot
            .get(&ObjectPath::from(parquet_path.clone()))
            .await
            .is_ok());
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::index_path_metrics(SegmentId(0))))
            .await
            .is_ok());

        // Rows sorted by (metric_name, service.name, timestamp):
        // (cpu.usage, api, 100) < (cpu.usage, web, 300) < (http.rps, api, 200)
        let sorted = read_back(&storage, &parquet_path).await;
        let names = sorted
            .column_by_name(METRIC_NAME)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(
            (names.value(0), names.value(1), names.value(2)),
            ("cpu.usage", "cpu.usage", "http.rps")
        );
        let ts = sorted
            .column_by_name(TIMESTAMP)
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        assert_eq!((ts.value(0), ts.value(1), ts.value(2)), (100, 300, 200));

        // Metrics manifest has the entry; logs/spans manifests untouched (absent).
        let metrics_manifest = storage
            .hot
            .get(&ObjectPath::from(METRICS_MANIFEST_OBJECT_PATH))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let m = Manifest::from_json(std::str::from_utf8(&metrics_manifest).unwrap()).unwrap();
        assert_eq!(m.candidates(i64::MIN, i64::MAX).len(), 1);
        assert!(storage
            .hot
            .get(&ObjectPath::from(
                photon_core::manifest::MANIFEST_OBJECT_PATH
            ))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(
                photon_core::manifest::SPANS_MANIFEST_OBJECT_PATH
            ))
            .await
            .is_err());

        // Segment removed from the WAL after a successful compaction.
        assert!(compactor.wal.list_closed_segments().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_once_records_parquet_bytes_in_metrics_manifest_entry() {
        // `FileEntry.bytes` is captured at write time with the exact on-disk Parquet size, so the
        // usage sampler's `storage_stats` becomes manifest arithmetic instead of a stat() per entry.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = MetricSchema::new(&[
            SERVICE_NAME_COLUMN.to_string(),
            HOST_NAME_COLUMN.to_string(),
        ]);

        let mut b = MetricBatchBuilder::new(&schema);
        b.append(&point("cpu.usage", "api", 100));
        b.append(&point("cpu.usage", "web", 300));
        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![(SegmentId(0), vec![b.finish().unwrap()])]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal, storage.clone(), replicator, schema);
        compactor.run_once().await.unwrap();

        let entry = load_manifest(&storage).await.candidates(i64::MIN, i64::MAX)[0].clone();
        let on_disk =
            std::fs::metadata(tmp.path().join(Storage::parquet_path_metrics(SegmentId(0))))
                .unwrap()
                .len();
        assert!(entry.bytes > 0, "bytes must be captured at write time");
        assert_eq!(
            entry.bytes, on_disk,
            "recorded bytes must equal the on-disk Parquet size"
        );
    }

    #[tokio::test]
    async fn run_once_drops_empty_metrics_segment_without_writing_parquet_or_manifest_entry() {
        // A closed segment recovered with 0 valid frames (e.g. via the torn-tail rotation added
        // in commit 9ecf107) must not produce a 0-row Parquet + a bogus `FileEntry{min_ts:0,
        // max_ts:0}` manifest entry — that would wedge merge_once forever trying to fold it in.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = MetricSchema::new(&[
            SERVICE_NAME_COLUMN.to_string(),
            HOST_NAME_COLUMN.to_string(),
        ]);

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![(SegmentId(0), vec![])]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal.clone(), storage.clone(), replicator, schema);

        let processed = compactor.run_once().await.unwrap();
        assert_eq!(processed, Some(SegmentId(0)));

        // No Parquet, no idx, no manifest object were ever created.
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path_metrics(SegmentId(
                0
            ))))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::index_path_metrics(SegmentId(0))))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(METRICS_MANIFEST_OBJECT_PATH))
            .await
            .is_err());

        // The drained (empty) WAL segment was still removed.
        assert!(wal.list_closed_segments().unwrap().is_empty());
    }

    /// Load the metrics manifest from the hot store (test-only helper; mirrors the private
    /// `load_manifest` method but reads from outside the compactor for assertions).
    async fn load_manifest(storage: &Storage) -> Manifest {
        let data = storage
            .hot
            .get(&ObjectPath::from(METRICS_MANIFEST_OBJECT_PATH))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        Manifest::from_json(std::str::from_utf8(&data).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn merge_writes_a_fresh_metric_segment_id_not_an_input_id() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = MetricSchema::new(&[
            SERVICE_NAME_COLUMN.to_string(),
            HOST_NAME_COLUMN.to_string(),
        ]);

        let mut b0 = MetricBatchBuilder::new(&schema);
        b0.append(&point("cpu.usage", "web", 300));
        b0.append(&point("cpu.usage", "api", 100));
        let mut b1 = MetricBatchBuilder::new(&schema);
        b1.append(&point("cpu.usage", "api", 50));
        b1.append(&point("cpu.usage", "web", 200));

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![
                (SegmentId(0), vec![b0.finish().unwrap()]),
                (SegmentId(1), vec![b1.finish().unwrap()]),
            ]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal, storage.clone(), replicator, schema);

        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(1)));
        assert_eq!(compactor.merge_once().await.unwrap(), 2);

        let manifest = load_manifest(&storage).await;
        let entries = manifest.candidates(i64::MIN, i64::MAX);
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].segment_id.is_merged(),
            "merged id {:?} must be from the high-bit namespace, not a reused input id",
            entries[0].segment_id
        );

        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path_metrics(SegmentId(
                0
            ))))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path_metrics(SegmentId(
                1
            ))))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn merged_metrics_survive_a_later_wal_segment_reusing_the_old_id() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = MetricSchema::new(&[
            SERVICE_NAME_COLUMN.to_string(),
            HOST_NAME_COLUMN.to_string(),
        ]);

        let mut b0 = MetricBatchBuilder::new(&schema);
        b0.append(&point("cpu.usage", "web", 300));
        b0.append(&point("cpu.usage", "api", 100));
        let mut b1 = MetricBatchBuilder::new(&schema);
        b1.append(&point("cpu.usage", "api", 50));
        b1.append(&point("cpu.usage", "web", 200));

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![
                (SegmentId(0), vec![b0.finish().unwrap()]),
                (SegmentId(1), vec![b1.finish().unwrap()]),
            ]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor =
            MetricsCompactor::new(wal.clone(), storage.clone(), replicator, schema.clone());

        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(1)));
        assert_eq!(compactor.merge_once().await.unwrap(), 2);

        let merged_seg = {
            let m = load_manifest(&storage).await;
            let e = m.candidates(i64::MIN, i64::MAX);
            assert_eq!(e.len(), 1);
            let seg = e[0].segment_id;
            assert!(
                seg.is_merged(),
                "merged id {seg:?} must be from the high-bit namespace"
            );
            assert_ne!(seg, SegmentId(1));
            seg
        };

        let mut b2 = MetricBatchBuilder::new(&schema);
        b2.append(&point("zzz", "zzz", 9000));
        b2.append(&point("zzz", "zzz", 9100));
        wal.add_segment(SegmentId(1), vec![b2.finish().unwrap()]);
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(1)));

        let manifest = load_manifest(&storage).await;
        let entries = manifest.candidates(i64::MIN, i64::MAX);
        assert_eq!(
            entries.len(),
            2,
            "merged entry AND the reused-id entry must both be present"
        );

        let merged_entry = entries
            .iter()
            .find(|e| e.segment_id == merged_seg)
            .expect("merged entry must still be present");
        assert_eq!(merged_entry.row_count, 4);
        assert_eq!(
            read_back(&storage, &Storage::parquet_path_metrics(merged_seg))
                .await
                .num_rows(),
            4
        );

        let reused_entry = entries
            .iter()
            .find(|e| e.segment_id == SegmentId(1))
            .expect("the reused-id (segment 1) entry must be present");
        assert_eq!(reused_entry.row_count, 2);
    }

    #[tokio::test]
    async fn merge_once_caps_files_per_pass_and_carries_forward_the_rest() {
        // Regression for the P2 memory fix: a merge pass with MORE than
        // MERGE_MAX_FILES_PER_PASS small files must merge exactly the cap and CARRY the rest
        // forward into the new manifest untouched (no rows lost, objects not deleted) — a naive
        // cap that just drops the un-merged entries would silently lose their data.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = MetricSchema::new(&[
            SERVICE_NAME_COLUMN.to_string(),
            HOST_NAME_COLUMN.to_string(),
        ]);

        // MERGE_MAX_FILES_PER_PASS + 3 small (1-row) files.
        let total_files = MERGE_MAX_FILES_PER_PASS + 3;
        let segments: Vec<(SegmentId, Vec<RecordBatch>)> = (0..total_files)
            .map(|i| {
                let mut b = MetricBatchBuilder::new(&schema);
                b.append(&point("cpu.usage", "svc", i as i64));
                (SegmentId(i as u64), vec![b.finish().unwrap()])
            })
            .collect();
        let wal = Arc::new(FakeWal {
            segments: Mutex::new(segments),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal.clone(), storage.clone(), replicator, schema);

        // Drain every WAL segment into its own small Parquet file.
        while compactor.run_once().await.unwrap().is_some() {}
        assert_eq!(
            load_manifest(&storage)
                .await
                .candidates(i64::MIN, i64::MAX)
                .len(),
            total_files
        );

        // First pass: merges exactly the cap, carries the remainder forward.
        let merged = compactor.merge_once().await.unwrap();
        assert_eq!(merged, MERGE_MAX_FILES_PER_PASS);

        let manifest = load_manifest(&storage).await;
        let entries = manifest.candidates(i64::MIN, i64::MAX);
        // 3 carried small entries (original ids, untouched) + 1 fresh merged entry.
        assert_eq!(entries.len(), 3 + 1);

        // No rows lost: total across every surviving entry still equals the original file count.
        let total_rows: u64 = entries.iter().map(|e| e.row_count).sum();
        assert_eq!(total_rows, total_files as u64);

        let carried: Vec<&FileEntry> = entries
            .iter()
            .copied()
            .filter(|e| !e.segment_id.is_merged())
            .collect();
        assert_eq!(
            carried.len(),
            3,
            "exactly the un-merged 3 small entries carry forward"
        );
        for e in &carried {
            // The carried entry's Parquet object must still be on disk (not deleted).
            assert!(
                storage
                    .hot
                    .get(&ObjectPath::from(e.path.clone()))
                    .await
                    .is_ok(),
                "carried entry {:?}'s Parquet object must still be present",
                e.segment_id
            );
        }

        let merged_entry = entries
            .iter()
            .find(|e| e.segment_id.is_merged())
            .expect("a fresh merged entry must be present");
        assert_eq!(merged_entry.row_count, MERGE_MAX_FILES_PER_PASS as u64);

        // Second pass: the 3 carried entries + the (still small) merged entry are all < the row
        // threshold, so this pass folds all 4 of them into one file.
        let merged2 = compactor.merge_once().await.unwrap();
        assert_eq!(merged2, 4);

        let manifest2 = load_manifest(&storage).await;
        let entries2 = manifest2.candidates(i64::MIN, i64::MAX);
        assert_eq!(
            entries2.len(),
            1,
            "second pass fully consolidates the remainder"
        );
        assert_eq!(
            entries2[0].row_count, total_files as u64,
            "still no rows lost"
        );
    }

    #[tokio::test]
    async fn purge_before_drops_old_metric_files() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = MetricSchema::new(&[
            SERVICE_NAME_COLUMN.to_string(),
            HOST_NAME_COLUMN.to_string(),
        ]);

        // Two segments: seg 0 entirely old (ts 100,200), seg 1 entirely new (ts 5000,6000).
        let mut old_builder = MetricBatchBuilder::new(&schema);
        old_builder.append(&point("cpu.usage", "api", 100));
        old_builder.append(&point("cpu.usage", "api", 200));
        let old = old_builder.finish().unwrap();

        let mut new_builder = MetricBatchBuilder::new(&schema);
        new_builder.append(&point("cpu.usage", "web", 5000));
        new_builder.append(&point("cpu.usage", "web", 6000));
        let new = new_builder.finish().unwrap();

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![(SegmentId(0), vec![old]), (SegmentId(1), vec![new])]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal, storage.clone(), replicator, schema);

        // Compact both segments into two files.
        while compactor.run_once().await.unwrap().is_some() {}
        assert_eq!(
            load_manifest(&storage)
                .await
                .candidates(i64::MIN, i64::MAX)
                .len(),
            2
        );

        // Cutoff between the two files: seg 0 (max 200 < 1000) drops, seg 1 (max 6000) stays.
        let report = compactor.purge_before(1000).await.unwrap();
        assert_eq!(report.files_removed, 1);
        assert_eq!(report.rows_removed, 2);

        let manifest = load_manifest(&storage).await;
        let entries = manifest.candidates(i64::MIN, i64::MAX);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].segment_id, SegmentId(1));

        // The dropped file's Parquet object is gone.
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path_metrics(SegmentId(
                0
            ))))
            .await
            .is_err());

        // Delete-all empties the manifest.
        let report2 = compactor.purge_before(i64::MAX).await.unwrap();
        assert_eq!(report2.files_removed, 1);
        assert!(load_manifest(&storage)
            .await
            .candidates(i64::MIN, i64::MAX)
            .is_empty());

        // Purging an empty manifest is a no-op.
        let report3 = compactor.purge_before(i64::MAX).await.unwrap();
        assert_eq!(report3, PurgeReport::default());
    }

    #[test]
    fn sort_metrics_orders_by_metric_service_host_then_time() {
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        // Two hosts, same metric+service, interleaved rows; host "a" must sort before "b".
        let mut b = MetricBatchBuilder::new(&schema);
        b.append(&point_host("system.cpu.utilization", "svc", "b", 100));
        b.append(&point_host("system.cpu.utilization", "svc", "a", 300));
        b.append(&point_host("system.cpu.utilization", "svc", "a", 200));
        let batch = b.finish().unwrap();

        let sorted = sort_metrics(&batch).unwrap();
        let h = sorted
            .column_by_name("host.name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let t = sorted
            .column_by_name(TIMESTAMP)
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        // host "a" rows first (sorted by ts within host), then host "b".
        assert_eq!((h.value(0), t.value(0)), ("a", 200));
        assert_eq!((h.value(1), t.value(1)), ("a", 300));
        assert_eq!((h.value(2), t.value(2)), ("b", 100));
    }
}

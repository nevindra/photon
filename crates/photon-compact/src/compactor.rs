//! The [`Compactor`]: drains closed WAL segments into sorted, zstd-compressed Parquet
//! files with a sidecar skip index, records each file in the manifest, and consolidates
//! small files into larger ones.
//!
//! # Data flow (`run_once`)
//! 1. Take the lowest closed WAL segment (`Wal::list_closed_segments`, ascending).
//! 2. Read + concat its recovered batches.
//! 3. Sort by `(service.name, timestamp)` with the Arrow lexsort kernels — this is the
//!    physical order the query engine's pruning relies on.
//! 4. Stream ONE zstd Parquet file straight to disk at the hot store's backing path for
//!    `Storage::parquet_path(seg)` (temp file + fsync + atomic rename — no whole-file `Vec<u8>`).
//! 5. Build a [`SkipIndex`] over the sorted batch and write it to `Storage::index_path(seg)`.
//! 6. Append a [`FileEntry`] (`durable = false`) to the manifest and save it back.
//! 7. Enqueue both objects for hot -> durable replication.
//! 8. Remove the drained WAL segment.
//!
//! The manifest is a single JSON object in the hot store (`MANIFEST_OBJECT_PATH`); the
//! compactor is its sole writer, so there are no write-write races.

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

use photon_core::manifest::{FileEntry, Manifest, MANIFEST_OBJECT_PATH};
use photon_core::retention::PurgeReport;
use photon_core::schema::{self, LogSchema};
use photon_core::segment::SegmentId;
use photon_core::PhotonError;
use photon_index::SkipIndex;
use photon_storage::{Replicator, Storage};
use photon_wal::Wal;

use crate::stream::{fsync_manifest, hot_local_path, write_parquet_streamed, DEFAULT_ZSTD_LEVEL};

/// Promoted attribute that is the primary sort key. Must match `SkipIndex`'s notion of the
/// service column and the schema promoted by `Config::validate`.
const SERVICE_NAME_COLUMN: &str = "service.name";

/// A Parquet file whose `row_count` is below this is a "small" file eligible for merging.
const MERGE_ROW_THRESHOLD: u64 = 10_000;

/// Cap on how many small files a single `merge_once` pass consolidates. Without a cap, a pass's
/// peak memory is bounded by NOTHING — after downtime, a merge-failure streak, or a burst of tiny
/// segments, one pass would hold ~2x the uncompressed union of every sub-threshold file. Any
/// remainder beyond the cap is carried forward into the new manifest untouched (not merged, not
/// deleted) and folds in over subsequent passes at the compactor's merge cadence.
const MERGE_MAX_FILES_PER_PASS: usize = 32;

/// Drains closed WAL segments into the hot object store and maintains the manifest.
///
/// Generic over `W: Wal` so it can be exercised against an in-memory fake in tests; in
/// production `W` is `photon_wal::DiskWal`.
pub struct Compactor<W: Wal> {
    wal: Arc<W>,
    storage: Storage,
    replicator: Arc<Replicator>,
    schema: LogSchema,
    zstd_level: i32,
}

impl<W: Wal> Compactor<W> {
    pub fn new(
        wal: Arc<W>,
        storage: Storage,
        replicator: Arc<Replicator>,
        schema: LogSchema,
    ) -> Compactor<W> {
        Compactor {
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

    /// Drain ONE closed WAL segment fully into a sorted Parquet file + skip index, record it
    /// in the manifest, enqueue replication, and remove the segment. Returns the processed
    /// [`SegmentId`], or `None` when no closed segment is available.
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
        // Resolve the Parquet's real on-disk path under the local hot root so the blocking task
        // can stream the encode straight to a `File` (no whole-file `Vec<u8>`, doc-04 F2). The
        // object path maps 1:1 onto `<hot_dir>/<parquet_path>`, so the same hot store still serves
        // it via `get` and the replicator reads it unchanged.
        let parquet_file = hot_local_path(&self.storage, &Storage::parquet_path(seg))?;
        let zstd_level = self.zstd_level;
        // Concat + sort + stream-to-disk + skip-index build all run on a blocking thread so they
        // never hold an async worker (doc-04 F3). `batches` is moved in and dropped there.
        let out = tokio::task::spawn_blocking(move || {
            compact_segment(&schema, batches, parquet_file, zstd_level)
        })
        .await
        .map_err(|e| PhotonError::Arrow(format!("compaction task panicked: {e}")))??;

        // The Parquet file already exists in the hot store's backing dir; only the small `.idx`
        // sidecar still goes through `put_object`. Assemble the entry before the puts.
        let entry = out.entry(seg);
        self.put_object(&Storage::index_path(seg), out.index)
            .await?;

        let mut manifest = self.load_manifest().await?;
        manifest.add(entry);
        self.save_manifest(&manifest).await?;
        // Durability barrier: make the manifest (now referencing the new Parquet + idx, both already
        // fsync'd to disk) durable BEFORE the point of no return — `remove_segment` drops the WAL
        // segment, the only other copy of these rows (doc-04 Finding 4).
        fsync_manifest(&self.storage, MANIFEST_OBJECT_PATH).await?;

        self.replicator.enqueue(Storage::parquet_path(seg));
        self.replicator.enqueue(Storage::index_path(seg));

        self.wal.remove_segment(seg)?;

        Ok(Some(seg))
    }

    /// Consolidate small Parquet files (below [`MERGE_ROW_THRESHOLD`] rows) into one larger
    /// sorted file: rewrite the manifest to drop the merged entries and add the consolidated
    /// one, then delete the superseded files from the hot store. Returns how many source
    /// files were merged (0 when fewer than two small files exist).
    pub async fn merge_once(&self) -> Result<usize, PhotonError> {
        let manifest = self.load_manifest().await?;
        // `candidates(MIN, MAX)` returns every entry (all time ranges overlap the full span).
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

        // Allocate the consolidated file's id from the MERGED (high-bit) namespace, disjoint from
        // every WAL-allocated id. WAL ids are small and sequential; merged ids carry the top bit, so
        // the merge output's Parquet path AND manifest key can never equal a live WAL segment's —
        // even the one the WAL has open right now. Without this, when the compactor is caught up,
        // `max(all ids).next()` equals the open WAL segment's id, and compacting that segment later
        // clobbers the merged Parquet + idempotent-replaces the merged manifest entry, silently
        // losing every consolidated row (the B2 merge-id-collision fix; closes doc-04 Finding 5 too:
        // strictly greater than every existing merged id, so re-merging small merged files is safe).
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
        // Same offload as `run_once`: the merge's decode/concat/sort/stream-encode is pure CPU
        // (doc-04 F3). The consolidated file streams to `merged_seg`'s fresh path (temp+fsync+
        // rename+parent-dir fsync inside `write_parquet_streamed`), never touching an input object.
        let parquet_file = hot_local_path(&self.storage, &Storage::parquet_path(merged_seg))?;
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
        self.put_object(&Storage::index_path(merged_seg), out.index)
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
        // Durability barrier: make the manifest (now pointing at the fresh merged file) durable
        // BEFORE the point of no return — deleting the superseded input objects below.
        fsync_manifest(&self.storage, MANIFEST_OBJECT_PATH).await?;

        self.replicator.enqueue(Storage::parquet_path(merged_seg));
        self.replicator.enqueue(Storage::index_path(merged_seg));

        // Delete ONLY the selected input objects — carried-forward small entries stay on disk;
        // they are still referenced by the manifest just saved above. Mirror each hot delete with a
        // durable delete enqueued on the replicator, keyed on the EXACT per-entry
        // parquet_path/index_path(segment_id) (never a prefix) — otherwise the durable replica keeps
        // every superseded merge input forever. Enqueue BEFORE the hot delete so the durable delete
        // is registered even if a hot delete errors mid-loop (the entry is already dropped from the
        // committed manifest above, so this is its only chance to enqueue it). Both are async, off
        // the ack/query path; a durable NotFound is a no-op success.
        for e in &selected {
            let parquet = Storage::parquet_path(e.segment_id);
            let index = Storage::index_path(e.segment_id);
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

    /// Load the manifest from the hot store, or a fresh empty one if it does not exist yet.
    async fn load_manifest(&self) -> Result<Manifest, PhotonError> {
        let path = ObjectPath::from(MANIFEST_OBJECT_PATH);
        match self.storage.hot.get(&path).await {
            Ok(result) => {
                let bytes = result
                    .bytes()
                    .await
                    .map_err(|e| PhotonError::Storage(e.to_string()))?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|e| PhotonError::Serde(format!("manifest is not valid UTF-8: {e}")))?;
                Manifest::from_json(text)
            }
            Err(object_store::Error::NotFound { .. }) => Ok(Manifest::new()),
            Err(e) => Err(PhotonError::Storage(e.to_string())),
        }
    }

    /// Persist the manifest to the hot store (the compactor is its sole writer).
    async fn save_manifest(&self, manifest: &Manifest) -> Result<(), PhotonError> {
        let json = manifest.to_json()?;
        self.put_object(MANIFEST_OBJECT_PATH, json.into_bytes())
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
        fsync_manifest(&self.storage, MANIFEST_OBJECT_PATH).await?;

        // Delete each expired file from hot AND enqueue a durable delete for the SAME exact
        // parquet_path/index_path(segment_id) (never a prefix) so durable-tier retention is enforced.
        // Enqueue BEFORE the hot delete so it is registered even if a hot delete errors (the entry is
        // already dropped from the committed manifest above). item 10c: keying the durable delete on
        // the exact per-segment path — not a prefix — is what makes it safe for a merged Parquet PATH
        // to be reused later, since any stale durable object at that path is removed here at purge
        // time. Async, off the ack/query path; a durable NotFound is success.
        for e in &drop {
            let parquet = Storage::parquet_path(e.segment_id);
            let index = Storage::index_path(e.segment_id);
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

/// Everything the compaction pipeline produces off the async runtime that the async caller still
/// needs: the small skip-index body to `put` plus the manifest metadata. The Parquet file itself
/// is streamed straight to disk inside [`compact_segment`] (not returned here). Computed on a
/// `spawn_blocking` thread; the async caller only does the `.idx` put and the manifest write.
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
    /// Assemble the manifest [`FileEntry`] for a given segment id (`durable = false`; min/max
    /// ts + service and row count come from the sorted batch via the skip index).
    fn entry(&self, seg: SegmentId) -> FileEntry {
        FileEntry {
            path: Storage::parquet_path(seg),
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

/// The compaction pipeline: concat -> sort by `(service.name, timestamp)` -> STREAM the zstd
/// Parquet encode straight to `parquet_file` on disk (no whole-file `Vec<u8>`, doc-04 F2) -> build
/// the [`SkipIndex`] and compute min/max + attribute keys. This runs on a `spawn_blocking` thread
/// (doc-04 F3) so the `concat`/`lexsort`/`take`/zstd + file I/O never holds a tokio async worker.
/// The only side effect is the streamed Parquet file; the caller `put`s the returned `.idx`.
fn compact_segment(
    schema: &LogSchema,
    batches: Vec<RecordBatch>,
    parquet_file: PathBuf,
    zstd_level: i32,
) -> Result<CompactedOut, PhotonError> {
    let concatenated = concat(schema, &batches)?;
    drop(batches); // free the raw WAL batches immediately (doc-04 F2)
    let sorted = sort_by_service_and_timestamp(&concatenated)?;
    drop(concatenated); // free the pre-sort copy before encoding (doc-04 F2)

    write_parquet_streamed(&parquet_file, &sorted, zstd_level)?;
    // Capture the exact on-disk Parquet size now, on this blocking thread, straight after the
    // write — this is what `storage_stats` used to `stat()` per entry every tick. A metadata error
    // degrades to `0`, which makes `storage_stats` fall back to a `stat()` for this entry.
    let bytes = std::fs::metadata(&parquet_file)
        .map(|m| m.len())
        .unwrap_or(0);

    let index = SkipIndex::build(&sorted, schema)?;
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

/// Concatenate batches into one. Uses the first batch's schema (identical to `schema.arrow`
/// for WAL input and preserved across a Parquet round-trip for merge input); an empty input
/// yields an empty batch of the configured schema.
fn concat(schema: &LogSchema, batches: &[RecordBatch]) -> Result<RecordBatch, PhotonError> {
    if batches.is_empty() {
        return Ok(RecordBatch::new_empty(schema.arrow.clone()));
    }
    concat_batches(&batches[0].schema(), batches).map_err(|e| PhotonError::Arrow(e.to_string()))
}

/// Stable lexicographic sort by `(service.name, timestamp)` via the Arrow sort kernels.
fn sort_by_service_and_timestamp(batch: &RecordBatch) -> Result<RecordBatch, PhotonError> {
    let service = batch.column_by_name(SERVICE_NAME_COLUMN).ok_or_else(|| {
        PhotonError::Arrow(format!("batch is missing the {SERVICE_NAME_COLUMN} column"))
    })?;
    let timestamp = batch.column_by_name(schema::TIMESTAMP).ok_or_else(|| {
        PhotonError::Arrow(format!("batch is missing the {} column", schema::TIMESTAMP))
    })?;

    let sort_columns = vec![
        SortColumn {
            values: service.clone(),
            options: None,
        },
        SortColumn {
            values: timestamp.clone(),
            options: None,
        },
    ];
    let indices =
        lexsort_to_indices(&sort_columns, None).map_err(|e| PhotonError::Arrow(e.to_string()))?;
    take_record_batch(batch, &indices).map_err(|e| PhotonError::Arrow(e.to_string()))
}

/// Sorted, deduped union of the long-tail attribute keys in a batch's `attributes` Map column.
/// Promoted attributes are real columns (static, known from config) and are excluded.
fn attribute_keys(batch: &RecordBatch) -> Vec<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    if let Some(map) = batch
        .column_by_name(photon_core::schema::ATTRIBUTES)
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

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use arrow::array::{Array, StringArray, TimestampNanosecondArray};
    use object_store::local::LocalFileSystem;
    use object_store::ObjectStore;
    use photon_core::record::{LogRecord, RecordBatchBuilder};

    /// In-memory [`Wal`] fake: serves pre-seeded closed segments and records removals. Only
    /// the read/list/remove surface the compactor uses is meaningful; append/sync are unused.
    struct FakeWal {
        segments: Mutex<Vec<(SegmentId, Vec<RecordBatch>)>>,
    }

    impl FakeWal {
        fn with_segments(segments: Vec<(SegmentId, Vec<RecordBatch>)>) -> FakeWal {
            FakeWal {
                segments: Mutex::new(segments),
            }
        }

        /// Append a closed segment after construction — used to simulate a NEW WAL segment
        /// rotating closed after a merge has already run.
        fn add_segment(&self, id: SegmentId, batches: Vec<RecordBatch>) {
            self.segments.lock().unwrap().push((id, batches));
        }
    }

    // `impl Future + Send` mirrors the trait's signature exactly; `manual_async_fn` would
    // suggest dropping the `Send` bound, which the trait requires.
    #[allow(clippy::manual_async_fn)]
    impl Wal for FakeWal {
        fn append(
            &self,
            _batch: RecordBatch,
        ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!("FakeWal::append is not exercised by the compactor") }
        }
        fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!("FakeWal::sync is not exercised by the compactor") }
        }
        fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
            let mut ids: Vec<SegmentId> = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .map(|(id, _)| *id)
                .collect();
            ids.sort(); // contract: ascending
            Ok(ids)
        }
        fn read_segment(
            &self,
            id: SegmentId,
        ) -> impl std::future::Future<Output = Result<Vec<RecordBatch>, PhotonError>> + Send
        {
            let guard = self.segments.lock().unwrap();
            let batches = guard
                .iter()
                .find(|(sid, _)| *sid == id)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            drop(guard);
            async move { Ok(batches) }
        }
        fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError> {
            self.segments.lock().unwrap().retain(|(sid, _)| *sid != id);
            Ok(())
        }
    }

    fn test_schema() -> LogSchema {
        LogSchema::new(&[SERVICE_NAME_COLUMN.to_string()])
    }

    fn test_storage(dir: &std::path::Path) -> Storage {
        let hot = LocalFileSystem::new_with_prefix(dir).unwrap();
        Storage {
            hot: Arc::new(hot),
            durable: None,
            hot_dir: Some(dir.to_path_buf()),
        }
    }

    /// Build a batch from `(service, timestamp, body)` rows.
    fn make_batch(schema: &LogSchema, rows: &[(&str, i64, &str)]) -> RecordBatch {
        let mut builder = RecordBatchBuilder::new(schema);
        for (service, ts, body) in rows {
            let mut attributes = BTreeMap::new();
            attributes.insert(SERVICE_NAME_COLUMN.to_string(), service.to_string());
            builder.append(&LogRecord {
                timestamp_nanos: *ts,
                body: Some(body.to_string()),
                attributes,
                ..Default::default()
            });
        }
        builder.finish().unwrap()
    }

    /// Read a Parquet object from the hot store back into one concatenated batch.
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

    /// The `(service, timestamp)` rows of a batch, in physical order.
    fn rows_of(batch: &RecordBatch) -> Vec<(String, i64)> {
        let service = batch
            .column_by_name(SERVICE_NAME_COLUMN)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let timestamp = batch
            .column_by_name(schema::TIMESTAMP)
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        (0..batch.num_rows())
            .map(|i| (service.value(i).to_string(), timestamp.value(i)))
            .collect()
    }

    async fn load_manifest(storage: &Storage) -> Manifest {
        let data = storage
            .hot
            .get(&ObjectPath::from(MANIFEST_OBJECT_PATH))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        Manifest::from_json(std::str::from_utf8(&data).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn run_once_writes_sorted_parquet_index_and_manifest_then_removes_segment() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();

        // Deliberately unsorted by (service, timestamp).
        let batch = make_batch(
            &schema,
            &[
                ("web", 300, "error alpha"),
                ("api", 100, "warn beta"),
                ("api", 50, "info gamma"),
                ("web", 200, "debug delta"),
            ],
        );
        let wal = Arc::new(FakeWal::with_segments(vec![(SegmentId(0), vec![batch])]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal.clone(), storage.clone(), replicator, schema);

        let processed = compactor.run_once().await.unwrap();
        assert_eq!(processed, Some(SegmentId(0)));

        let parquet_path = Storage::parquet_path(SegmentId(0));
        let index_path = Storage::index_path(SegmentId(0));

        // hot store ends with the parquet + idx + manifest.
        assert!(storage
            .hot
            .get(&ObjectPath::from(parquet_path.clone()))
            .await
            .is_ok());
        assert!(storage.hot.get(&ObjectPath::from(index_path)).await.is_ok());

        // Parquet rows are sorted by (service.name, timestamp).
        let sorted = read_back(&storage, &parquet_path).await;
        assert_eq!(
            rows_of(&sorted),
            vec![
                ("api".to_string(), 50),
                ("api".to_string(), 100),
                ("web".to_string(), 200),
                ("web".to_string(), 300),
            ]
        );

        // Manifest entry reflects the file's min/max ts + service and row count.
        let manifest = load_manifest(&storage).await;
        let entries = manifest.candidates(i64::MIN, i64::MAX);
        assert_eq!(entries.len(), 1);
        let entry = entries[0];
        assert_eq!(entry.path, parquet_path);
        assert_eq!(entry.segment_id, SegmentId(0));
        assert_eq!(entry.min_ts_nanos, 50);
        assert_eq!(entry.max_ts_nanos, 300);
        assert_eq!(entry.min_service, "api");
        assert_eq!(entry.max_service, "web");
        assert_eq!(entry.row_count, 4);
        assert!(!entry.durable);

        // The WAL segment was removed after compaction.
        assert!(wal.list_closed_segments().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_once_records_parquet_bytes_in_manifest_entry() {
        // `FileEntry.bytes` must be populated at write time with the exact on-disk Parquet size,
        // so the usage sampler's `storage_stats` becomes manifest arithmetic instead of a stat()
        // per entry. Assert the recorded value is non-zero and matches the file on disk exactly.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();
        let batch = make_batch(&schema, &[("api", 50, "a"), ("web", 300, "b")]);
        let wal = Arc::new(FakeWal::with_segments(vec![(SegmentId(0), vec![batch])]));
        let compactor = Compactor::new(
            wal,
            storage.clone(),
            Arc::new(Replicator::new(storage.clone())),
            schema,
        );
        compactor.run_once().await.unwrap();

        let entry = {
            let m = load_manifest(&storage).await;
            m.candidates(i64::MIN, i64::MAX)[0].clone()
        };
        let on_disk = std::fs::metadata(tmp.path().join(Storage::parquet_path(SegmentId(0))))
            .unwrap()
            .len();
        assert!(entry.bytes > 0, "bytes must be captured at write time");
        assert_eq!(
            entry.bytes, on_disk,
            "recorded bytes must equal the on-disk Parquet size"
        );
    }

    #[tokio::test]
    async fn run_once_still_sorts_after_offloading_cpu() {
        // Identical assertion set to the existing run_once test, guarding the refactor that
        // moves the concat/sort/encode CPU work onto a `spawn_blocking` thread.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();
        let batch = make_batch(
            &schema,
            &[("web", 300, "a"), ("api", 100, "b"), ("api", 50, "c")],
        );
        let wal = Arc::new(FakeWal::with_segments(vec![(SegmentId(0), vec![batch])]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal, storage.clone(), replicator, schema);
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));
        let sorted = read_back(&storage, &Storage::parquet_path(SegmentId(0))).await;
        assert_eq!(
            rows_of(&sorted),
            vec![("api".into(), 50), ("api".into(), 100), ("web".into(), 300)]
        );
    }

    #[tokio::test]
    async fn streamed_parquet_reads_back_identical() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();
        let batch = make_batch(&schema, &[("api", 50, "a"), ("web", 300, "b")]);
        let wal = Arc::new(FakeWal::with_segments(vec![(SegmentId(0), vec![batch])]));
        let compactor = Compactor::new(
            wal,
            storage.clone(),
            Arc::new(Replicator::new(storage.clone())),
            schema,
        );
        compactor.run_once().await.unwrap();
        let back = read_back(&storage, &Storage::parquet_path(SegmentId(0))).await;
        assert_eq!(
            rows_of(&back),
            vec![("api".into(), 50), ("web".into(), 300)]
        );
    }

    #[tokio::test]
    async fn run_once_drops_empty_segment_without_writing_parquet_or_manifest_entry() {
        // A closed segment recovered with 0 valid frames (e.g. via the torn-tail rotation added
        // in commit 9ecf107) must not produce a 0-row Parquet + a bogus `FileEntry{min_ts:0,
        // max_ts:0}` manifest entry — that would wedge merge_once forever trying to fold it in.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();

        let wal = Arc::new(FakeWal::with_segments(vec![(SegmentId(0), vec![])]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal.clone(), storage.clone(), replicator, schema);

        let processed = compactor.run_once().await.unwrap();
        assert_eq!(processed, Some(SegmentId(0)));

        // No Parquet, no idx, no manifest object were ever created.
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path(SegmentId(0))))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::index_path(SegmentId(0))))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(MANIFEST_OBJECT_PATH))
            .await
            .is_err());

        // The drained (empty) WAL segment was still removed.
        assert!(wal.list_closed_segments().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_once_returns_none_when_no_closed_segment() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();

        let wal = Arc::new(FakeWal::with_segments(vec![]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal, storage.clone(), replicator, schema);

        assert_eq!(compactor.run_once().await.unwrap(), None);
    }

    #[tokio::test]
    async fn merge_once_consolidates_two_small_files_and_updates_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();

        // Distinct long-tail attribute keys per segment, to prove `write_file` unions them on
        // merge (step 6): seg0 carries `region`, seg1 carries `tier`.
        let mut b0 = RecordBatchBuilder::new(&schema);
        for (svc, ts, body) in [("web", 300i64, "a"), ("api", 100, "b")] {
            let mut attrs = BTreeMap::new();
            attrs.insert(SERVICE_NAME_COLUMN.to_string(), svc.to_string());
            attrs.insert("region".to_string(), "us".to_string());
            b0.append(&LogRecord {
                timestamp_nanos: ts,
                body: Some(body.to_string()),
                attributes: attrs,
                ..Default::default()
            });
        }
        let seg0 = b0.finish().unwrap();

        let mut b1 = RecordBatchBuilder::new(&schema);
        for (svc, ts, body) in [("api", 50i64, "c"), ("web", 200, "d")] {
            let mut attrs = BTreeMap::new();
            attrs.insert(SERVICE_NAME_COLUMN.to_string(), svc.to_string());
            attrs.insert("tier".to_string(), "gold".to_string());
            b1.append(&LogRecord {
                timestamp_nanos: ts,
                body: Some(body.to_string()),
                attributes: attrs,
                ..Default::default()
            });
        }
        let seg1 = b1.finish().unwrap();
        let wal = Arc::new(FakeWal::with_segments(vec![
            (SegmentId(0), vec![seg0]),
            (SegmentId(1), vec![seg1]),
        ]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal, storage.clone(), replicator, schema);

        // Produce two small Parquet files (one per segment).
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(1)));
        assert_eq!(
            load_manifest(&storage)
                .await
                .candidates(i64::MIN, i64::MAX)
                .len(),
            2
        );

        let merged = compactor.merge_once().await.unwrap();
        assert_eq!(merged, 2);

        // Manifest now holds a single consolidated entry covering both files.
        let manifest = load_manifest(&storage).await;
        let entries = manifest.candidates(i64::MIN, i64::MAX);
        assert_eq!(entries.len(), 1);
        let entry = entries[0];
        // Consolidated id comes from the merged (high-bit) namespace — the first merged id, since
        // no merged segment existed yet. It is NOT a reused input id and never a live WAL id.
        assert_eq!(entry.segment_id, SegmentId::first_merged());
        assert!(entry.segment_id.is_merged());
        assert_eq!(entry.row_count, 4);
        assert_eq!(entry.min_ts_nanos, 50);
        assert_eq!(entry.max_ts_nanos, 300);
        assert_eq!(entry.min_service, "api");
        assert_eq!(entry.max_service, "web");
        // The merged file's attribute_keys is the union of both inputs' long-tail keys.
        assert_eq!(
            entry.attribute_keys,
            vec!["region".to_string(), "tier".to_string()]
        );

        // The superseded (lower) segment's objects are gone; the merged file survives, sorted.
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path(SegmentId(0))))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::index_path(SegmentId(0))))
            .await
            .is_err());

        let merged_batch =
            read_back(&storage, &Storage::parquet_path(SegmentId::first_merged())).await;
        assert_eq!(
            rows_of(&merged_batch),
            vec![
                ("api".to_string(), 50),
                ("api".to_string(), 100),
                ("web".to_string(), 200),
                ("web".to_string(), 300),
            ]
        );
    }

    #[tokio::test]
    async fn merge_writes_a_fresh_segment_id_not_an_input_id() {
        // Regression for doc-04 Finding 5: merge must write the consolidated file under a FRESH
        // segment id, never overwrite an input id in place (that overwrites live data before the
        // manifest commit → a crash leaves duplicate rows). Two small files (segments 0 and 1);
        // after the merge the consolidated entry's id must be strictly greater than every input
        // id, and BOTH input Parquet objects must be gone.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();

        let seg0 = make_batch(&schema, &[("web", 300, "a"), ("api", 100, "b")]);
        let seg1 = make_batch(&schema, &[("api", 50, "c"), ("web", 200, "d")]);
        let wal = Arc::new(FakeWal::with_segments(vec![
            (SegmentId(0), vec![seg0]),
            (SegmentId(1), vec![seg1]),
        ]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal, storage.clone(), replicator, schema);

        // Two small Parquet files (segments 0 and 1).
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(1)));

        let merged = compactor.merge_once().await.unwrap();
        assert_eq!(merged, 2);

        let manifest = load_manifest(&storage).await;
        let entries = manifest.candidates(i64::MIN, i64::MAX);
        assert_eq!(entries.len(), 1);
        let entry = entries[0];

        // The merged entry uses a FRESH id, strictly greater than every input id.
        assert!(
            entry.segment_id > SegmentId(1),
            "merged id {:?} must be fresh (> every input id), not a reused input id",
            entry.segment_id
        );

        // BOTH input Parquet objects are absent — none was reused/overwritten in place.
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path(SegmentId(0))))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path(SegmentId(1))))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn merge_output_survives_a_later_wal_segment_reusing_the_old_next_id() {
        // Regression for the B2 merge-id-collision data-loss bug. Under the OLD allocation the
        // merged output took `max(all manifest ids).next()`. When the compactor is caught up that
        // equals the id the WAL will hand its NEXT closed segment — so when that segment later
        // compacts via `run_once`, it writes `parquet_path(that id)` (clobbering the merged Parquet)
        // and `manifest.add(entry{that id})` (idempotently REPLACING the merged manifest entry),
        // silently losing every row the merge consolidated. The high-bit merged namespace makes the
        // merge output's path + manifest key disjoint from every WAL id, so both files coexist.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();

        // Two small WAL-compacted segments (ids 0 and 1) with distinct rows.
        let seg0 = make_batch(&schema, &[("web", 300, "a"), ("api", 100, "b")]);
        let seg1 = make_batch(&schema, &[("api", 50, "c"), ("web", 200, "d")]);
        let wal = Arc::new(FakeWal::with_segments(vec![
            (SegmentId(0), vec![seg0]),
            (SegmentId(1), vec![seg1]),
        ]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal.clone(), storage.clone(), replicator, schema.clone());

        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(1)));

        // Merge the two small files. The consolidated id must be from the merged (high-bit)
        // namespace — never a small input id, and never the id the WAL will reuse next.
        assert_eq!(compactor.merge_once().await.unwrap(), 2);
        let merged_seg = {
            let manifest = load_manifest(&storage).await;
            let entries = manifest.candidates(i64::MIN, i64::MAX);
            assert_eq!(entries.len(), 1);
            let seg = entries[0].segment_id;
            assert!(
                seg.is_merged(),
                "merged id {seg:?} must be from the high-bit namespace"
            );
            assert_ne!(seg, SegmentId(0));
            assert_ne!(seg, SegmentId(1));
            // Crucially: NOT the id the old buggy code would have picked (`max(0,1).next()`).
            assert_ne!(seg, SegmentId(2));
            seg
        };

        // Now a NEW WAL segment closes with EXACTLY the id the old allocator would have handed the
        // merged file (`SegmentId(2)`), carrying its own distinct rows, and gets compacted.
        wal.add_segment(
            SegmentId(2),
            vec![make_batch(
                &schema,
                &[("zzz", 9000, "z"), ("zzz", 9100, "y")],
            )],
        );
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(2)));

        // BOTH survive — no clobber, no lost rows. On the OLD code the manifest would hold a single
        // entry (segment-2's, having replaced the merged one) and the merged rows would be gone.
        let manifest = load_manifest(&storage).await;
        let entries = manifest.candidates(i64::MIN, i64::MAX);
        assert_eq!(
            entries.len(),
            2,
            "merged entry AND segment-2 entry must both be present"
        );

        // The merged entry is intact and unchanged: still 4 rows, still its own ts span.
        let merged_entry = entries
            .iter()
            .find(|e| e.segment_id == merged_seg)
            .expect("merged entry must still be present");
        assert_eq!(merged_entry.row_count, 4);
        assert_eq!(merged_entry.min_ts_nanos, 50);
        assert_eq!(merged_entry.max_ts_nanos, 300);
        // The merged Parquet still holds the consolidated rows (not overwritten by segment 2).
        let merged_batch = read_back(&storage, &Storage::parquet_path(merged_seg)).await;
        assert_eq!(
            rows_of(&merged_batch),
            vec![
                ("api".to_string(), 50),
                ("api".to_string(), 100),
                ("web".to_string(), 200),
                ("web".to_string(), 300),
            ]
        );

        // The new segment-2 entry is also present with ITS distinct rows.
        let seg2_entry = entries
            .iter()
            .find(|e| e.segment_id == SegmentId(2))
            .expect("segment-2 entry must be present");
        assert_eq!(seg2_entry.row_count, 2);
        let seg2_batch = read_back(&storage, &Storage::parquet_path(SegmentId(2))).await;
        assert_eq!(
            rows_of(&seg2_batch),
            vec![("zzz".to_string(), 9000), ("zzz".to_string(), 9100)]
        );
    }

    #[tokio::test]
    async fn merge_once_caps_files_per_pass_and_carries_forward_the_rest() {
        // Regression for the P2 memory fix: a merge pass with MORE than
        // MERGE_MAX_FILES_PER_PASS small files must merge exactly the cap and CARRY the rest
        // forward into the new manifest untouched (no rows lost, objects not deleted) — a naive
        // cap that just drops the un-merged entries would silently lose their data.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();

        // MERGE_MAX_FILES_PER_PASS + 3 small (1-row) files.
        let total_files = MERGE_MAX_FILES_PER_PASS + 3;
        let segments: Vec<(SegmentId, Vec<RecordBatch>)> = (0..total_files)
            .map(|i| {
                let batch = make_batch(&schema, &[("svc", i as i64, "row")]);
                (SegmentId(i as u64), vec![batch])
            })
            .collect();
        let wal = Arc::new(FakeWal::with_segments(segments));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal.clone(), storage.clone(), replicator, schema);

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

    #[test]
    fn attribute_keys_are_sorted_unique_map_keys() {
        let schema = test_schema(); // promotes only service.name
                                    // make_batch only sets service.name; extend inline with long-tail attrs:
        let mut builder = RecordBatchBuilder::new(&schema);
        for (svc, ts, attrs) in [
            ("api", 1i64, vec![("region", "us"), ("tier", "gold")]),
            ("web", 2, vec![("region", "eu")]),
        ] {
            let mut a = std::collections::BTreeMap::new();
            a.insert("service.name".to_string(), svc.to_string());
            for (k, v) in attrs {
                a.insert(k.to_string(), v.to_string());
            }
            builder.append(&LogRecord {
                timestamp_nanos: ts,
                body: Some("x".into()),
                attributes: a,
                ..Default::default()
            });
        }
        let batch = builder.finish().unwrap();
        assert_eq!(
            attribute_keys(&batch),
            vec!["region".to_string(), "tier".to_string()]
        );
    }

    #[tokio::test]
    async fn purge_before_drops_fully_old_files_keeps_straddling_and_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = test_schema();

        // Two segments: seg 0 entirely old (ts 100,200), seg 1 entirely new (ts 5000,6000).
        let old = make_batch(&schema, &[("api", 100, "a"), ("api", 200, "b")]);
        let new = make_batch(&schema, &[("web", 5000, "c"), ("web", 6000, "d")]);
        let wal = Arc::new(FakeWal::with_segments(vec![
            (SegmentId(0), vec![old]),
            (SegmentId(1), vec![new]),
        ]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal, storage.clone(), replicator, schema);

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
            .get(&ObjectPath::from(Storage::parquet_path(SegmentId(0))))
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
        assert_eq!(report3, photon_core::retention::PurgeReport::default());
    }

    /// Poll a sync condition until true, or panic after ~5s.
    async fn wait_until<F: FnMut() -> bool>(mut cond: F) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !cond() {
            assert!(
                std::time::Instant::now() < deadline,
                "condition not met within deadline"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Poll `store` until `path` is absent, or panic after ~5s. Used to observe a durable DELETE
    /// completing (deletes have no `on_durable` callback, so pending()==0 alone can't confirm the
    /// last op finished — the object being present here first and then gone is an unambiguous signal
    /// that the delete ran, since the object was uploaded before we started waiting).
    async fn wait_until_gone(store: &Arc<dyn ObjectStore>, path: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.get(&ObjectPath::from(path)).await.is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "durable object {path} was not deleted within the deadline"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Hot = `LocalFileSystem` (so the compactor can stream Parquet to disk), durable = `InMemory`
    /// so the test can observe what the background replicator uploads to / deletes from durable.
    fn test_storage_with_durable(dir: &std::path::Path) -> (Storage, Arc<dyn ObjectStore>) {
        let durable: Arc<dyn ObjectStore> = Arc::new(object_store::memory::InMemory::new());
        let storage = Storage {
            hot: Arc::new(LocalFileSystem::new_with_prefix(dir).unwrap()),
            durable: Some(durable.clone()),
            hot_dir: Some(dir.to_path_buf()),
        };
        (storage, durable)
    }

    #[tokio::test]
    async fn merge_and_purge_enqueue_durable_deletes_for_superseded_and_expired_objects() {
        // The compactor must enqueue durable deletes (via `Replicator::enqueue_delete`) for the
        // EXACT per-segment parquet/idx objects it removes from hot — both the superseded merge
        // inputs (`merge_once`) and the expired files (`purge_before`) — or the durable replica
        // grows forever. Assert via a durable InMemory fake drained by a real background drain loop.
        let tmp = tempfile::tempdir().unwrap();
        let (storage, durable) = test_storage_with_durable(tmp.path());
        let schema = test_schema();

        // Two small files (both < MERGE_ROW_THRESHOLD): seg 0 and seg 1.
        let seg0 = make_batch(&schema, &[("api", 100, "a"), ("api", 200, "b")]);
        let seg1 = make_batch(&schema, &[("web", 5000, "c"), ("web", 6000, "d")]);
        let wal = Arc::new(FakeWal::with_segments(vec![
            (SegmentId(0), vec![seg0]),
            (SegmentId(1), vec![seg1]),
        ]));
        let replicator = Arc::new(Replicator::new(storage.clone()));
        // Background drain loop against a CLONE sharing the same queue + durable store. `on_durable`
        // records each COMPLETED upload path — the reliable "upload landed in durable" signal
        // (pending()==0 only means the op was popped, not that its put finished).
        let uploaded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = uploaded.clone();
        let drain = (*replicator)
            .clone()
            .spawn_drain_loop(std::time::Duration::from_millis(5), move |p, _b| {
                sink.lock().unwrap().push(p)
            });
        let compactor = Compactor::new(wal, storage.clone(), replicator.clone(), schema);

        let has_uploaded = |path: &str| {
            let path = path.to_string();
            let uploaded = uploaded.clone();
            move || uploaded.lock().unwrap().contains(&path)
        };

        // Compact both → each enqueues an UPLOAD of its parquet + idx. Let replication actually
        // FINISH so both are in durable BEFORE any hot delete (a still-pending upload whose hot
        // object we later delete would fail — pre-existing behavior; keep the flow realistic).
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(1)));
        for seg in [SegmentId(0), SegmentId(1)] {
            wait_until(has_uploaded(&Storage::parquet_path(seg))).await;
            wait_until(has_uploaded(&Storage::index_path(seg))).await;
        }

        // Sanity: uploads still work with the new op-enum queue — both files reached durable.
        for seg in [SegmentId(0), SegmentId(1)] {
            assert!(durable
                .get(&ObjectPath::from(Storage::parquet_path(seg)))
                .await
                .is_ok());
            assert!(durable
                .get(&ObjectPath::from(Storage::index_path(seg)))
                .await
                .is_ok());
        }

        // Merge the two small files. seg0 & seg1 are superseded → their durable objects must be
        // deleted (keyed on the exact per-segment path), and the merged file uploaded.
        assert_eq!(compactor.merge_once().await.unwrap(), 2);

        let merged_seg = {
            let m = load_manifest(&storage).await;
            let e = m.candidates(i64::MIN, i64::MAX);
            assert_eq!(e.len(), 1);
            e[0].segment_id
        };
        // The superseded inputs' durable objects are removed (exact per-segment paths).
        for seg in [SegmentId(0), SegmentId(1)] {
            wait_until_gone(&durable, &Storage::parquet_path(seg)).await;
            wait_until_gone(&durable, &Storage::index_path(seg)).await;
        }
        // The merged file's upload is enqueued BEFORE those deletes (one FIFO queue), so once the
        // deletes are observed the merged upload has definitely landed.
        assert!(durable
            .get(&ObjectPath::from(Storage::parquet_path(merged_seg)))
            .await
            .is_ok());
        assert!(durable
            .get(&ObjectPath::from(Storage::index_path(merged_seg)))
            .await
            .is_ok());

        // Purge the merged file (cutoff past its max ts) → its durable objects must also be deleted
        // — at the EXACT merged-segment path (item 10c), not a prefix.
        let report = compactor.purge_before(i64::MAX).await.unwrap();
        assert_eq!(report.files_removed, 1);
        wait_until_gone(&durable, &Storage::parquet_path(merged_seg)).await;
        wait_until_gone(&durable, &Storage::index_path(merged_seg)).await;

        drain.abort();
    }
}

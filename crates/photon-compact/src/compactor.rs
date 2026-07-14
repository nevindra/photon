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

use crate::stream::{fsync_manifest, hot_local_path, write_parquet_streamed};

/// Promoted attribute that is the primary sort key. Must match `SkipIndex`'s notion of the
/// service column and the schema promoted by `Config::validate`.
const SERVICE_NAME_COLUMN: &str = "service.name";

/// A Parquet file whose `row_count` is below this is a "small" file eligible for merging.
const MERGE_ROW_THRESHOLD: u64 = 10_000;

/// Drains closed WAL segments into the hot object store and maintains the manifest.
///
/// Generic over `W: Wal` so it can be exercised against an in-memory fake in tests; in
/// production `W` is `photon_wal::DiskWal`.
pub struct Compactor<W: Wal> {
    wal: Arc<W>,
    storage: Storage,
    replicator: Arc<Replicator>,
    schema: LogSchema,
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
        }
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
        let schema = self.schema.clone();
        // Resolve the Parquet's real on-disk path under the local hot root so the blocking task
        // can stream the encode straight to a `File` (no whole-file `Vec<u8>`, doc-04 F2). The
        // object path maps 1:1 onto `<hot_dir>/<parquet_path>`, so the same hot store still serves
        // it via `get` and the replicator reads it unchanged.
        let parquet_file = hot_local_path(&self.storage, &Storage::parquet_path(seg))?;
        // Concat + sort + stream-to-disk + skip-index build all run on a blocking thread so they
        // never hold an async worker (doc-04 F3). `batches` is moved in and dropped there.
        let out =
            tokio::task::spawn_blocking(move || compact_segment(&schema, batches, parquet_file))
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

        let (small, large): (Vec<FileEntry>, Vec<FileEntry>) = all
            .into_iter()
            .partition(|e| e.row_count < MERGE_ROW_THRESHOLD);

        if small.len() < 2 {
            return Ok(0);
        }

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

        let mut batches = Vec::new();
        for e in &small {
            batches.extend(self.read_parquet(&e.path).await?);
        }
        let schema = self.schema.clone();
        // Same offload as `run_once`: the merge's concat/sort/stream-encode is pure CPU (doc-04
        // F3). The consolidated file streams to `merged_seg`'s fresh path (temp+fsync+rename+parent
        // -dir fsync inside `write_parquet_streamed`), never touching an input object.
        let parquet_file = hot_local_path(&self.storage, &Storage::parquet_path(merged_seg))?;
        let out =
            tokio::task::spawn_blocking(move || compact_segment(&schema, batches, parquet_file))
                .await
                .map_err(|e| PhotonError::Arrow(format!("compaction task panicked: {e}")))??;

        let entry = out.entry(merged_seg);
        self.put_object(&Storage::index_path(merged_seg), out.index)
            .await?;

        // Single commit point: drop every input (small) entry, add the fresh merged entry, save once.
        let mut new_manifest = Manifest::new();
        for e in large {
            new_manifest.add(e);
        }
        new_manifest.add(entry);
        self.save_manifest(&new_manifest).await?;
        // Durability barrier: make the manifest (now pointing at the fresh merged file) durable
        // BEFORE the point of no return — deleting the superseded input objects below.
        fsync_manifest(&self.storage, MANIFEST_OBJECT_PATH).await?;

        self.replicator.enqueue(Storage::parquet_path(merged_seg));
        self.replicator.enqueue(Storage::index_path(merged_seg));

        // Delete ALL input objects — the fresh id collides with none, so nothing is spared.
        for e in &small {
            self.delete_object(&Storage::parquet_path(e.segment_id))
                .await?;
            self.delete_object(&Storage::index_path(e.segment_id))
                .await?;
        }

        Ok(small.len())
    }

    /// Read every [`RecordBatch`] from a Parquet object in the hot store.
    async fn read_parquet(&self, path: &str) -> Result<Vec<RecordBatch>, PhotonError> {
        let data = self
            .storage
            .hot
            .get(&ObjectPath::from(path))
            .await
            .map_err(|e| PhotonError::Storage(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| PhotonError::Storage(e.to_string()))?;

        let reader = ParquetRecordBatchReaderBuilder::try_new(data)
            .map_err(|e| PhotonError::Arrow(e.to_string()))?
            .build()
            .map_err(|e| PhotonError::Arrow(e.to_string()))?;

        let mut batches = Vec::new();
        for batch in reader {
            batches.push(batch.map_err(|e| PhotonError::Arrow(e.to_string()))?);
        }
        Ok(batches)
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

        for e in &drop {
            self.delete_object_if_present(&Storage::parquet_path(e.segment_id))
                .await?;
            self.delete_object_if_present(&Storage::index_path(e.segment_id))
                .await?;
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
) -> Result<CompactedOut, PhotonError> {
    let concatenated = concat(schema, &batches)?;
    drop(batches); // free the raw WAL batches immediately (doc-04 F2)
    let sorted = sort_by_service_and_timestamp(&concatenated)?;
    drop(concatenated); // free the pre-sort copy before encoding (doc-04 F2)

    write_parquet_streamed(&parquet_file, &sorted)?;

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
    })
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
}

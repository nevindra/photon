//! [`SpanCompactor`]: drains closed spans-WAL segments into `(service.name, start_time)`-sorted
//! zstd Parquet under `data-spans/` with a spans skip-index sidecar, recorded in the spans
//! manifest. Mirrors [`Compactor`](crate::Compactor); kept separate so the logs path is untouched.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Array, MapArray, StringArray};
use arrow::compute::{concat_batches, lexsort_to_indices, take_record_batch, SortColumn};
use arrow::record_batch::RecordBatch;
use object_store::path::Path as ObjectPath;
use object_store::PutPayload;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use photon_core::manifest::{FileEntry, Manifest, SPANS_MANIFEST_OBJECT_PATH};
use photon_core::retention::PurgeReport;
use photon_core::segment::SegmentId;
use photon_core::span_schema::{self, SpanSchema};
use photon_core::PhotonError;
use photon_index::SkipIndex;
use photon_storage::{Replicator, Storage};
use photon_wal::Wal;

use crate::stream::{fsync_manifest, hot_local_path, write_parquet_streamed};

const SERVICE_NAME_COLUMN: &str = "service.name";
const MERGE_ROW_THRESHOLD: u64 = 10_000;

pub struct SpanCompactor<W: Wal> {
    wal: Arc<W>,
    storage: Storage,
    replicator: Arc<Replicator>,
    schema: SpanSchema,
}

impl<W: Wal> SpanCompactor<W> {
    pub fn new(
        wal: Arc<W>,
        storage: Storage,
        replicator: Arc<Replicator>,
        schema: SpanSchema,
    ) -> SpanCompactor<W> {
        SpanCompactor {
            wal,
            storage,
            replicator,
            schema,
        }
    }

    pub async fn run_once(&self) -> Result<Option<SegmentId>, PhotonError> {
        let closed = self.wal.list_closed_segments()?;
        let Some(seg) = closed.into_iter().next() else {
            return Ok(None);
        };

        let batches = self.wal.read_segment(seg).await?;
        let schema = self.schema.clone();
        let parquet_file = hot_local_path(&self.storage, &Storage::parquet_path_spans(seg))?;
        let out =
            tokio::task::spawn_blocking(move || compact_segment(&schema, batches, parquet_file))
                .await
                .map_err(|e| PhotonError::Arrow(format!("compaction task panicked: {e}")))??;

        let entry = out.entry(seg);
        self.put_object(&Storage::index_path_spans(seg), out.index)
            .await?;

        let mut manifest = self.load_manifest().await?;
        manifest.add(entry);
        self.save_manifest(&manifest).await?;
        // Durability barrier: pin the manifest before removing the WAL segment (the only other copy).
        fsync_manifest(&self.storage, SPANS_MANIFEST_OBJECT_PATH).await?;

        self.replicator.enqueue(Storage::parquet_path_spans(seg));
        self.replicator.enqueue(Storage::index_path_spans(seg));

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
        let (small, large): (Vec<FileEntry>, Vec<FileEntry>) = all
            .into_iter()
            .partition(|e| e.row_count < MERGE_ROW_THRESHOLD);
        if small.len() < 2 {
            return Ok(0);
        }

        // Allocate the consolidated file's id from the MERGED (high-bit) namespace, disjoint from
        // every WAL-allocated id — one past the highest existing merged id, or the first merged id.
        // Without this, `max(small)` reuses a WAL id: the merge overwrites an input in place and a
        // later WAL segment reusing that (freed) id clobbers the merged Parquet + idempotently
        // replaces its manifest entry, silently losing every consolidated row.
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
        let parquet_file = hot_local_path(&self.storage, &Storage::parquet_path_spans(merged_seg))?;
        let out =
            tokio::task::spawn_blocking(move || compact_segment(&schema, batches, parquet_file))
                .await
                .map_err(|e| PhotonError::Arrow(format!("compaction task panicked: {e}")))??;

        let entry = out.entry(merged_seg);
        self.put_object(&Storage::index_path_spans(merged_seg), out.index)
            .await?;

        let mut new_manifest = Manifest::new();
        for e in large {
            new_manifest.add(e);
        }
        new_manifest.add(entry);
        self.save_manifest(&new_manifest).await?;
        fsync_manifest(&self.storage, SPANS_MANIFEST_OBJECT_PATH).await?;

        self.replicator
            .enqueue(Storage::parquet_path_spans(merged_seg));
        self.replicator
            .enqueue(Storage::index_path_spans(merged_seg));

        // Delete ALL input objects — the fresh merged id collides with none, so nothing is spared.
        for e in &small {
            self.delete_object(&Storage::parquet_path_spans(e.segment_id))
                .await?;
            self.delete_object(&Storage::index_path_spans(e.segment_id))
                .await?;
        }

        Ok(small.len())
    }

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

    async fn load_manifest(&self) -> Result<Manifest, PhotonError> {
        let path = ObjectPath::from(SPANS_MANIFEST_OBJECT_PATH);
        match self.storage.hot.get(&path).await {
            Ok(result) => {
                let bytes = result
                    .bytes()
                    .await
                    .map_err(|e| PhotonError::Storage(e.to_string()))?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|e| PhotonError::Serde(format!("spans manifest not UTF-8: {e}")))?;
                Manifest::from_json(text)
            }
            Err(object_store::Error::NotFound { .. }) => Ok(Manifest::new()),
            Err(e) => Err(PhotonError::Storage(e.to_string())),
        }
    }

    async fn save_manifest(&self, manifest: &Manifest) -> Result<(), PhotonError> {
        let json = manifest.to_json()?;
        self.put_object(SPANS_MANIFEST_OBJECT_PATH, json.into_bytes())
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
            self.delete_object_if_present(&Storage::parquet_path_spans(e.segment_id))
                .await?;
            self.delete_object_if_present(&Storage::index_path_spans(e.segment_id))
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

/// Everything the spans compaction pipeline produces off the async runtime that the caller still
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
}

impl CompactedOut {
    fn entry(&self, seg: SegmentId) -> FileEntry {
        FileEntry {
            path: Storage::parquet_path_spans(seg),
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

/// concat -> sort by `(service.name, start_time)` -> STREAM the zstd Parquet encode straight to
/// `parquet_file` on disk -> build the spans skip index. Runs on a `spawn_blocking` thread so the
/// concat/lexsort/take/zstd + file I/O never holds a tokio async worker.
fn compact_segment(
    schema: &SpanSchema,
    batches: Vec<RecordBatch>,
    parquet_file: PathBuf,
) -> Result<CompactedOut, PhotonError> {
    let concatenated = concat(schema, &batches)?;
    drop(batches);
    let sorted = sort_by_service_and_start_time(&concatenated)?;
    drop(concatenated);

    write_parquet_streamed(&parquet_file, &sorted)?;

    let index = SkipIndex::build_spans(&sorted)?;
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

fn concat(schema: &SpanSchema, batches: &[RecordBatch]) -> Result<RecordBatch, PhotonError> {
    if batches.is_empty() {
        return Ok(RecordBatch::new_empty(schema.arrow.clone()));
    }
    concat_batches(&batches[0].schema(), batches).map_err(|e| PhotonError::Arrow(e.to_string()))
}

fn sort_by_service_and_start_time(batch: &RecordBatch) -> Result<RecordBatch, PhotonError> {
    let service = batch.column_by_name(SERVICE_NAME_COLUMN).ok_or_else(|| {
        PhotonError::Arrow(format!("batch is missing the {SERVICE_NAME_COLUMN} column"))
    })?;
    let start = batch
        .column_by_name(span_schema::START_TIME)
        .ok_or_else(|| {
            PhotonError::Arrow(format!(
                "batch is missing the {} column",
                span_schema::START_TIME
            ))
        })?;
    let sort_columns = vec![
        SortColumn {
            values: service.clone(),
            options: None,
        },
        SortColumn {
            values: start.clone(),
            options: None,
        },
    ];
    let indices =
        lexsort_to_indices(&sort_columns, None).map_err(|e| PhotonError::Arrow(e.to_string()))?;
    take_record_batch(batch, &indices).map_err(|e| PhotonError::Arrow(e.to_string()))
}

fn attribute_keys(batch: &RecordBatch) -> Vec<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    if let Some(map) = batch
        .column_by_name(span_schema::ATTRIBUTES)
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

    use arrow::array::TimestampNanosecondArray;
    use object_store::local::LocalFileSystem;
    use object_store::ObjectStore;
    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};

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

    fn span(trace: &str, svc: &str, start: i64) -> SpanRecord {
        let mut a = BTreeMap::new();
        a.insert("service.name".to_string(), svc.to_string());
        SpanRecord {
            trace_id: trace.into(),
            span_id: "s".into(),
            name: Some("op".into()),
            start_time_nanos: start,
            attributes: a,
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
    async fn run_once_writes_sorted_spans_parquet_index_and_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = SpanSchema::new(&[SERVICE_NAME_COLUMN.to_string()]);

        let mut b = SpanBatchBuilder::new(&schema);
        for (t, s, start) in [
            ("web", "web", 300i64),
            ("api", "api", 100),
            ("api", "api", 50),
        ] {
            b.append(&span(t, s, start));
        }
        let batch = b.finish().unwrap();

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![(SegmentId(0), vec![batch])]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = SpanCompactor::new(wal, storage.clone(), replicator, schema);

        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(0)));

        let parquet_path = Storage::parquet_path_spans(SegmentId(0));
        assert!(storage
            .hot
            .get(&ObjectPath::from(parquet_path.clone()))
            .await
            .is_ok());
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::index_path_spans(SegmentId(0))))
            .await
            .is_ok());

        // Rows sorted by (service.name, start_time).
        let sorted = read_back(&storage, &parquet_path).await;
        let start = sorted
            .column_by_name("start_time_nanos")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        assert_eq!(
            (start.value(0), start.value(1), start.value(2)),
            (50, 100, 300)
        );

        // Spans manifest has the entry; logs manifest untouched (absent).
        let spans_manifest = storage
            .hot
            .get(&ObjectPath::from(SPANS_MANIFEST_OBJECT_PATH))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let m = Manifest::from_json(std::str::from_utf8(&spans_manifest).unwrap()).unwrap();
        assert_eq!(m.candidates(i64::MIN, i64::MAX).len(), 1);
        assert!(storage
            .hot
            .get(&ObjectPath::from(
                photon_core::manifest::MANIFEST_OBJECT_PATH
            ))
            .await
            .is_err());
    }

    /// Load the spans manifest from the hot store (test-only helper; mirrors the private
    /// `load_manifest` method but reads from outside the compactor for assertions).
    async fn load_manifest(storage: &Storage) -> Manifest {
        let data = storage
            .hot
            .get(&ObjectPath::from(SPANS_MANIFEST_OBJECT_PATH))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        Manifest::from_json(std::str::from_utf8(&data).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn merge_writes_a_fresh_span_segment_id_not_an_input_id() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = SpanSchema::new(&[SERVICE_NAME_COLUMN.to_string()]);

        let mut b0 = SpanBatchBuilder::new(&schema);
        b0.append(&span("web", "web", 300));
        b0.append(&span("api", "api", 100));
        let mut b1 = SpanBatchBuilder::new(&schema);
        b1.append(&span("api", "api", 50));
        b1.append(&span("web", "web", 200));

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![
                (SegmentId(0), vec![b0.finish().unwrap()]),
                (SegmentId(1), vec![b1.finish().unwrap()]),
            ]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = SpanCompactor::new(wal, storage.clone(), replicator, schema);

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

        // Both input Parquet objects are gone — none was reused/overwritten in place.
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path_spans(SegmentId(0))))
            .await
            .is_err());
        assert!(storage
            .hot
            .get(&ObjectPath::from(Storage::parquet_path_spans(SegmentId(1))))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn merged_spans_survive_a_later_wal_segment_reusing_the_old_id() {
        // Regression for the merge-id-collision data-loss bug ported from the logs path. The old
        // allocator picked `max(small input ids)` for the merge output, overwriting an input in
        // place; when the WAL later reused that (freed) id for a new segment, `run_once` clobbered
        // the merged Parquet and `manifest.add` idempotently REPLACED the merged entry — losing
        // every consolidated row. The high-bit merged namespace makes the output disjoint from
        // every WAL id, so both coexist.
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = SpanSchema::new(&[SERVICE_NAME_COLUMN.to_string()]);

        let mut b0 = SpanBatchBuilder::new(&schema);
        b0.append(&span("web", "web", 300));
        b0.append(&span("api", "api", 100));
        let mut b1 = SpanBatchBuilder::new(&schema);
        b1.append(&span("api", "api", 50));
        b1.append(&span("web", "web", 200));

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![
                (SegmentId(0), vec![b0.finish().unwrap()]),
                (SegmentId(1), vec![b1.finish().unwrap()]),
            ]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor =
            SpanCompactor::new(wal.clone(), storage.clone(), replicator, schema.clone());

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
            // SegmentId(1) is exactly what the OLD `max(small)` allocator would have reused.
            assert_ne!(seg, SegmentId(1));
            seg
        };

        // A NEW WAL segment closes reusing SegmentId(1) — the id the old allocator picked for the
        // merge output — carrying its own distinct rows, and gets compacted.
        let mut b2 = SpanBatchBuilder::new(&schema);
        b2.append(&span("zzz", "zzz", 9000));
        b2.append(&span("zzz", "zzz", 9100));
        wal.add_segment(SegmentId(1), vec![b2.finish().unwrap()]);
        assert_eq!(compactor.run_once().await.unwrap(), Some(SegmentId(1)));

        // Both survive — no clobber, no lost rows.
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
            read_back(&storage, &Storage::parquet_path_spans(merged_seg))
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
    async fn purge_before_drops_old_span_files() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = test_storage(tmp.path());
        let schema = SpanSchema::new(&[SERVICE_NAME_COLUMN.to_string()]);

        // Two segments: seg 0 entirely old (ts 100,200), seg 1 entirely new (ts 5000,6000).
        let mut old_builder = SpanBatchBuilder::new(&schema);
        old_builder.append(&span("t1", "api", 100));
        old_builder.append(&span("t2", "api", 200));
        let old = old_builder.finish().unwrap();

        let mut new_builder = SpanBatchBuilder::new(&schema);
        new_builder.append(&span("t3", "web", 5000));
        new_builder.append(&span("t4", "web", 6000));
        let new = new_builder.finish().unwrap();

        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![(SegmentId(0), vec![old]), (SegmentId(1), vec![new])]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = SpanCompactor::new(wal, storage.clone(), replicator, schema);

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
            .get(&ObjectPath::from(Storage::parquet_path_spans(SegmentId(0))))
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
}

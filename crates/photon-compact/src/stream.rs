//! Signal-agnostic streaming-Parquet write + fsync durability helpers, shared by the logs,
//! spans, and metrics compactors. Extracted from the logs `Compactor` (B2/WS3) so all three
//! write paths get identical crash-consistency: stream ONE zstd Parquet file straight to the
//! hot store's backing directory (temp file + fsync + atomic rename + parent-dir fsync — no
//! whole-file `Vec<u8>`), and pin the just-saved manifest to disk before the point of no return.

use std::fs::File;
use std::path::{Path, PathBuf};

use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use photon_core::PhotonError;
use photon_storage::Storage;

/// Default zstd compression level: matches the level `Compression::ZSTD(Default::default())`
/// hardcoded before the level became configurable, so a compactor built via `::new` (without a
/// `.with_zstd_level(..)` override) streams byte-identical Parquet to the pre-config behavior —
/// `parquet::basic::ZstdLevel::try_new(1)` equals `ZstdLevel::default()` in parquet 53.
pub(crate) const DEFAULT_ZSTD_LEVEL: i32 = 1;

/// Cap on rows per Parquet row group, overriding parquet-rs's default of 1,048,576. Without this,
/// `ArrowWriter` buffers an entire row group's column data in memory before flushing it, so a large
/// sorted batch (bounded by `wal.segment_max_bytes`) can transiently hold up to a million rows of
/// decoded columns alongside the batch itself. 128k keeps that buffer small while still leaving
/// row-group pruning (min/max stats per group) reasonably coarse-grained.
pub(crate) const MAX_ROW_GROUP_SIZE: usize = 131_072;

/// Resolve an object path to its real on-disk location under the hot store's local root, so a
/// blocking task can stream a Parquet encode straight to a `File`. The object path maps 1:1 onto
/// `<hot_dir>/<object_path>`, so the same hot store still serves it via `get`. Errors when the hot
/// store is not backed by a local directory (streamed compaction requires one).
pub(crate) fn hot_local_path(storage: &Storage, object_path: &str) -> Result<PathBuf, PhotonError> {
    let root = storage.hot_local_root().ok_or_else(|| {
        PhotonError::Storage(
            "hot store is not backed by a local directory; streamed compaction requires one"
                .to_string(),
        )
    })?;
    Ok(root.join(object_path))
}

/// fsync the just-saved manifest file's contents AND its parent directory entry, making both
/// durable before the caller removes a WAL segment / deletes superseded inputs. A no-op when the
/// hot store is not local (in-memory test stores). `manifest_object_path` is the per-signal
/// manifest object key (logs / spans / metrics).
pub(crate) async fn fsync_manifest(
    storage: &Storage,
    manifest_object_path: &str,
) -> Result<(), PhotonError> {
    let Some(root) = storage.hot_local_root() else {
        return Ok(());
    };
    let manifest_path = root.join(manifest_object_path);
    tokio::task::spawn_blocking(move || fsync_file_and_parent(&manifest_path))
        .await
        .map_err(|e| PhotonError::Io(format!("manifest fsync task panicked: {e}")))?
}

/// Stream a sorted batch to a zstd-compressed Parquet file at `target` via an `ArrowWriter` over a
/// `std::fs::File`, without ever holding the whole compressed file in RAM. Writes to a sibling
/// `.tmp` path in the SAME directory, fsyncs it, atomically renames it into place, then fsyncs the
/// parent directory so the rename itself is crash-durable — a crash mid-write can never leave a
/// torn file visible at `target`, and a crash after the rename can never lose it. The parent dir is
/// created first — a raw `std::fs` write, unlike `object_store::put`, does not auto-create parents.
/// `zstd_level` is the configured `[storage] zstd_level` (validated to `1..=19` at config load);
/// the default (1) is byte-identical to the former hardcoded zstd default, so the file reads back
/// byte-equivalent to what `object_store::put(encode_parquet(..))` produced at that level.
pub(crate) fn write_parquet_streamed(
    target: &Path,
    batch: &RecordBatch,
    zstd_level: i32,
) -> Result<(), PhotonError> {
    let parent = target.parent().ok_or_else(|| {
        PhotonError::Io(format!("parquet target {target:?} has no parent directory"))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| PhotonError::Io(format!("failed to create {parent:?}: {e}")))?;

    let tmp = tmp_path(target);
    let file = File::create(&tmp)
        .map_err(|e| PhotonError::Io(format!("failed to create {tmp:?}: {e}")))?;

    let level = parquet::basic::ZstdLevel::try_new(zstd_level).map_err(|e| {
        PhotonError::Config(format!("invalid storage.zstd_level {zstd_level}: {e}"))
    })?;
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(level))
        .set_max_row_group_size(MAX_ROW_GROUP_SIZE)
        .build();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))
        .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    writer
        .write(batch)
        .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    let file = writer
        .into_inner()
        .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    file.sync_all()
        .map_err(|e| PhotonError::Io(format!("failed to fsync {tmp:?}: {e}")))?;
    drop(file);

    std::fs::rename(&tmp, target)
        .map_err(|e| PhotonError::Io(format!("failed to rename {tmp:?} -> {target:?}: {e}")))?;
    fsync_dir(parent)?;
    Ok(())
}

/// fsync a directory so its recent entry changes (a `rename`/`create`) are durable.
fn fsync_dir(dir: &Path) -> Result<(), PhotonError> {
    let handle = File::open(dir)
        .map_err(|e| PhotonError::Io(format!("failed to open dir {dir:?} for fsync: {e}")))?;
    handle
        .sync_all()
        .map_err(|e| PhotonError::Io(format!("failed to fsync dir {dir:?}: {e}")))
}

/// fsync a file's contents AND its parent directory entry, making both durable.
fn fsync_file_and_parent(path: &Path) -> Result<(), PhotonError> {
    let file = File::open(path)
        .map_err(|e| PhotonError::Io(format!("failed to open {path:?} for fsync: {e}")))?;
    file.sync_all()
        .map_err(|e| PhotonError::Io(format!("failed to fsync {path:?}: {e}")))?;
    drop(file);
    if let Some(parent) = path.parent() {
        fsync_dir(parent)?;
    }
    Ok(())
}

/// Sibling temp path in the SAME directory as `target` (same-filesystem, so the rename is atomic).
fn tmp_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use arrow::array::Int64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    fn sample_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![1, 2, 3]))]).unwrap()
    }

    fn read_back(target: &Path) -> Vec<RecordBatch> {
        let file = File::open(target).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        reader.map(|b| b.unwrap()).collect()
    }

    #[test]
    fn default_level_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(&target, &sample_batch(), DEFAULT_ZSTD_LEVEL).unwrap();

        let batches = read_back(&target);
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
    }

    /// A non-default level (still valid, `1..=19` per `Config::validate`) must also produce a
    /// readable Parquet file — the level only changes the codec's internal effort, not the
    /// logical rows the reader sees back.
    #[test]
    fn non_default_level_still_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(&target, &sample_batch(), 9).unwrap();

        let batches = read_back(&target);
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
    }

    #[test]
    fn out_of_range_level_errors_instead_of_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        let err = write_parquet_streamed(&target, &sample_batch(), 0).unwrap_err();
        assert!(err.to_string().contains("zstd_level"));
    }

    /// A batch bigger than one row group's worth of rows must still be split into multiple row
    /// groups on write — otherwise the `ArrowWriter` buffers the whole batch's column data in one
    /// in-progress row group (parquet-rs's default `max_row_group_size` is 1,048,576 rows), which
    /// is exactly the peak-RSS knob this test locks in. 262,145 rows is just over 2x the intended
    /// cap (131,072) so a correct writer must emit at least 3 row groups, none larger than the cap.
    #[test]
    fn write_parquet_streamed_caps_row_group_size() {
        let cap = MAX_ROW_GROUP_SIZE as i64;
        let n: i64 = cap * 2 + 1;

        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from_iter_values(0..n))])
                .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(&target, &batch, DEFAULT_ZSTD_LEVEL).unwrap();

        let file = File::open(&target).unwrap();
        let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        let row_groups = reader_builder.metadata().row_groups();
        assert!(
            !row_groups.is_empty(),
            "expected at least one row group in the written file"
        );
        for rg in row_groups {
            assert!(
                rg.num_rows() <= cap,
                "row group has {} rows, expected <= {cap}",
                rg.num_rows()
            );
        }
    }
}

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
/// Compression matches the former in-memory encoder exactly (zstd default), so the file reads back
/// byte-equivalent to what `object_store::put(encode_parquet(..))` produced.
pub(crate) fn write_parquet_streamed(
    target: &Path,
    batch: &RecordBatch,
) -> Result<(), PhotonError> {
    let parent = target.parent().ok_or_else(|| {
        PhotonError::Io(format!("parquet target {target:?} has no parent directory"))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| PhotonError::Io(format!("failed to create {parent:?}: {e}")))?;

    let tmp = tmp_path(target);
    let file = File::create(&tmp)
        .map_err(|e| PhotonError::Io(format!("failed to create {tmp:?}: {e}")))?;

    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
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

//! `Storage`: the hot (local disk) and durable (S3-compatible) object stores, plus the
//! fixed object-path scheme that `photon-compact` and `photon-query` agree on.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::ObjectStore;

use photon_core::config::StorageConfig;
use photon_core::segment::SegmentId;
use photon_core::PhotonError;

/// Hot + durable object stores. `hot` is always present (local disk); `durable` is
/// present only when the config declares an S3-compatible durable tier.
#[derive(Clone)]
pub struct Storage {
    pub hot: Arc<dyn ObjectStore>,
    pub durable: Option<Arc<dyn ObjectStore>>,
    /// Local filesystem root backing `hot` (`cfg.storage.hot_dir`), present whenever the hot
    /// store is a `LocalFileSystem` — which it always is in Milestone 1; `None` only for the
    /// in-memory stores used by some tests. Because an object path like `data/<stem>.parquet`
    /// maps 1:1 onto `<hot_dir>/data/<stem>.parquet` on disk, the compactor can stream a Parquet
    /// encode straight to a `File` here (no whole-file `Vec<u8>` buffer) and the object still
    /// reads back through `hot.get(...)`. See [`Storage::hot_local_root`].
    pub hot_dir: Option<PathBuf>,
}

impl Storage {
    /// Build hot (and optionally durable) object stores from config.
    ///
    /// `hot` is a `LocalFileSystem` rooted at `cfg.hot_dir` (created if missing).
    /// `durable`, when `cfg.durable` is set, is an `AmazonS3` store built from
    /// `DurableConfig` (endpoint/bucket/region), with `with_allow_http(true)` so
    /// on-prem / self-hosted S3-compatible endpoints (e.g. MinIO) work over plain HTTP.
    pub fn from_config(cfg: &StorageConfig) -> Result<Storage, PhotonError> {
        std::fs::create_dir_all(&cfg.hot_dir).map_err(|e| {
            PhotonError::Storage(format!("failed to create hot_dir {:?}: {e}", cfg.hot_dir))
        })?;
        let hot = LocalFileSystem::new_with_prefix(&cfg.hot_dir).map_err(|e| {
            PhotonError::Storage(format!("failed to open hot_dir {:?}: {e}", cfg.hot_dir))
        })?;
        let hot: Arc<dyn ObjectStore> = Arc::new(hot);

        let durable = match &cfg.durable {
            Some(durable_cfg) => {
                let mut builder = AmazonS3Builder::new()
                    .with_endpoint(&durable_cfg.endpoint)
                    .with_bucket_name(&durable_cfg.bucket)
                    .with_region(&durable_cfg.region)
                    .with_allow_http(true);
                if let Some(key) = &durable_cfg.access_key_id {
                    builder = builder.with_access_key_id(key);
                }
                if let Some(secret) = &durable_cfg.secret_access_key {
                    builder = builder.with_secret_access_key(secret);
                }
                let s3 = builder.build().map_err(|e| {
                    PhotonError::Storage(format!("failed to build durable S3 store: {e}"))
                })?;
                Some(Arc::new(s3) as Arc<dyn ObjectStore>)
            }
            None => None,
        };

        Ok(Storage {
            hot,
            durable,
            hot_dir: Some(cfg.hot_dir.clone()),
        })
    }

    /// The local filesystem root backing the hot store, if it is a `LocalFileSystem` (always so
    /// in Milestone 1). Callers resolve an object path against it — `root.join(parquet_path)` —
    /// to write a real `File` that the same hot store then serves via `get`. `None` for in-memory
    /// test stores, in which case a caller must fall back to `hot.put(...)`.
    pub fn hot_local_root(&self) -> Option<&Path> {
        self.hot_dir.as_deref()
    }

    /// Parquet object path for a segment: `data/<seg-name-without-.wal>.parquet`.
    pub fn parquet_path(seg: SegmentId) -> String {
        format!("data/{}.parquet", segment_stem(seg))
    }

    /// Skip-index object path for a segment: `data/<seg-name-without-.wal>.idx`.
    pub fn index_path(seg: SegmentId) -> String {
        format!("data/{}.idx", segment_stem(seg))
    }

    /// Parquet object path for a spans segment: `data-spans/<seg-name-without-.wal>.parquet`.
    pub fn parquet_path_spans(seg: SegmentId) -> String {
        format!("data-spans/{}.parquet", segment_stem(seg))
    }

    /// Skip-index object path for a spans segment: `data-spans/<seg-name-without-.wal>.idx`.
    pub fn index_path_spans(seg: SegmentId) -> String {
        format!("data-spans/{}.idx", segment_stem(seg))
    }

    /// Parquet object path for a metrics segment: `data-metrics/<seg-name-without-.wal>.parquet`.
    pub fn parquet_path_metrics(seg: SegmentId) -> String {
        format!("data-metrics/{}.parquet", segment_stem(seg))
    }

    /// Skip-index object path for a metrics segment: `data-metrics/<seg-name-without-.wal>.idx`.
    pub fn index_path_metrics(seg: SegmentId) -> String {
        format!("data-metrics/{}.idx", segment_stem(seg))
    }

    /// Sweep orphaned `*.tmp` files left behind under the hot store's local root.
    ///
    /// `write_parquet_streamed` (`photon-compact`) writes each Parquet to a sibling
    /// `<target>.tmp` path, fsyncs it, then atomically renames it into place. A crash between
    /// `File::create(tmp)` and the `rename` leaves that `.tmp` file on disk forever — nothing
    /// else ever revisits it. This is a best-effort startup sweep (called once in `photon-server`'s
    /// `main`, before the compactors are spawned): it recursively walks `data/`, `data-spans/`,
    /// `data-metrics/`, and any other subdirectory under the root, removing every file whose name
    /// ends in `.tmp` and returning the count removed. Directories are only ever recursed into,
    /// never deleted, and only entries reported as plain files by `DirEntry::file_type()` (which
    /// does not follow symlinks) are considered.
    ///
    /// A no-op returning `0` when the hot store isn't local (`hot_local_root()` is `None` — the
    /// in-memory stores used by some tests), since there's nothing on disk to sweep. Individual
    /// entries that error (permission issues, races with a concurrent writer, etc.) are skipped —
    /// this is best-effort cosmetic cleanup, not a correctness path — but a failure to read the
    /// root itself is a hard error.
    pub fn sweep_stale_tmp_files(&self) -> Result<usize, PhotonError> {
        let Some(root) = self.hot_local_root() else {
            return Ok(0);
        };
        let mut count = 0usize;
        sweep_tmp_dir(root, &mut count, true)?;
        Ok(count)
    }
}

/// Recursive helper for [`Storage::sweep_stale_tmp_files`]. `is_root` controls whether a failure
/// to read `dir` itself is propagated (only true for the very first call) or silently skipped
/// (every deeper directory, so one unreadable subtree can't abort the whole sweep).
fn sweep_tmp_dir(dir: &Path, count: &mut usize, is_root: bool) -> Result<(), PhotonError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if is_root => {
            return Err(PhotonError::Storage(format!(
                "failed to read hot store root {dir:?}: {e}"
            )));
        }
        Err(_) => return Ok(()),
    };

    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            // Best-effort: an unreadable subdirectory doesn't abort the rest of the sweep.
            let _ = sweep_tmp_dir(&path, count, false);
        } else if file_type.is_file()
            && path.extension().is_some_and(|ext| ext == "tmp")
            && std::fs::remove_file(&path).is_ok()
        {
            *count += 1;
        }
    }
    Ok(())
}

/// `SegmentId::filename()` yields `seg-<hex>.wal`; strip the `.wal` extension to get the
/// shared stem used by both the parquet and index object paths.
fn segment_stem(seg: SegmentId) -> String {
    let filename = seg.filename();
    filename
        .strip_suffix(".wal")
        .expect("SegmentId::filename() always ends with .wal")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use object_store::path::Path as ObjectPath;
    use object_store::PutPayload;

    #[tokio::test]
    async fn from_config_with_no_durable_builds_working_hot_store() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = StorageConfig {
            hot_dir: tmp.path().to_path_buf(),
            db_path: String::new(),
            durable: None,
            zstd_level: 1,
        };

        let storage = Storage::from_config(&cfg).unwrap();
        assert!(storage.durable.is_none());

        let path = ObjectPath::from("data/hello.txt");
        storage
            .hot
            .put(&path, PutPayload::from(Bytes::from_static(b"hello world")))
            .await
            .unwrap();

        let got = storage.hot.get(&path).await.unwrap().bytes().await.unwrap();
        assert_eq!(&got[..], b"hello world");
    }

    #[tokio::test]
    async fn from_config_creates_missing_hot_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let hot_dir = tmp.path().join("nested/does/not/exist/yet");
        let cfg = StorageConfig {
            hot_dir: hot_dir.clone(),
            db_path: String::new(),
            durable: None,
            zstd_level: 1,
        };

        let storage = Storage::from_config(&cfg).unwrap();
        assert!(hot_dir.is_dir());

        let path = ObjectPath::from("probe");
        storage
            .hot
            .put(&path, PutPayload::from(Bytes::from_static(b"x")))
            .await
            .unwrap();
    }

    #[test]
    fn object_path_scheme_matches_contract() {
        let seg = SegmentId(255);
        assert_eq!(seg.filename(), "seg-00000000000000ff.wal");
        assert_eq!(
            Storage::parquet_path(seg),
            "data/seg-00000000000000ff.parquet"
        );
        assert_eq!(Storage::index_path(seg), "data/seg-00000000000000ff.idx");
    }

    #[test]
    fn object_path_scheme_handles_segment_zero() {
        let seg = SegmentId(0);
        assert_eq!(
            Storage::parquet_path(seg),
            "data/seg-0000000000000000.parquet"
        );
        assert_eq!(Storage::index_path(seg), "data/seg-0000000000000000.idx");
    }

    #[test]
    fn spans_object_paths_use_a_distinct_prefix() {
        let seg = SegmentId(255);
        assert_eq!(
            Storage::parquet_path_spans(seg),
            "data-spans/seg-00000000000000ff.parquet"
        );
        assert_eq!(
            Storage::index_path_spans(seg),
            "data-spans/seg-00000000000000ff.idx"
        );
        // Never collides with the logs prefix even at the same SegmentId.
        assert_ne!(Storage::parquet_path_spans(seg), Storage::parquet_path(seg));
    }

    #[test]
    fn metrics_paths_use_the_data_metrics_prefix() {
        let seg = SegmentId(0x2a);
        assert!(Storage::parquet_path_metrics(seg).starts_with("data-metrics/"));
        assert!(Storage::parquet_path_metrics(seg).ends_with(".parquet"));
        assert!(Storage::index_path_metrics(seg).starts_with("data-metrics/"));
        assert!(Storage::index_path_metrics(seg).ends_with(".idx"));
    }

    #[test]
    fn sweep_removes_orphaned_tmp_files_but_keeps_real_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = StorageConfig {
            hot_dir: tmp.path().to_path_buf(),
            db_path: String::new(),
            durable: None,
            zstd_level: 1,
        };
        let storage = Storage::from_config(&cfg).unwrap();

        std::fs::create_dir_all(tmp.path().join("data")).unwrap();
        std::fs::create_dir_all(tmp.path().join("data-spans")).unwrap();
        std::fs::create_dir_all(tmp.path().join("data-metrics")).unwrap();

        // Orphaned .tmp files across the three signal subdirs — these must be swept.
        std::fs::write(tmp.path().join("data/x.parquet.tmp"), b"stale").unwrap();
        std::fs::write(tmp.path().join("data-spans/a.parquet.tmp"), b"stale").unwrap();
        std::fs::write(tmp.path().join("data-metrics/b.parquet.tmp"), b"stale").unwrap();

        // A real, already-renamed Parquet file — must survive the sweep.
        std::fs::write(tmp.path().join("data/y.parquet"), b"real").unwrap();

        let removed = storage.sweep_stale_tmp_files().unwrap();
        assert_eq!(removed, 3);

        assert!(!tmp.path().join("data/x.parquet.tmp").exists());
        assert!(!tmp.path().join("data-spans/a.parquet.tmp").exists());
        assert!(!tmp.path().join("data-metrics/b.parquet.tmp").exists());
        assert!(tmp.path().join("data/y.parquet").exists());

        // Sweeping again is a no-op (nothing left to remove).
        let removed_again = storage.sweep_stale_tmp_files().unwrap();
        assert_eq!(removed_again, 0);
    }

    #[test]
    fn sweep_is_a_no_op_for_non_local_hot_store() {
        let hot: Arc<dyn ObjectStore> = Arc::new(object_store::memory::InMemory::new());
        let storage = Storage {
            hot,
            durable: None,
            hot_dir: None,
        };
        assert_eq!(storage.sweep_stale_tmp_files().unwrap(), 0);
    }
}

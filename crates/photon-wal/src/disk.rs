//! Disk-backed WAL with a single-writer group-commit task.
//!
//! ## Durability model (the crux)
//!
//! One background task owns the active segment file. Every `append`/`sync` call hands a
//! command to that task over an mpsc channel and waits on a oneshot ack. The task pulls
//! the first command, then coalesces every further command that arrives within
//! `group_commit_max_delay_ms` into the *same* batch, writes all of their frames in one
//! `write_all`, issues a **single** `File::sync_data`, and only then resolves every ack in
//! the batch. An `append` future therefore never resolves before the fsync covering its
//! bytes has completed — there is no ack before durability.
//!
//! ## Rotation & recovery
//!
//! After a commit the active segment is sealed (moved to the closed set, a fresh active
//! segment is opened) once it exceeds `segment_max_bytes` or `segment_max_age_secs`. On
//! `open`, every pre-existing segment file is enumerated; the highest-id one may have a
//! torn tail from a crash, so it is scanned and physically truncated to its last valid
//! frame. If it still holds data it is sealed as a closed segment and a new empty active
//! segment is started; if it is empty it is reused as the active segment. All lower-id
//! segments are already-closed and remain listed for the compactor to drain.

use crate::frame::{frame_batch, scan_segment};
use crate::Wal;
use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use photon_core::config::WalConfig;
use photon_core::schema::LogSchema;
use photon_core::segment::SegmentId;
use photon_core::PhotonError;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout_at, Instant as TokioInstant};

fn io_err(e: std::io::Error) -> PhotonError {
    PhotonError::Io(e.to_string())
}

/// A durable-commit acknowledgement channel back to a waiting caller.
type Ack = oneshot::Sender<Result<(), PhotonError>>;

/// Capacity of the writer task's command channel. This bounds how many `Append`/`Sync`
/// commands (each holding an already-encoded frame) may be in flight, unacked, at once —
/// if ingest outruns fsync throughput, `send` backpressures callers instead of letting
/// pending frames pile up on the heap without bound. 1024 is generous relative to the
/// group-commit window (a round drains the whole queue every `group_commit_max_delay_ms`)
/// while still capping worst-case buffered memory.
const COMMAND_CHANNEL_CAPACITY: usize = 1024;

/// Commands the writer task processes in FIFO order.
enum Command {
    /// Append a pre-framed record to the active segment, ack after fsync.
    Append { frame: Vec<u8>, ack: Ack },
    /// Force an fsync of the active segment, ack after it completes.
    Sync { ack: Ack },
}

/// State shared between `DiskWal` (sync reader methods) and the writer task.
struct Shared {
    dir: PathBuf,
    inner: Mutex<Inner>,
    /// Number of fsync commit rounds performed — used by tests to prove coalescing.
    commit_rounds: AtomicU64,
}

struct Inner {
    /// Sealed, immutable segments ready for compaction (a `BTreeSet` keeps them ascending).
    closed: BTreeSet<SegmentId>,
}

/// Concrete disk-backed implementation of [`Wal`].
pub struct DiskWal {
    shared: Arc<Shared>,
    tx: mpsc::Sender<Command>,
    schema: Arc<Schema>,
}

impl DiskWal {
    /// Open (or recover) a WAL bound to the log schema. Thin wrapper over [`open_arrow`].
    pub async fn open(
        dir: impl Into<PathBuf>,
        schema: LogSchema,
        cfg: WalConfig,
    ) -> Result<DiskWal, PhotonError> {
        DiskWal::open_arrow(dir, schema.arrow.clone(), cfg).await
    }

    /// Open (or recover) a WAL bound to an arbitrary Arrow schema (logs or spans). Appended
    /// batches are validated against this schema. On open, a torn tail is truncated to its
    /// last valid frame; existing closed segments remain listed for the compactor.
    pub async fn open_arrow(
        dir: impl Into<PathBuf>,
        schema: Arc<Schema>,
        cfg: WalConfig,
    ) -> Result<DiskWal, PhotonError> {
        let dir = dir.into();
        tokio::fs::create_dir_all(&dir).await.map_err(io_err)?;

        // Enumerate existing segment files in ascending id order.
        let mut ids: Vec<SegmentId> = Vec::new();
        let mut rd = tokio::fs::read_dir(&dir).await.map_err(io_err)?;
        while let Some(entry) = rd.next_entry().await.map_err(io_err)? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Ok(id) = SegmentId::parse_filename(&name) {
                ids.push(id);
            }
        }
        ids.sort();

        let mut closed: BTreeSet<SegmentId> = BTreeSet::new();
        let active_id: SegmentId;
        if let Some((&highest, lower)) = ids.split_last() {
            // Every segment below the highest was sealed cleanly before rotation.
            for id in lower {
                closed.insert(*id);
            }
            // The highest is the one that was active at shutdown/crash: recover its tail.
            let valid_len = recover_segment_tail(&dir, highest).await?;
            if valid_len > 0 {
                // Non-empty: seal it and begin a fresh active segment.
                closed.insert(highest);
                active_id = highest.next();
            } else {
                // Empty (or fully torn): reuse the slot as the active segment.
                active_id = highest;
            }
        } else {
            active_id = SegmentId(0);
        }

        // Open the active segment for writing (fresh/truncated -> starts at size 0).
        let active_path = dir.join(active_id.filename());
        let file = new_active_file(&active_path).await.map_err(io_err)?;

        let shared = Arc::new(Shared {
            dir: dir.clone(),
            inner: Mutex::new(Inner { closed }),
            commit_rounds: AtomicU64::new(0),
        });

        let (tx, rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let writer = Writer {
            dir,
            file,
            id: active_id,
            size: 0,
            created: Instant::now(),
            max_bytes: cfg.segment_max_bytes,
            max_age: Duration::from_secs(cfg.segment_max_age_secs),
            shared: shared.clone(),
        };
        tokio::spawn(run_writer(writer, rx, cfg.group_commit_max_delay_ms));

        Ok(DiskWal { shared, tx, schema })
    }

    /// Number of fsync commit rounds so far. Hidden diagnostic used by tests to assert
    /// that concurrent appends are coalesced into fewer fsyncs than there were appends.
    #[doc(hidden)]
    pub fn commit_rounds(&self) -> u64 {
        self.shared.commit_rounds.load(Ordering::Relaxed)
    }

    async fn append_impl(&self, batch: RecordBatch) -> Result<(), PhotonError> {
        // Guard against schema drift so a corrupt/mismatched batch can never enter the log.
        if batch.schema().as_ref() != self.schema.as_ref() {
            return Err(PhotonError::Wal(
                "appended batch schema does not match the WAL schema".into(),
            ));
        }
        let frame = frame_batch(&batch)?;
        let (ack, ack_rx) = oneshot::channel();
        self.tx
            .send(Command::Append { frame, ack })
            .await
            .map_err(|_| PhotonError::Wal("wal writer task is gone".into()))?;
        ack_rx
            .await
            .map_err(|_| PhotonError::Wal("wal writer dropped before ack".into()))?
    }

    async fn sync_impl(&self) -> Result<(), PhotonError> {
        let (ack, ack_rx) = oneshot::channel();
        self.tx
            .send(Command::Sync { ack })
            .await
            .map_err(|_| PhotonError::Wal("wal writer task is gone".into()))?;
        ack_rx
            .await
            .map_err(|_| PhotonError::Wal("wal writer dropped before ack".into()))?
    }

    fn list_closed_impl(&self) -> Result<Vec<SegmentId>, PhotonError> {
        let inner = self.shared.inner.lock().expect("wal state poisoned");
        Ok(inner.closed.iter().copied().collect())
    }

    /// Read a closed segment off the tokio runtime: this can be up to `segment_max_bytes`
    /// (e.g. 128MB) and is called from the async compactor loop, so a synchronous
    /// `std::fs::read` here would block a shared runtime worker thread. `tokio::fs::read`
    /// runs the read via `spawn_blocking` instead.
    async fn read_segment_impl(&self, id: SegmentId) -> Result<Vec<RecordBatch>, PhotonError> {
        let path = self.shared.dir.join(id.filename());
        let bytes = tokio::fs::read(&path).await.map_err(io_err)?;
        let (batches, _valid_len) = scan_segment(&bytes);
        Ok(batches)
    }

    fn remove_segment_impl(&self, id: SegmentId) -> Result<(), PhotonError> {
        {
            let mut inner = self.shared.inner.lock().expect("wal state poisoned");
            inner.closed.remove(&id);
        }
        let path = self.shared.dir.join(id.filename());
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            // Idempotent: a segment already gone counts as removed.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }
}

// Inherent methods mirror the trait so `DiskWal` values (and `Arc<DiskWal>`) expose the
// API directly with concrete `Send` futures, while `impl Wal` keeps generic consumers
// working. Inherent methods win name resolution, so callers get the concrete versions.
impl DiskWal {
    pub async fn append(&self, batch: RecordBatch) -> Result<(), PhotonError> {
        self.append_impl(batch).await
    }
    pub async fn sync(&self) -> Result<(), PhotonError> {
        self.sync_impl().await
    }
    pub fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
        self.list_closed_impl()
    }
    pub async fn read_segment(&self, id: SegmentId) -> Result<Vec<RecordBatch>, PhotonError> {
        self.read_segment_impl(id).await
    }
    pub fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError> {
        self.remove_segment_impl(id)
    }
}

// The `impl Future + Send` return type is load-bearing (generic consumers need the `Send`
// bound), so the `manual_async_fn` suggestion to rewrite these as `async fn` — which would
// drop the explicit bound — does not apply.
#[allow(clippy::manual_async_fn)]
impl Wal for DiskWal {
    fn append(
        &self,
        batch: RecordBatch,
    ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
        // `append_impl` only awaits a `oneshot` receiver (Send) and never holds the
        // `Shared::inner` `MutexGuard` across an `.await`, so this future is `Send`.
        async move { self.append_impl(batch).await }
    }
    fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
        async move { self.sync_impl().await }
    }
    fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
        self.list_closed_impl()
    }
    fn read_segment(
        &self,
        id: SegmentId,
    ) -> impl std::future::Future<Output = Result<Vec<RecordBatch>, PhotonError>> + Send {
        // `read_segment_impl` only awaits `tokio::fs::read` (Send) and holds no non-Send
        // state across it, so this future is `Send`.
        async move { self.read_segment_impl(id).await }
    }
    fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError> {
        self.remove_segment_impl(id)
    }
}

/// Create (or truncate) an active segment file. Truncation drops any stale/torn bytes so
/// the writer always starts a session at size 0.
async fn new_active_file(path: &Path) -> std::io::Result<tokio::fs::File> {
    tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await
}

/// Recover the (possibly torn) tail of a segment: scan for the last valid frame and
/// physically truncate any trailing partial bytes. Returns the valid byte length.
async fn recover_segment_tail(dir: &Path, id: SegmentId) -> Result<usize, PhotonError> {
    let path = dir.join(id.filename());
    let bytes = tokio::fs::read(&path).await.map_err(io_err)?;
    let (_batches, valid_len) = scan_segment(&bytes);
    if valid_len < bytes.len() {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .map_err(io_err)?;
        file.set_len(valid_len as u64).await.map_err(io_err)?;
        file.sync_all().await.map_err(io_err)?;
    }
    Ok(valid_len)
}

/// Frames + acks accumulated for one fsync commit round.
struct Pending {
    frames: Vec<Vec<u8>>,
    acks: Vec<Ack>,
}

impl Pending {
    fn new() -> Pending {
        Pending {
            frames: Vec::new(),
            acks: Vec::new(),
        }
    }
    fn push(&mut self, cmd: Command) {
        match cmd {
            Command::Append { frame, ack } => {
                self.frames.push(frame);
                self.acks.push(ack);
            }
            Command::Sync { ack } => self.acks.push(ack),
        }
    }
}

/// Owns the active segment file and performs all writes/fsyncs/rotations.
struct Writer {
    dir: PathBuf,
    file: tokio::fs::File,
    id: SegmentId,
    size: u64,
    created: Instant,
    max_bytes: u64,
    max_age: Duration,
    shared: Arc<Shared>,
}

impl Writer {
    /// Write every pending frame, fsync once, then resolve every ack. All acks in a round
    /// see the same outcome; on failure each caller gets its own error value.
    async fn commit(&mut self, pending: Pending) {
        // Avoid concatenating into a fresh buffer when it isn't needed: the common case is
        // one frame per round (no coalescing window, or a lone straggler), which can be
        // written directly with no copy; only 2+ frames need a concat, and even then the
        // total length is known up front so the buffer is allocated exactly once.
        let total_len: usize = pending.frames.iter().map(Vec::len).sum();

        let result: Result<(), PhotonError> = async {
            match pending.frames.as_slice() {
                [] => {}
                [only] => self.file.write_all(only).await.map_err(io_err)?,
                frames => {
                    let mut buf = Vec::with_capacity(total_len);
                    for frame in frames {
                        buf.extend_from_slice(frame);
                    }
                    self.file.write_all(&buf).await.map_err(io_err)?;
                }
            }
            // Single fsync covering every byte in this round — the durability boundary.
            self.file.sync_data().await.map_err(io_err)?;
            Ok(())
        }
        .await;

        self.shared.commit_rounds.fetch_add(1, Ordering::Relaxed);

        match &result {
            Ok(()) => {
                self.size += total_len as u64;
                for ack in pending.acks {
                    let _ = ack.send(Ok(()));
                }
                self.maybe_rotate().await;
            }
            Err(e) => {
                let msg = e.to_string();
                for ack in pending.acks {
                    let _ = ack.send(Err(PhotonError::Io(msg.clone())));
                }
            }
        }
    }

    /// Seal the active segment and start a fresh one if it grew past its size or age bound.
    async fn maybe_rotate(&mut self) {
        // Never seal an empty segment (keeps age-based checks from spawning empty segments).
        if self.size == 0 {
            return;
        }
        if self.size < self.max_bytes && self.created.elapsed() < self.max_age {
            return;
        }
        let new_id = self.id.next();
        let new_path = self.dir.join(new_id.filename());
        // Create the successor first; only flip state once it exists so a failure here
        // leaves the current (oversized) segment usable and simply retries next commit.
        let new_file = match new_active_file(&new_path).await {
            Ok(f) => f,
            Err(_) => return,
        };
        {
            let mut inner = self.shared.inner.lock().expect("wal state poisoned");
            inner.closed.insert(self.id);
        }
        self.file = new_file;
        self.id = new_id;
        self.size = 0;
        self.created = Instant::now();
    }
}

/// The writer task: block for one command, coalesce the group-commit window, commit once.
async fn run_writer(mut writer: Writer, mut rx: mpsc::Receiver<Command>, delay_ms: u64) {
    loop {
        let first = match rx.recv().await {
            Some(cmd) => cmd,
            None => break, // all senders dropped -> shut down
        };
        let mut pending = Pending::new();
        pending.push(first);

        let mut disconnected = false;
        if delay_ms > 0 {
            // Coalesce everything that arrives within the window into this commit round.
            let deadline = TokioInstant::now() + Duration::from_millis(delay_ms);
            loop {
                match timeout_at(deadline, rx.recv()).await {
                    Ok(Some(cmd)) => pending.push(cmd),
                    Ok(None) => {
                        disconnected = true;
                        break;
                    }
                    Err(_) => break, // window elapsed
                }
            }
        } else {
            // No window: still coalesce whatever is already queued (non-blocking).
            while let Ok(cmd) = rx.try_recv() {
                pending.push(cmd);
            }
        }

        writer.commit(pending).await;
        if disconnected {
            break;
        }
    }
}

#[cfg(test)]
mod span_wal_tests {
    use super::*;
    use photon_core::config::WalConfig;
    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;
    use std::collections::BTreeMap;

    fn wal_cfg() -> WalConfig {
        WalConfig {
            segment_max_bytes: 1 << 20,
            segment_max_age_secs: 60,
            group_commit_max_delay_ms: 0,
        }
    }

    #[tokio::test]
    async fn appends_a_span_batch_through_an_arrow_schema_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let schema = SpanSchema::new(&["service.name".to_string()]);
        let wal = DiskWal::open_arrow(
            tmp.path().join("wal-traces"),
            schema.arrow.clone(),
            wal_cfg(),
        )
        .await
        .unwrap();

        let mut b = SpanBatchBuilder::new(&schema);
        let mut attrs = BTreeMap::new();
        attrs.insert("service.name".to_string(), "checkout".to_string());
        b.append(&SpanRecord {
            trace_id: "t1".into(),
            span_id: "s1".into(),
            start_time_nanos: 100,
            attributes: attrs,
            ..Default::default()
        });
        let batch = b.finish().unwrap();

        // Appending a matching-schema batch succeeds (ack = durable).
        wal.append(batch).await.unwrap();
        wal.sync().await.unwrap();
    }
}

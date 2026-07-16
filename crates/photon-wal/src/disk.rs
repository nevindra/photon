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
//! segment is opened) once it exceeds `segment_max_bytes` or `segment_max_age_secs`; the
//! writer's idle wait also wakes at the age deadline, so a non-empty segment on an idle
//! instance still seals on time without waiting for the next write. On
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
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
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
            dirty_tail: false,
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
    /// Poison flag: a prior commit's compound failure (both `roll_back_to_committed` AND
    /// `rotate_after_failed_rollback` failed) may have left a torn tail past `self.size` with
    /// the OS write cursor parked past it — an unknown, unrecovered state. While set, the next
    /// `commit` MUST recover (truncate/reposition to `self.size`, or rotate onto a fresh
    /// segment) BEFORE writing any acked data; if it can't, it refuses to write. This upholds
    /// the invariant that `commit` never appends acked bytes unless the cursor is at `self.size`.
    dirty_tail: bool,
}

impl Writer {
    /// Write every pending frame, fsync once, then resolve every ack. All acks in a round
    /// see the same outcome; on failure each caller gets its own error value.
    async fn commit(&mut self, pending: Pending) {
        // Recover-before-write: if a prior round's compound failure poisoned the writer (a torn
        // tail may sit past `self.size` with the OS cursor parked past it), we must NOT append
        // acked bytes on top of that. Reset to the last durable offset first: try to roll back
        // (truncate + seek to `self.size`), else rotate onto a fresh clean segment. Only if BOTH
        // still fail do we refuse to write — fail every ack, leave `self.size` untouched, and
        // return without writing, so nothing is acked and nothing lands past the torn tail (no
        // acked round can be stranded behind a torn frame where crash recovery would drop it).
        if self.dirty_tail {
            if self.roll_back_to_committed().await.is_ok()
                || self.rotate_after_failed_rollback().await
            {
                self.dirty_tail = false;
            } else {
                let msg = "wal writer has an unrecovered torn tail; refusing to append";
                for ack in pending.acks {
                    let _ = ack.send(Err(PhotonError::Io(msg.to_string())));
                }
                return;
            }
        }

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
                // The failed write may have left a torn tail past `self.size`. Discard it so
                // the next (acked) round appends at the last durable offset; if even that
                // fails, rotate onto a fresh segment. If BOTH fail (disk genuinely gone), the
                // cursor is left past a torn tail with no clean segment — poison the writer so
                // the next `commit` recovers before writing rather than appending past the tear.
                // Either way, no acked round can end up stranded behind a torn frame where crash
                // recovery would drop it.
                if self.roll_back_to_committed().await.is_err()
                    && !self.rotate_after_failed_rollback().await
                {
                    self.dirty_tail = true;
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

    /// Discard a torn tail left behind by a failed [`commit`]: bytes that a failed
    /// `write_all`/`sync_data` wrote past `self.size` but that were never acked (`self.size`
    /// was not advanced to cover them). Restore the active segment to its last durable length
    /// so the *next* (successful, acked) round appends at `self.size` — not after the torn
    /// frame. Otherwise crash recovery (`scan_segment`) would stop at that torn frame and drop
    /// every later acked round: acked data loss, exactly when the disk is already failing.
    ///
    /// Truncates the file to `self.size` and rewinds the write cursor to match — a partial
    /// `write_all` leaves the cursor past `self.size`, so `set_len` alone would let the next
    /// write re-open the gap. Returns `Err` if the truncate/seek itself fails (the disk is
    /// genuinely gone); the caller then rotates onto a fresh segment.
    async fn roll_back_to_committed(&mut self) -> Result<(), PhotonError> {
        self.file.set_len(self.size).await.map_err(io_err)?;
        self.file
            .seek(SeekFrom::Start(self.size))
            .await
            .map_err(io_err)?;
        Ok(())
    }

    /// When the active segment holds data, the instant its age bound expires — the writer's
    /// idle wait wakes then to seal it, so the tail of ingested data becomes readable by the
    /// compactor even when no further traffic ever arrives. `None` while empty (an empty
    /// segment never age-rotates, so there is nothing to wake up for).
    ///
    /// Floored at 1s from now: if the deadline is already past (a prior idle rotation attempt
    /// failed — e.g. successor creation failed on a failing disk), the wake retries at a 1s
    /// cadence instead of spinning hot on an immediately-elapsed timer.
    fn age_deadline(&self) -> Option<TokioInstant> {
        if self.size == 0 {
            return None;
        }
        let deadline = TokioInstant::from_std(self.created + self.max_age);
        Some(deadline.max(TokioInstant::now() + Duration::from_secs(1)))
    }

    /// Fallback when [`roll_back_to_committed`] itself fails (the disk is genuinely gone):
    /// rotate onto a fresh segment using the same successor-first pattern as [`maybe_rotate`]
    /// — create the successor first, then flip state — so a later failure can't strand the
    /// writer. The torn segment is sealed as-is; it still recovers cleanly up to `self.size`
    /// on read (`scan_segment` drops the torn tail), so no acked round is lost, and the next
    /// append lands on the fresh segment. An empty torn segment is not sealed (nothing durable
    /// to preserve), mirroring `maybe_rotate`.
    ///
    /// Returns `true` if the rotation succeeded (the writer now sits on a fresh clean segment
    /// with a known-good cursor at offset 0) and `false` if even creating the successor failed
    /// (the writer is still on the torn segment). The caller uses this to decide whether to
    /// poison the writer (`dirty_tail`).
    async fn rotate_after_failed_rollback(&mut self) -> bool {
        let new_id = self.id.next();
        let new_path = self.dir.join(new_id.filename());
        let new_file = match new_active_file(&new_path).await {
            Ok(f) => f,
            Err(_) => return false,
        };
        if self.size > 0 {
            let mut inner = self.shared.inner.lock().expect("wal state poisoned");
            inner.closed.insert(self.id);
        }
        self.file = new_file;
        self.id = new_id;
        self.size = 0;
        self.created = Instant::now();
        true
    }
}

/// The writer task: wait for a command, coalesce the group-commit window, commit once.
/// The idle wait wakes at the active segment's age deadline: a plain `recv().await` would
/// sleep until the NEXT write arrives, so on a low-traffic instance the tail of ingested
/// data would sit in the active segment — invisible to the compactor, and therefore to
/// queries — indefinitely (age rotation used to run only after a commit).
async fn run_writer(mut writer: Writer, mut rx: mpsc::Receiver<Command>, delay_ms: u64) {
    loop {
        let first = loop {
            match writer.age_deadline() {
                Some(deadline) => match timeout_at(deadline, rx.recv()).await {
                    Ok(Some(cmd)) => break cmd,
                    Ok(None) => return, // all senders dropped -> shut down
                    Err(_) => writer.maybe_rotate().await, // aged while idle -> seal it
                },
                None => match rx.recv().await {
                    Some(cmd) => break cmd,
                    None => return, // all senders dropped -> shut down
                },
            }
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

#[cfg(test)]
mod rollback_tests {
    use super::*;
    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;
    use std::collections::BTreeMap;

    /// One log row, framed exactly as `append` would frame it, ready for `commit`.
    fn one_row_frame(schema: &LogSchema, ts: i64, body: &str) -> Vec<u8> {
        let mut b = RecordBatchBuilder::new(schema);
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), "svc".to_string());
        b.append(&LogRecord {
            timestamp_nanos: ts,
            body: Some(body.to_string()),
            attributes,
            ..Default::default()
        });
        frame_batch(&b.finish().unwrap()).unwrap()
    }

    /// A bare `Writer` over a fresh segment 0, with rotation bounds high enough that the test
    /// controls every state transition itself (no size/age rotation interferes).
    async fn new_test_writer(dir: &Path) -> Writer {
        let id = SegmentId(0);
        let file = new_active_file(&dir.join(id.filename())).await.unwrap();
        let shared = Arc::new(Shared {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Inner {
                closed: BTreeSet::new(),
            }),
            commit_rounds: AtomicU64::new(0),
        });
        Writer {
            dir: dir.to_path_buf(),
            file,
            id,
            size: 0,
            created: Instant::now(),
            max_bytes: 1 << 30,
            max_age: Duration::from_secs(3600),
            shared,
            dirty_tail: false,
        }
    }

    /// Drive one frame through the real `commit` path and assert its ack is `Ok` (durable).
    async fn good_commit(writer: &mut Writer, frame: Vec<u8>) {
        let (ack, ack_rx) = oneshot::channel();
        let mut pending = Pending::new();
        pending.push(Command::Append { frame, ack });
        writer.commit(pending).await;
        ack_rx.await.unwrap().unwrap();
    }

    // Audit F3 durability hole: a failed commit must not strand a later *acked* round behind a
    // torn frame. After a clean round, garbage past `self.size` (the torn tail a failed write
    // leaves behind) is rolled back; the next clean round then lands at the last durable
    // offset, and crash recovery (`scan_segment`) recovers *both* rounds with no truncation of
    // acked data.
    #[tokio::test]
    async fn rollback_discards_torn_tail_so_recovery_keeps_acked_rounds() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let schema = LogSchema::new(&["service.name".to_string()]);
        let seg = dir.join(SegmentId(0).filename());

        let mut writer = new_test_writer(dir).await;

        // Round 1: a clean, durable commit.
        good_commit(&mut writer, one_row_frame(&schema, 1, "alpha")).await;
        let committed = writer.size;
        assert!(committed > 0, "first commit advanced self.size");

        // Simulate the F3 hole: a failed write left partial bytes past `self.size` that were
        // never acked (so `self.size` was never advanced to cover them).
        writer.file.write_all(&[0xABu8; 40]).await.unwrap();
        writer.file.flush().await.unwrap();
        assert_eq!(
            tokio::fs::metadata(&seg).await.unwrap().len(),
            committed + 40,
            "the torn tail is on disk before rollback",
        );

        // Roll back to the last durable offset.
        writer.roll_back_to_committed().await.unwrap();
        assert_eq!(
            tokio::fs::metadata(&seg).await.unwrap().len(),
            committed,
            "rollback truncates the torn tail back to self.size",
        );
        assert_eq!(
            writer.size, committed,
            "rollback leaves self.size unchanged"
        );

        // Round 2: the next clean commit must land at the last durable offset, not after the
        // discarded torn bytes.
        good_commit(&mut writer, one_row_frame(&schema, 2, "beta")).await;

        // Crash recovery: scan the raw segment image. Both acked rounds must survive — no torn
        // frame in the middle stranding the second round.
        let bytes = tokio::fs::read(&seg).await.unwrap();
        let (batches, valid_len) = scan_segment(&bytes);
        assert_eq!(
            valid_len,
            bytes.len(),
            "no torn frame: every acked byte is part of a valid frame",
        );
        assert_eq!(
            valid_len, writer.size as usize,
            "recovery matches self.size"
        );
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 2, "both acked rounds recovered");
    }

    // FIX 1: the compound rollback+rotate failure aftermath. When a failed commit's rollback AND
    // rotate BOTH failed, the writer is poisoned (`dirty_tail = true`) still sitting on the OLD
    // segment with a torn tail past `self.size` and the OS write cursor parked PAST that tail.
    // The next `commit` MUST recover BEFORE writing — truncate the torn tail and reposition to
    // `self.size` — so the round-2 frame lands at the last durable offset, not on top of the
    // garbage. Without the pre-write recovery, the frame appends at the stale cursor (past the
    // tear); crash recovery (`scan_segment`) then stops at the torn frame and drops the later
    // acked round: acked data loss, the exact hole this fix closes. This simulates the aftermath
    // directly (no fault injection): garbage on disk past S1 + the cursor left past it + the
    // poison flag set, then a normal round 2.
    #[tokio::test]
    async fn dirty_tail_pre_write_recovery_keeps_acked_rounds_after_compound_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let schema = LogSchema::new(&["service.name".to_string()]);
        let seg = dir.join(SegmentId(0).filename());

        let mut writer = new_test_writer(dir).await;

        // Round 1: a clean, durable commit → self.size = S1.
        good_commit(&mut writer, one_row_frame(&schema, 1, "alpha")).await;
        let s1 = writer.size;
        assert!(s1 > 0, "first commit advanced self.size");

        // Simulate the compound-failure aftermath: a failed write left garbage past S1 that was
        // never acked (self.size never advanced), the failed rollback never truncated it, and the
        // failed rotate never moved off the segment — so the OS cursor now sits PAST the garbage
        // and the writer is poisoned.
        writer.file.write_all(&[0xABu8; 40]).await.unwrap();
        writer.file.flush().await.unwrap();
        writer.dirty_tail = true;
        assert_eq!(
            tokio::fs::metadata(&seg).await.unwrap().len(),
            s1 + 40,
            "torn tail is on disk before the poisoned round runs",
        );

        // Round 2: a normal commit. `commit` must recover FIRST (truncate to S1, seek to S1),
        // clear the poison, then append the round-2 frame AT S1 — not after the 40 garbage bytes.
        let round2_frame = one_row_frame(&schema, 2, "beta");
        let round2_len = round2_frame.len() as u64;
        good_commit(&mut writer, round2_frame).await;

        assert!(
            !writer.dirty_tail,
            "pre-write recovery cleared the poison flag",
        );
        // If the frame had appended at the stale cursor (S1 + 40) the file would be longer than
        // self.size; instead self.size advanced by exactly the frame length FROM S1, and the file
        // length matches — proving the frame landed at S1 with the torn tail discarded.
        assert_eq!(
            writer.size,
            s1 + round2_len,
            "round-2 frame landed at S1: self.size advanced by exactly its length",
        );
        assert_eq!(
            tokio::fs::metadata(&seg).await.unwrap().len(),
            writer.size,
            "file length matches self.size — no garbage stranded past the frame",
        );

        // Crash recovery: scan the raw segment image. Both acked rounds must survive — no torn
        // frame in the middle stranding the second round.
        let bytes = tokio::fs::read(&seg).await.unwrap();
        let (batches, valid_len) = scan_segment(&bytes);
        assert_eq!(
            valid_len,
            bytes.len(),
            "no torn frame: every acked byte is part of a valid frame",
        );
        assert_eq!(
            valid_len, writer.size as usize,
            "recovery matches self.size"
        );
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 2, "both acked rounds recovered");
    }
}

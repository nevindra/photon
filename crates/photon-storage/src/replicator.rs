//! `Replicator`: background hot -> durable object copier with retry/backoff.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, PutPayload};

use crate::storage::Storage;

/// Number of attempts (including the first) made to replicate a single object before it
/// is re-enqueued (see [`Replicator::spawn_drain_loop`]) rather than uploaded.
const MAX_ATTEMPTS: u32 = 5;
/// Base delay for exponential backoff between per-item retries.
const BASE_BACKOFF: Duration = Duration::from_millis(10);
/// Base delay applied *after* an item exhausts `MAX_ATTEMPTS` and is re-enqueued, escalated
/// (doubled per additional consecutive failure) while the durable store keeps rejecting
/// every item — i.e. it looks evidently down rather than transiently flaky. This is
/// deliberately much larger than `BASE_BACKOFF` (which only covers the fast per-item retry
/// window) so a permanently-failing item re-enqueued to the tail is retried on a slow,
/// bounded cadence instead of busy-looping the drain loop.
const PASS_FAILURE_BACKOFF_BASE: Duration = Duration::from_millis(500);
/// Upper bound on the escalating pass-failure backoff.
const PASS_FAILURE_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// A single unit of durable-replication work drained by [`Replicator::spawn_drain_loop`]. The
/// queue carries BOTH kinds so uploads and deletes drain through one FIFO loop, preserving their
/// enqueue order — a delete for an object is always enqueued *after* that object's upload, so it
/// can never race ahead of it (which a second, independent delete queue could not guarantee).
enum ReplicaOp {
    /// Copy the hot object at this path to durable (the original replicator behavior).
    Upload(String),
    /// Delete the object at this path from durable, enforcing durable-tier retention. A durable
    /// `NotFound` is treated as success — the object may never have been replicated, or was
    /// already deleted.
    Delete(String),
}

/// Background hot->durable replicator. Objects enqueued via [`Replicator::enqueue`] are copied
/// from `storage.hot` to `storage.durable`, and paths enqueued via [`Replicator::enqueue_delete`]
/// are removed from `storage.durable`, by the single long-lived drain loop started with
/// [`Replicator::spawn_drain_loop`]. Both op kinds share ONE FIFO queue, so a delete never races
/// ahead of the upload of the same object.
#[derive(Clone)]
pub struct Replicator {
    storage: Storage,
    queue: Arc<Mutex<VecDeque<ReplicaOp>>>,
    /// Count of ops (uploads or deletes) that have exhausted `MAX_ATTEMPTS` retries and been
    /// re-enqueued (never dropped). Exposed via [`Replicator::failed_attempts`] as a
    /// replication-health signal — a nonzero, growing count means the durable store is failing
    /// and falling behind, not just running slow. A durable `NotFound` on a delete is NOT a
    /// failure and never bumps this counter.
    failed_attempts: Arc<AtomicU64>,
}

impl Replicator {
    pub fn new(storage: Storage) -> Replicator {
        Replicator {
            storage,
            queue: Arc::new(Mutex::new(VecDeque::new())),
            failed_attempts: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Enqueue a hot object path for replication (an upload). No-op if `durable` is `None`.
    pub fn enqueue(&self, path: String) {
        if self.storage.durable.is_none() {
            return;
        }
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back(ReplicaOp::Upload(path));
    }

    /// Enqueue a durable-tier DELETE for `path`. No-op if `durable` is `None` (mirrors
    /// [`Replicator::enqueue`]). Drained by the SAME long-lived loop as uploads, in FIFO order, so
    /// the delete of an object always trails that object's upload. A durable `NotFound` at delete
    /// time counts as success and is never re-enqueued. Used by the compactors to enforce
    /// durable-tier retention when they delete a superseded (merge input) or expired (purge) object
    /// from the hot store — without it, the durable replica would grow forever.
    pub fn enqueue_delete(&self, path: String) {
        if self.storage.durable.is_none() {
            return;
        }
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back(ReplicaOp::Delete(path));
    }

    /// Current queue depth (exposed as the replication-lag metric).
    pub fn pending(&self) -> usize {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Whether a durable (S3/Garage) replica is configured.
    pub fn durable_configured(&self) -> bool {
        self.storage.durable.is_some()
    }

    /// Total number of items that have exhausted retries and been re-enqueued (never
    /// dropped). Monotonically increasing; exposed as a replication-health metric so a
    /// durable-store outage is observable instead of silently diverging hot from durable.
    pub fn failed_attempts(&self) -> u64 {
        self.failed_attempts.load(Ordering::Relaxed)
    }

    /// Spawn the single, long-lived durable drain loop; returns its handle. Unlike a
    /// drain-then-exit task, this is spawned **once** at startup and runs for the whole process
    /// lifetime — it is never re-spawned per compaction tick. Each queue entry is either an UPLOAD
    /// (copy hot->durable, then call `on_durable(path, bytes)` with the uploaded byte length so the
    /// caller can forward it to the usage recorder / manifest) or a DELETE (remove the object from
    /// durable to enforce retention; a durable `NotFound` counts as success). Only uploads fire
    /// `on_durable` — deletes never do, so the callback signature is unchanged.
    ///
    /// The loop drains the queue **one op at a time**: it fully awaits each upload/delete before
    /// popping the next, so in-flight uploads — and thus peak whole-file-buffer RAM — are
    /// bounded to exactly one regardless of how far durable replication lags. When the queue is
    /// empty it idles for `interval` (instead of exiting) and then re-checks. An op that
    /// exhausts `MAX_ATTEMPTS` retries is re-enqueued to the tail (never dropped) with a warning
    /// logged and [`Replicator::failed_attempts`] incremented; consecutive re-enqueue events (no
    /// success in between, i.e. the durable store looks evidently down) escalate the delay before
    /// the next op is attempted, so a permanently-failing op re-enqueued to the tail cannot
    /// busy-loop this task. A delete that hits `NotFound` is immediate success — not counted, not
    /// re-enqueued. No-op (returns immediately) when no durable tier is configured.
    pub fn spawn_drain_loop<F>(
        self,
        interval: Duration,
        on_durable: F,
    ) -> tokio::task::JoinHandle<()>
    where
        F: Fn(String, u64) + Send + 'static,
    {
        let Replicator {
            storage,
            queue,
            failed_attempts,
        } = self;
        tokio::spawn(async move {
            let Some(durable) = storage.durable.clone() else {
                return;
            };
            let hot = storage.hot.clone();
            // Consecutive re-enqueue events observed by this loop (reset on any success).
            // Drives the escalating pass-failure backoff below.
            let mut consecutive_failures: u32 = 0;
            loop {
                let next = {
                    let mut q = queue.lock().unwrap_or_else(|p| p.into_inner());
                    q.pop_front()
                };
                let Some(op) = next else {
                    // Queue drained: idle until the next wake instead of exiting, so exactly one
                    // drain task lives for the whole process. A plain interval tick is sufficient
                    // — replication is an async replica, never on the ack/query path — so a small
                    // pickup latency for freshly-enqueued objects is fine.
                    tokio::time::sleep(interval).await;
                    continue;
                };
                match op {
                    ReplicaOp::Upload(path) => {
                        match replicate_with_retry(&hot, &durable, &path).await {
                            Some(bytes) => {
                                consecutive_failures = 0;
                                // Uploads (and ONLY uploads) notify the usage recorder / manifest.
                                on_durable(path, bytes);
                            }
                            None => {
                                failed_attempts.fetch_add(1, Ordering::Relaxed);
                                eprintln!(
                                    "photon-storage: warning: replicator exhausted {MAX_ATTEMPTS} \
                                     upload attempts for {path:?}, re-enqueueing (durable store may \
                                     be down)"
                                );
                                queue
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .push_back(ReplicaOp::Upload(path));
                                consecutive_failures = consecutive_failures.saturating_add(1);
                                tokio::time::sleep(pass_failure_backoff(consecutive_failures))
                                    .await;
                            }
                        }
                    }
                    ReplicaOp::Delete(path) => {
                        if delete_with_retry(&durable, &path).await {
                            // Success — including a durable `NotFound` (the object may never have
                            // been replicated, or was already deleted). Deletes never call
                            // `on_durable`. A success resets the escalating pass-failure backoff,
                            // exactly like an upload.
                            consecutive_failures = 0;
                        } else {
                            // A real (non-`NotFound`) durable failure exhausted its retries:
                            // re-enqueue (never drop) so a transient durable outage cannot silently
                            // skip retention. Bounded by the SAME escalating pass-failure backoff +
                            // `failed_attempts` counter as an upload.
                            failed_attempts.fetch_add(1, Ordering::Relaxed);
                            eprintln!(
                                "photon-storage: warning: replicator exhausted {MAX_ATTEMPTS} \
                                 delete attempts for {path:?}, re-enqueueing (durable store may be \
                                 down)"
                            );
                            queue
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push_back(ReplicaOp::Delete(path));
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            tokio::time::sleep(pass_failure_backoff(consecutive_failures)).await;
                        }
                    }
                }
            }
        })
    }
}

/// Escalating delay to apply after `consecutive_failures` consecutive re-enqueue events
/// (i.e. items that exhausted `MAX_ATTEMPTS` back-to-back with no successful upload in
/// between). Doubles per additional failure, bounded at `PASS_FAILURE_BACKOFF_MAX` — this is
/// what keeps a permanently-failing item from busy-looping the drain once it's re-enqueued.
fn pass_failure_backoff(consecutive_failures: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(16);
    PASS_FAILURE_BACKOFF_BASE
        .saturating_mul(1u32 << exponent)
        .min(PASS_FAILURE_BACKOFF_MAX)
}

/// Copy one object from `hot` to `durable`, retrying with exponential backoff.
/// Returns `Some(uploaded byte length)` on success, `None` if all attempts were exhausted.
async fn replicate_with_retry(
    hot: &Arc<dyn ObjectStore>,
    durable: &Arc<dyn ObjectStore>,
    path: &str,
) -> Option<u64> {
    for attempt in 0..MAX_ATTEMPTS {
        match replicate_once(hot, durable, path).await {
            Ok(len) => return Some(len),
            Err(_) if attempt + 1 < MAX_ATTEMPTS => {
                let delay = BASE_BACKOFF * 2u32.pow(attempt);
                tokio::time::sleep(delay).await;
            }
            Err(_) => return None,
        }
    }
    None
}

async fn replicate_once(
    hot: &Arc<dyn ObjectStore>,
    durable: &Arc<dyn ObjectStore>,
    path: &str,
) -> Result<u64, object_store::Error> {
    let object_path = ObjectPath::from(path);
    let bytes: Bytes = hot.get(&object_path).await?.bytes().await?;
    let len = bytes.len() as u64;
    durable.put(&object_path, PutPayload::from(bytes)).await?;
    Ok(len)
}

/// Delete one object from `durable`, retrying non-`NotFound` failures with the same exponential
/// backoff as [`replicate_with_retry`]. A `NotFound` is treated as SUCCESS — the object may never
/// have been replicated, or was already deleted — and returns immediately without counting a
/// failure. Returns `true` on success (incl. `NotFound`), `false` only if a real error exhausted
/// all `MAX_ATTEMPTS`. Touches durable only; the hot object is already gone (the compactor deleted
/// it before enqueueing this).
async fn delete_with_retry(durable: &Arc<dyn ObjectStore>, path: &str) -> bool {
    let object_path = ObjectPath::from(path);
    for attempt in 0..MAX_ATTEMPTS {
        match durable.delete(&object_path).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => return true,
            Err(_) if attempt + 1 < MAX_ATTEMPTS => {
                let delay = BASE_BACKOFF * 2u32.pow(attempt);
                tokio::time::sleep(delay).await;
            }
            Err(_) => return false,
        }
    }
    false
}

/// Small drain interval used by the tests: the empty-queue idle must be short so a drained
/// loop re-checks the queue promptly without slowing the suite. Production passes a 2s const.
#[cfg(test)]
const TEST_INTERVAL: Duration = Duration::from_millis(5);

/// Poll `cond` until it returns true, or panic after a ~2s deadline. The drain loop is now
/// long-lived (it never completes), so tests can't `handle.await` it — they observe the
/// externally-visible effects (pending depth, the `on_durable` callback, durable contents)
/// and then `handle.abort()`.
#[cfg(test)]
async fn wait_until<F: FnMut() -> bool>(mut cond: F) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !cond() {
        assert!(
            std::time::Instant::now() < deadline,
            "wait_until: condition not met within deadline"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use object_store::memory::InMemory;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn storage_with_durable() -> Storage {
        Storage {
            hot: Arc::new(InMemory::new()),
            durable: Some(Arc::new(InMemory::new())),
            hot_dir: None,
        }
    }

    fn storage_without_durable() -> Storage {
        Storage {
            hot: Arc::new(InMemory::new()),
            durable: None,
            hot_dir: None,
        }
    }

    #[tokio::test]
    async fn replicates_enqueued_blob_and_reports_pending() {
        let storage = storage_with_durable();
        let path = "data/seg-1.parquet";

        storage
            .hot
            .put(
                &ObjectPath::from(path),
                PutPayload::from(Bytes::from_static(b"payload bytes")),
            )
            .await
            .unwrap();

        let durable = storage.durable.clone().unwrap();

        let replicator = Replicator::new(storage);
        replicator.enqueue(path.to_string());
        assert_eq!(replicator.pending(), 1);

        let observer = replicator.clone();

        let notified: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let notified_clone = notified.clone();

        let handle = replicator.spawn_drain_loop(TEST_INTERVAL, move |p, _bytes| {
            notified_clone.lock().unwrap().push(p);
        });

        // The loop is long-lived, so it never completes — wait for the upload to be observed
        // (on_durable fires only after the durable `put` succeeds) instead of awaiting the task.
        wait_until(|| !notified.lock().unwrap().is_empty()).await;

        assert_eq!(observer.pending(), 0);
        assert!(
            !handle.is_finished(),
            "drain loop must stay alive after draining, not exit"
        );
        assert_eq!(notified.lock().unwrap().as_slice(), [path.to_string()]);

        let got = durable
            .get(&ObjectPath::from(path))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(&got[..], b"payload bytes");

        handle.abort();
    }

    /// The new drain loop is a **single, long-lived** task: it drains a whole backlog one
    /// object at a time (bounding in-flight uploads — and thus peak whole-file-buffer RAM — to
    /// exactly one), then idles WITHOUT exiting and later picks up objects enqueued after the
    /// backlog was cleared. This is the behavior the old drain-then-exit `spawn` lacked.
    #[tokio::test]
    async fn drain_loop_is_long_lived_and_bounds_in_flight() {
        let storage = storage_with_durable();
        let hot = storage.hot.clone();
        let durable = storage.durable.clone().unwrap();

        // Seed and enqueue a deep backlog up front.
        let n = 20usize;
        for i in 0..n {
            hot.put(
                &ObjectPath::from(format!("data/seg-{i}.parquet").as_str()),
                PutPayload::from(Bytes::from(format!("payload-{i}"))),
            )
            .await
            .unwrap();
        }

        let replicator = Replicator::new(storage);
        for i in 0..n {
            replicator.enqueue(format!("data/seg-{i}.parquet"));
        }
        let observer = replicator.clone();

        let uploaded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = uploaded.clone();
        let handle = replicator.spawn_drain_loop(TEST_INTERVAL, move |p, _b| {
            sink.lock().unwrap().push(p);
        });

        // The one sequential loop drains the entire backlog (in-flight bounded to 1 by
        // construction: each upload is fully awaited before the next `pop_front`).
        wait_until(|| uploaded.lock().unwrap().len() >= n).await;
        assert_eq!(observer.pending(), 0);
        for i in 0..n {
            assert!(
                durable
                    .get(&ObjectPath::from(format!("data/seg-{i}.parquet").as_str()))
                    .await
                    .is_ok(),
                "seg-{i} must be replicated to durable"
            );
        }

        // Backlog drained, yet the loop must NOT have exited — it idles and then picks up an
        // object enqueued afterwards (drain-then-exit could never do this).
        assert!(
            !handle.is_finished(),
            "drain loop must stay alive after draining the backlog"
        );
        hot.put(
            &ObjectPath::from("data/late.parquet"),
            PutPayload::from_static(b"late payload"),
        )
        .await
        .unwrap();
        observer.enqueue("data/late.parquet".to_string());
        wait_until(|| {
            uploaded
                .lock()
                .unwrap()
                .iter()
                .any(|p| p == "data/late.parquet")
        })
        .await;

        assert!(!handle.is_finished());
        handle.abort();
    }

    #[tokio::test]
    async fn enqueue_is_noop_when_durable_is_none() {
        let storage = storage_without_durable();
        let replicator = Replicator::new(storage);

        replicator.enqueue("data/seg-1.parquet".to_string());
        assert_eq!(replicator.pending(), 0);

        let called = Arc::new(AtomicUsize::new(0));
        let called_clone = called.clone();
        // With no durable tier the drain loop early-returns (no loop), so the task actually
        // completes and we can await it to completion here.
        let handle = replicator.spawn_drain_loop(TEST_INTERVAL, move |_, _| {
            called_clone.fetch_add(1, Ordering::SeqCst);
        });
        handle.await.unwrap();

        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    /// A panic while holding the queue mutex (e.g. inside `enqueue`/`pending`/the drain loop's
    /// pop/push, however unlikely) poisons it. Since the `VecDeque<String>` payload is just
    /// paths, a panic mid-operation can't leave it logically corrupt, so recovering the guard
    /// with `.unwrap_or_else(|p| p.into_inner())` is correct — this proves `enqueue`/`pending`
    /// keep working afterwards instead of the old `.expect(...)` propagating the poison forever
    /// (which would silently and permanently kill the owning compactor's replication).
    #[tokio::test]
    async fn queue_operations_recover_after_poisoned_mutex() {
        let storage = storage_with_durable();
        let replicator = Replicator::new(storage);
        replicator.enqueue("data/seg-1.parquet".to_string());
        assert_eq!(replicator.pending(), 1);

        // Poison the queue mutex deterministically: lock it on a separate (non-async) thread and
        // panic while still holding the guard.
        let queue = replicator.queue.clone();
        let poisoner = std::thread::spawn(move || {
            let _guard = queue.lock().unwrap();
            panic!("intentionally poisoning the replicator queue mutex for this test");
        });
        // The thread panicked, so `join` returns `Err`; we only need the mutex to now be
        // poisoned, not the panic payload.
        assert!(poisoner.join().is_err());

        // Both accessors must recover the poisoned guard instead of panicking.
        assert_eq!(replicator.pending(), 1);
        replicator.enqueue("data/seg-2.parquet".to_string());
        assert_eq!(replicator.pending(), 2);
    }

    #[tokio::test]
    async fn enqueue_delete_removes_object_from_durable() {
        let storage = storage_with_durable();
        let durable = storage.durable.clone().unwrap();
        let path = "data/seg-7.parquet";

        // Seed the object directly into durable, as if a prior upload had replicated it.
        durable
            .put(&ObjectPath::from(path), PutPayload::from_static(b"stale"))
            .await
            .unwrap();
        assert!(durable.get(&ObjectPath::from(path)).await.is_ok());

        let replicator = Replicator::new(storage);
        replicator.enqueue_delete(path.to_string());
        assert_eq!(replicator.pending(), 1);

        let observer = replicator.clone();
        // A delete must NEVER fire `on_durable`; the closure panics if it does.
        let handle = replicator.spawn_drain_loop(TEST_INTERVAL, |_, _| {
            panic!("on_durable must not fire for a delete op");
        });

        // Observe the object gone from durable (pending() alone would flip to 0 the instant the op
        // is popped, before the delete completes — so poll the durable store directly instead).
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if durable.get(&ObjectPath::from(path)).await.is_err() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "durable object was not deleted within the deadline"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        assert_eq!(observer.pending(), 0);
        assert_eq!(observer.failed_attempts(), 0);
        assert!(
            !handle.is_finished(),
            "drain loop must stay alive after a delete"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn delete_of_missing_durable_object_is_success_not_reenqueued() {
        // A `NotFound` at delete time is success — the object may never have been replicated (a
        // backed-up replicator whose upload was dropped when the compactor deleted the hot copy),
        // or was already deleted. It must NOT be counted as a failure or re-enqueued forever.
        // `LocalFileSystem::delete` of a missing path returns `Error::NotFound` deterministically,
        // so this genuinely exercises the `NotFound` arm (unlike some stores that return `Ok`).
        use object_store::local::LocalFileSystem;
        let dir = tempfile::tempdir().unwrap();
        let durable: Arc<dyn ObjectStore> =
            Arc::new(LocalFileSystem::new_with_prefix(dir.path()).unwrap());
        let storage = Storage {
            hot: Arc::new(InMemory::new()),
            durable: Some(durable),
            hot_dir: None,
        };

        let replicator = Replicator::new(storage);
        replicator.enqueue_delete("data/never-existed.parquet".to_string());
        assert_eq!(replicator.pending(), 1);

        let observer = replicator.clone();
        let handle = replicator.spawn_drain_loop(TEST_INTERVAL, |_, _| {
            panic!("on_durable must not fire for a delete op");
        });

        // Wait for the loop to pop and process the delete.
        wait_until(|| observer.pending() == 0).await;
        // Give the loop several idle/backoff cycles: had `NotFound` been mishandled as a failure,
        // the op would already be re-enqueued (pending back to 1, failed_attempts bumped) — the
        // re-enqueue happens BEFORE the backoff sleep, so it is observable well within this window.
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(
            observer.pending(),
            0,
            "a NotFound delete must not be re-enqueued"
        );
        assert_eq!(
            observer.failed_attempts(),
            0,
            "NotFound is success, never a counted failure"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn uploads_and_deletes_drain_through_one_loop() {
        // Uploads still work unchanged alongside deletes: one object is uploaded (hot->durable,
        // firing `on_durable`) and another is deleted from durable, both through the single FIFO
        // loop — proving deletes don't regress the upload path and that `on_durable` fires ONLY for
        // the upload.
        let storage = storage_with_durable();
        let hot = storage.hot.clone();
        let durable = storage.durable.clone().unwrap();

        hot.put(
            &ObjectPath::from("data/up.parquet"),
            PutPayload::from_static(b"up"),
        )
        .await
        .unwrap();
        durable
            .put(
                &ObjectPath::from("data/del.parquet"),
                PutPayload::from_static(b"del"),
            )
            .await
            .unwrap();

        let replicator = Replicator::new(storage);
        replicator.enqueue("data/up.parquet".to_string());
        replicator.enqueue_delete("data/del.parquet".to_string());

        let uploaded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = uploaded.clone();
        let observer = replicator.clone();
        let handle =
            replicator.spawn_drain_loop(TEST_INTERVAL, move |p, _b| sink.lock().unwrap().push(p));

        // Upload notifies via `on_durable`.
        wait_until(|| !uploaded.lock().unwrap().is_empty()).await;
        // Delete removes its target from durable.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if durable
                .get(&ObjectPath::from("data/del.parquet"))
                .await
                .is_err()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "delete target was not removed within the deadline"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // The upload landed in durable; only the upload was notified (delete never fires on_durable).
        assert!(durable
            .get(&ObjectPath::from("data/up.parquet"))
            .await
            .is_ok());
        assert_eq!(
            uploaded.lock().unwrap().as_slice(),
            ["data/up.parquet".to_string()]
        );
        assert_eq!(observer.failed_attempts(), 0);
        handle.abort();
    }

    #[tokio::test]
    async fn enqueue_delete_is_noop_when_durable_is_none() {
        let replicator = Replicator::new(storage_without_durable());
        replicator.enqueue_delete("data/seg-1.parquet".to_string());
        assert_eq!(replicator.pending(), 0);
    }
}

#[cfg(test)]
mod durable_hook_tests {
    use super::*;
    use crate::Storage;
    use object_store::{memory::InMemory, ObjectStore, PutPayload};
    use std::sync::{Arc, Mutex};

    fn mem_storage(with_durable: bool) -> Storage {
        Storage {
            hot: Arc::new(InMemory::new()),
            durable: with_durable.then(|| Arc::new(InMemory::new()) as Arc<dyn ObjectStore>),
            hot_dir: None,
        }
    }

    #[tokio::test]
    async fn on_durable_receives_path_and_byte_size() {
        let storage = mem_storage(true);
        // Put a known 5-byte object into the hot store.
        storage
            .hot
            .put(
                &"data/seg-1.parquet".into(),
                PutPayload::from_static(b"hello"),
            )
            .await
            .unwrap();
        let repl = Replicator::new(storage);
        assert!(repl.durable_configured());
        repl.enqueue("data/seg-1.parquet".to_string());

        let seen: Arc<Mutex<Vec<(String, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        // Long-lived loop: observe the callback then abort, rather than awaiting to completion.
        let handle = repl.spawn_drain_loop(TEST_INTERVAL, move |path, bytes| {
            sink.lock().unwrap().push((path, bytes))
        });
        wait_until(|| !seen.lock().unwrap().is_empty()).await;
        handle.abort();

        let got = seen.lock().unwrap().clone();
        assert_eq!(got, vec![("data/seg-1.parquet".to_string(), 5)]);
    }

    #[test]
    fn durable_configured_false_without_durable() {
        assert!(!Replicator::new(mem_storage(false)).durable_configured());
    }
}

#[cfg(test)]
mod retry_exhaustion_tests {
    use super::*;
    use crate::Storage;
    use object_store::local::LocalFileSystem;
    use object_store::memory::InMemory;

    /// A durable store where every `put` underneath `data/` fails deterministically: `data`
    /// exists as a plain *file* (not a directory) at the store's root, so
    /// `LocalFileSystem` can never create `data/<key>` beneath it — regardless of file
    /// permissions or which user runs the test (this also fails for root, since it's a
    /// filesystem structural conflict, not a permission check). This exercises a
    /// permanently-failing durable store without a hand-rolled `ObjectStore` fake, which
    /// would need the `async-trait` crate that photon-storage doesn't depend on.
    fn permanently_failing_durable() -> (tempfile::TempDir, Arc<dyn ObjectStore>) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("data"), b"not a directory").expect("seed blocker file");
        let store: Arc<dyn ObjectStore> =
            Arc::new(LocalFileSystem::new_with_prefix(dir.path()).expect("local fs durable"));
        (dir, store)
    }

    #[tokio::test]
    async fn exhausted_item_is_reenqueued_and_counted_not_dropped() {
        let (_dir, durable) = permanently_failing_durable();
        let hot: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let path = "data/seg-1.parquet";
        hot.put(
            &ObjectPath::from(path),
            PutPayload::from_static(b"payload bytes"),
        )
        .await
        .unwrap();

        let storage = Storage {
            hot,
            durable: Some(durable),
            hot_dir: None,
        };
        let replicator = Replicator::new(storage);
        replicator.enqueue(path.to_string());
        assert_eq!(replicator.failed_attempts(), 0);

        let observer = replicator.clone();
        // The item is re-enqueued on every exhaustion, so the queue is never empty and the
        // empty-queue idle (TEST_INTERVAL) is never reached — the escalating pass-failure
        // backoff is what gates re-attempts here, so the timing window below is unaffected.
        let handle = replicator.spawn_drain_loop(TEST_INTERVAL, |_, _| {
            panic!("on_durable must not fire: every put() on this durable store fails");
        });

        // One retry cycle for a single item is MAX_ATTEMPTS-1 backoff sleeps starting at
        // BASE_BACKOFF and doubling (~150ms total here). After that first exhaustion, the
        // item sits re-enqueued in the queue for a full PASS_FAILURE_BACKOFF_BASE (500ms)
        // before the drain loop pops it again — so checking at 400ms lands inside that
        // window: comfortably after the first exhaustion, comfortably before the second
        // retry cycle starts (which would pop the item back out of the queue).
        tokio::time::sleep(Duration::from_millis(400)).await;
        handle.abort();

        assert_eq!(
            observer.pending(),
            1,
            "item must be re-enqueued after exhausting retries, not dropped"
        );
        assert_eq!(
            observer.failed_attempts(),
            1,
            "exhaustion must be counted exactly once in this window: a second exhaustion \
             this soon would mean the escalating pass-failure backoff isn't gating the \
             re-enqueued item, i.e. the drain loop is busy-looping it"
        );
    }
}

//! `Replicator`: background hot -> durable object copier with retry/backoff.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, PutPayload};

use crate::storage::Storage;

/// Number of attempts (including the first) made to replicate a single object before it
/// is dropped from the queue.
const MAX_ATTEMPTS: u32 = 5;
/// Base delay for exponential backoff between retries.
const BASE_BACKOFF: Duration = Duration::from_millis(10);

/// Background hot->durable replicator. Objects enqueued via [`Replicator::enqueue`] are
/// copied from `storage.hot` to `storage.durable` by the drain loop started with
/// [`Replicator::spawn`].
#[derive(Clone)]
pub struct Replicator {
    storage: Storage,
    queue: Arc<Mutex<VecDeque<String>>>,
}

impl Replicator {
    pub fn new(storage: Storage) -> Replicator {
        Replicator {
            storage,
            queue: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Enqueue a hot object path for replication. No-op if `durable` is `None`.
    pub fn enqueue(&self, path: String) {
        if self.storage.durable.is_none() {
            return;
        }
        self.queue
            .lock()
            .expect("replicator queue mutex poisoned")
            .push_back(path);
    }

    /// Current queue depth (exposed as the replication-lag metric).
    pub fn pending(&self) -> usize {
        self.queue
            .lock()
            .expect("replicator queue mutex poisoned")
            .len()
    }

    /// Whether a durable (S3/Garage) replica is configured.
    pub fn durable_configured(&self) -> bool {
        self.storage.durable.is_some()
    }

    /// Spawn the background drain loop; returns a handle. `on_durable(path, bytes)` is
    /// called after each successful upload (with the uploaded object's byte length) so
    /// the caller can update the manifest.
    ///
    /// Drains whatever is queued at the time of (and enqueued during) the drain; the
    /// task completes once the queue is empty.
    pub fn spawn<F>(self, on_durable: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(String, u64) + Send + 'static,
    {
        let Replicator { storage, queue } = self;
        tokio::spawn(async move {
            let Some(durable) = storage.durable.clone() else {
                return;
            };
            let hot = storage.hot.clone();
            loop {
                let next = {
                    let mut q = queue.lock().expect("replicator queue mutex poisoned");
                    q.pop_front()
                };
                let Some(path) = next else {
                    break;
                };
                if let Some(bytes) = replicate_with_retry(&hot, &durable, &path).await {
                    on_durable(path, bytes);
                }
            }
        })
    }
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

        let pending_check = replicator.clone();

        let notified: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let notified_clone = notified.clone();

        let handle = replicator.spawn(move |p, _bytes| {
            notified_clone.lock().unwrap().push(p);
        });
        handle.await.unwrap();

        assert_eq!(pending_check.pending(), 0);
        assert_eq!(notified.lock().unwrap().as_slice(), [path.to_string()]);

        let got = durable
            .get(&ObjectPath::from(path))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(&got[..], b"payload bytes");
    }

    #[tokio::test]
    async fn enqueue_is_noop_when_durable_is_none() {
        let storage = storage_without_durable();
        let replicator = Replicator::new(storage);

        replicator.enqueue("data/seg-1.parquet".to_string());
        assert_eq!(replicator.pending(), 0);

        let called = Arc::new(AtomicUsize::new(0));
        let called_clone = called.clone();
        let handle = replicator.spawn(move |_, _| {
            called_clone.fetch_add(1, Ordering::SeqCst);
        });
        handle.await.unwrap();

        assert_eq!(called.load(Ordering::SeqCst), 0);
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
        repl.spawn(move |path, bytes| sink.lock().unwrap().push((path, bytes)))
            .await
            .unwrap();

        let got = seen.lock().unwrap().clone();
        assert_eq!(got, vec![("data/seg-1.parquet".to_string(), 5)]);
    }

    #[test]
    fn durable_configured_false_without_durable() {
        assert!(!Replicator::new(mem_storage(false)).durable_configured());
    }
}

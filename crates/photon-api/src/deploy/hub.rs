//! The in-memory side of the control channel. Everything here is rebuilt from nothing on restart
//! and that is fine: a wake-up missed becomes the next poll, a stream session dropped is reopened
//! by the next UI read, a bootstrap job lost is one the operator re-runs.

use photon_deploy::model::*;
use photon_deploy::proto::{StreamChunk, StreamKind, StreamSpec};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// Container stats kept per (host, container id), fed by every sync's observed inventory and,
/// while a stats session is open, by the agent's 2-second stream.
const STAT_RING: usize = 4 * 60 * 6; // 24h at 1/min, or ~1h at 2s
const EVENT_RING: usize = 500;
const LOG_RING: usize = 5_000;
/// A stream nobody has read for this long is closed (the agent stops uploading on its next sync).
pub const STREAM_IDLE_MS: i64 = 30_000;
/// An exec session nobody typed into for this long is closed and audited.
pub const EXEC_IDLE_MS: i64 = 10 * 60_000;

#[derive(Debug, Clone, Default)]
pub struct StatSample {
    pub t: i64,
    pub cpu_pct: Option<f64>,
    pub mem_bytes: Option<u64>,
    pub mem_limit_bytes: Option<u64>,
    pub net_rx_bps: Option<f64>,
    pub net_tx_bps: Option<f64>,
    pub block_read_bps: Option<f64>,
    pub block_write_bps: Option<f64>,
}

pub struct StreamSession {
    pub id: String,
    pub host: String,
    pub kind: StreamKind,
    pub container_id: String,
    pub container_name: String,
    pub spec: StreamSpec,
    pub opened_ms: i64,
    pub last_read_ms: i64,
    /// Log lines / exec output, newest last.
    pub buffer: VecDeque<StreamChunk>,
    /// Sequence number of the first element in `buffer` (so a reader can ask "since seq N").
    pub base_seq: u64,
    pub closed: bool,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    /// Exec: input lines the agent has not fetched yet.
    pub input: VecDeque<String>,
    pub input_notify: Arc<Notify>,
    pub output_notify: Arc<Notify>,
    pub commands: u32,
    /// Exec: who opened it (for the audit row at close).
    pub opened_by: Option<(String, Role)>,
    /// Set once the agent has seen this session in a sync response (so a 404 on its upload means
    /// "closed", never "not yet advertised").
    pub advertised: bool,
    /// Exec: the sequence number up to which output has been handed to the UI, so a chunk that
    /// lands between two reads is delivered by the next one instead of skipped.
    pub delivered_seq: u64,
}

impl StreamSession {
    pub fn push(&mut self, chunk: StreamChunk) {
        if let StreamChunk::End { exit_code, error } = &chunk {
            self.closed = true;
            self.exit_code = *exit_code;
            self.error = error.clone();
        }
        self.buffer.push_back(chunk);
        while self.buffer.len() > LOG_RING {
            self.buffer.pop_front();
            self.base_seq += 1;
        }
        self.output_notify.notify_one();
    }
    pub fn next_seq(&self) -> u64 {
        self.base_seq + self.buffer.len() as u64
    }
    /// Chunks at or after `seq`.
    pub fn since(&self, seq: u64) -> Vec<StreamChunk> {
        let skip = seq.saturating_sub(self.base_seq) as usize;
        self.buffer.iter().skip(skip).cloned().collect()
    }
}

#[derive(Default)]
struct HubInner {
    streams: HashMap<String, StreamSession>,
    events: HashMap<String, VecDeque<DockerEvent>>,
    stats: HashMap<(String, String), VecDeque<StatSample>>,
    jobs: HashMap<String, BootstrapJob>,
    /// Exec sessions the UI has closed but whose audit row is still owed.
    wakers: HashMap<String, Arc<Notify>>,
}

#[derive(Default)]
pub struct DeployHub {
    inner: Mutex<HubInner>,
}

impl DeployHub {
    pub fn new() -> Self {
        Self::default()
    }

    fn waker(&self, host: &str) -> Arc<Notify> {
        let mut g = self.inner.lock().unwrap();
        g.wakers
            .entry(host.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    /// Wake a host's pending long-poll: new desired state, an action, a stream.
    /// Wake `host`'s sync long-poll. `notify_one`, not `notify_waiters`: it stores a permit
    /// when nobody is waiting, so a wake that fires while the agent is between two polls is
    /// honoured by its next one instead of lost — every waiter re-checks for work before it
    /// waits again, so a stale permit costs one extra look, never a wrong answer.
    pub fn wake(&self, host: &str) {
        self.waker(host).notify_one();
    }

    /// Wait up to `ms` for a wake-up on `host`.
    pub async fn wait(&self, host: &str, ms: u64) {
        let n = self.waker(host);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(ms), n.notified()).await;
    }

    // --- events ---

    pub fn push_events(&self, host: &str, events: Vec<DockerEvent>) {
        let mut g = self.inner.lock().unwrap();
        let ring = g.events.entry(host.to_string()).or_default();
        for e in events {
            ring.push_back(e);
            while ring.len() > EVENT_RING {
                ring.pop_front();
            }
        }
    }

    pub fn events(&self, host: &str, limit: usize) -> Vec<DockerEvent> {
        let g = self.inner.lock().unwrap();
        g.events
            .get(host)
            .map(|r| r.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    // --- stats ---

    pub fn push_stat(&self, host: &str, container_id: &str, s: StatSample) {
        let mut g = self.inner.lock().unwrap();
        let ring = g
            .stats
            .entry((host.to_string(), container_id.to_string()))
            .or_default();
        if let Some(last) = ring.back() {
            if s.t <= last.t {
                return;
            }
        }
        ring.push_back(s);
        while ring.len() > STAT_RING {
            ring.pop_front();
        }
    }

    pub fn stats(
        &self,
        host: &str,
        container_id: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<StatSample> {
        let g = self.inner.lock().unwrap();
        g.stats
            .get(&(host.to_string(), container_id.to_string()))
            .map(|r| {
                r.iter()
                    .filter(|s| s.t >= from_ms && s.t <= to_ms)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Drop the rings of containers that no longer exist on a host.
    pub fn retain_containers(&self, host: &str, ids: &[String]) {
        let mut g = self.inner.lock().unwrap();
        g.stats.retain(|(h, id), _| h != host || ids.contains(id));
    }

    // --- streams ---

    /// Find the open session of this kind for a container, or create one.
    #[allow(clippy::too_many_arguments)]
    pub fn open_stream(
        &self,
        host: &str,
        kind: StreamKind,
        container_id: &str,
        container_name: &str,
        spec: StreamSpec,
        opened_by: Option<(String, Role)>,
        now_ms: i64,
    ) -> (String, bool) {
        let mut g = self.inner.lock().unwrap();
        if kind != StreamKind::Exec {
            if let Some(s) = g.streams.values_mut().find(|s| {
                s.host == host && s.kind == kind && s.container_id == container_id && !s.closed
            }) {
                s.last_read_ms = now_ms;
                return (s.id.clone(), false);
            }
        }
        let id = photon_deploy::ids::new_id("ss");
        let mut spec = spec;
        spec.session = id.clone();
        g.streams.insert(
            id.clone(),
            StreamSession {
                id: id.clone(),
                host: host.to_string(),
                kind,
                container_id: container_id.to_string(),
                container_name: container_name.to_string(),
                spec,
                opened_ms: now_ms,
                last_read_ms: now_ms,
                buffer: VecDeque::new(),
                base_seq: 0,
                closed: false,
                exit_code: None,
                error: None,
                input: VecDeque::new(),
                input_notify: Arc::new(Notify::new()),
                output_notify: Arc::new(Notify::new()),
                commands: 0,
                opened_by,
                advertised: false,
                delivered_seq: 0,
            },
        );
        (id, true)
    }

    pub fn with_stream<T>(&self, id: &str, f: impl FnOnce(&mut StreamSession) -> T) -> Option<T> {
        let mut g = self.inner.lock().unwrap();
        g.streams.get_mut(id).map(f)
    }

    pub fn stream_exists(&self, id: &str) -> bool {
        self.inner.lock().unwrap().streams.contains_key(id)
    }

    /// The streams the agent for `host` should be running right now; marks them advertised.
    pub fn streams_for(&self, host: &str) -> Vec<StreamSpec> {
        let mut g = self.inner.lock().unwrap();
        g.streams
            .values_mut()
            .filter(|s| s.host == host && !s.closed)
            .map(|s| {
                s.advertised = true;
                s.spec.clone()
            })
            .collect()
    }

    pub fn has_open_streams(&self, host: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .streams
            .values()
            .any(|s| s.host == host && !s.closed)
    }

    /// Close a session from the UI side; returns what the audit row needs, if it was an exec.
    pub fn close_stream(&self, id: &str) -> Option<StreamSession> {
        let mut g = self.inner.lock().unwrap();
        if let Some(s) = g.streams.get_mut(id) {
            s.closed = true;
            s.input_notify.notify_one();
            s.output_notify.notify_one();
        }
        g.streams.remove(id)
    }

    /// Sessions idle past their limit → removed; exec ones are returned so the caller can audit.
    pub fn expire_streams(&self, now_ms: i64) -> Vec<StreamSession> {
        let mut g = self.inner.lock().unwrap();
        let dead: Vec<String> = g
            .streams
            .values()
            .filter(|s| {
                let idle = if s.kind == StreamKind::Exec {
                    EXEC_IDLE_MS
                } else {
                    STREAM_IDLE_MS
                };
                now_ms - s.last_read_ms > idle || (s.closed && now_ms - s.last_read_ms > 5_000)
            })
            .map(|s| s.id.clone())
            .collect();
        let mut out = Vec::new();
        for id in dead {
            if let Some(s) = g.streams.remove(&id) {
                s.input_notify.notify_one();
                s.output_notify.notify_one();
                out.push(s);
            }
        }
        out
    }

    // --- bootstrap jobs ---

    pub fn put_job(&self, job: BootstrapJob) {
        self.inner.lock().unwrap().jobs.insert(job.id.clone(), job);
    }

    pub fn job(&self, id: &str) -> Option<BootstrapJob> {
        self.inner.lock().unwrap().jobs.get(id).cloned()
    }

    pub fn update_job(&self, id: &str, f: impl FnOnce(&mut BootstrapJob)) {
        let mut g = self.inner.lock().unwrap();
        if let Some(j) = g.jobs.get_mut(id) {
            f(j);
        }
    }

    /// Jobs finished more than an hour ago are forgotten.
    pub fn expire_jobs(&self, now_ms: i64) {
        let mut g = self.inner.lock().unwrap();
        g.jobs
            .retain(|_, j| j.status == JobStatus::Running || now_ms - j.started_ms < 3_600_000);
    }
}

/// The containers a stream may target must exist in the host's observed inventory; this is how
/// a handler names one (id or name) and learns its display name.
pub fn find_container<'a>(containers: &'a [Container], id_or_name: &str) -> Option<&'a Container> {
    containers.iter().find(|c| {
        c.id == id_or_name
            || c.name == id_or_name
            || (id_or_name.len() >= 12 && c.id.starts_with(id_or_name))
    })
}

/// Points for the UI's stats chart from the ring.
pub fn stat_series(samples: &[StatSample]) -> ContainerStats {
    let pick = |f: &dyn Fn(&StatSample) -> Option<f64>| -> Vec<StatPoint> {
        samples
            .iter()
            .map(|s| StatPoint { t: s.t, v: f(s) })
            .collect()
    };
    ContainerStats {
        cpu_pct: pick(&|s| s.cpu_pct),
        mem_bytes: pick(&|s| s.mem_bytes.map(|v| v as f64)),
        net_rx_bps: pick(&|s| s.net_rx_bps),
        net_tx_bps: pick(&|s| s.net_tx_bps),
        block_read_bps: pick(&|s| s.block_read_bps),
        block_write_bps: pick(&|s| s.block_write_bps),
        mem_limit_bytes: samples.iter().rev().find_map(|s| s.mem_limit_bytes),
    }
}

/// Group a host's containers into the compose projects Photon does not manage yet (R4.6).
pub fn discover_projects(
    host: &str,
    containers: &[Container],
    managed_names: &[String],
) -> Vec<DiscoveredProject> {
    let mut by_project: BTreeMap<String, Vec<&Container>> = BTreeMap::new();
    for c in containers {
        if let Some(p) = &c.compose_project {
            if c.managed_stack_id.is_none() && !managed_names.contains(p) {
                by_project.entry(p.clone()).or_default().push(c);
            }
        }
    }
    by_project
        .into_iter()
        .map(|(name, cs)| {
            let mut services: Vec<String> = cs
                .iter()
                .filter_map(|c| c.compose_service.clone())
                .collect();
            services.sort();
            services.dedup();
            DiscoveredProject {
                host: host.to_string(),
                name,
                services,
                containers: cs.len() as u32,
                running: cs
                    .iter()
                    .filter(|c| c.state == ContainerState::Running)
                    .count() as u32,
                compose_file: cs.iter().find_map(|c| {
                    c.labels
                        .get("com.docker.compose.project.config_files")
                        .cloned()
                }),
                working_dir: cs.iter().find_map(|c| {
                    c.labels
                        .get("com.docker.compose.project.working_dir")
                        .cloned()
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_dedupe_per_container_and_expire() {
        let hub = DeployHub::new();
        let spec = StreamSpec {
            session: String::new(),
            kind: StreamKind::Logs,
            container_id: "c1".into(),
            tail: Some(100),
            since_ms: None,
            shell: None,
        };
        let (a, created) = hub.open_stream(
            "h",
            StreamKind::Logs,
            "c1",
            "web",
            spec.clone(),
            None,
            1_000,
        );
        assert!(created);
        let (b, created) = hub.open_stream(
            "h",
            StreamKind::Logs,
            "c1",
            "web",
            spec.clone(),
            None,
            2_000,
        );
        assert!(!created);
        assert_eq!(a, b);
        assert_eq!(hub.streams_for("h").len(), 1);
        hub.with_stream(&a, |s| {
            s.push(StreamChunk::Log {
                ts_ms: 1,
                stream: "stdout".into(),
                text: "hi".into(),
            })
        });
        assert_eq!(hub.with_stream(&a, |s| s.since(0).len()), Some(1));
        assert!(hub.expire_streams(2_000 + STREAM_IDLE_MS - 1).is_empty());
        assert_eq!(hub.expire_streams(2_000 + STREAM_IDLE_MS + 1).len(), 1);
        assert!(!hub.stream_exists(&a));
    }

    #[test]
    fn stat_ring_is_monotonic_and_bounded() {
        let hub = DeployHub::new();
        for t in 0..(STAT_RING as i64 + 10) {
            hub.push_stat(
                "h",
                "c",
                StatSample {
                    t,
                    cpu_pct: Some(1.0),
                    ..Default::default()
                },
            );
        }
        hub.push_stat(
            "h",
            "c",
            StatSample {
                t: 5,
                ..Default::default()
            },
        ); // stale, dropped
        let all = hub.stats("h", "c", 0, i64::MAX);
        assert_eq!(all.len(), STAT_RING);
        assert_eq!(all.last().unwrap().t, STAT_RING as i64 + 9);
        hub.retain_containers("h", &[]);
        assert!(hub.stats("h", "c", 0, i64::MAX).is_empty());
    }
}

//! The control channel (section 7.4): one long-polled `POST /api/agent/sync` carrying what I am,
//! what I see and what I did, answered with what I should be. Outbound only. The loop:
//!
//! 1. build the request (identity, capabilities, observed inventory, drained events, results of
//!    the last round's actions and stack applies, the streams still running);
//! 2. POST it with `wait_secs` so the server can hold the response until it has work;
//! 3. apply the response: skew, ingest hint, upgrade, streams (start new / stop gone), one-shot
//!    actions (seen-set + expiry), desired stacks (only the generations not yet applied);
//! 4. sleep the rest of `next_poll_secs` — or not at all when there are results to report.
//!
//! Everything the server sends passes the local capability gate first (R1.3). A server that is
//! unreachable is logged and retried with backoff; the agent keeps its last state (R1.7).
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use photon_deploy::model::{ActionKind, ActionStatus, AgentCapability};
use photon_deploy::proto::{
    ActionResult, ActionSpec, Desired, HostIdentity, RegistryAuth, RegistryAuthRequest,
    RegistryAuthResponse, SecretsRequest, SecretsResponse, StackResult, SyncRequest, SyncResponse,
    PROTOCOL_VERSION,
};

use crate::clock::{now_ms, Skew};
use crate::config::AgentConfig;
use crate::docker::{Caps, DockerRuntime};
use crate::send::{IngestTarget, SharedTarget};
use crate::state::{resolve_data_dir, LocalState, STATE_FILE};

pub const MAX_BACKOFF: Duration = Duration::from_secs(60);
pub const MIN_BACKOFF: Duration = Duration::from_secs(5);

pub struct SyncClient {
    pub client: reqwest::Client,
    pub server: String,
    pub token: String,
}

impl SyncClient {
    pub fn url(&self, path: &str) -> String {
        format!("{}/api/agent/{}", self.server.trim_end_matches('/'), path)
    }

    pub async fn sync(&self, req: &SyncRequest) -> Result<SyncResponse, SyncError> {
        let wait = Duration::from_secs(req.wait_secs as u64 + 15);
        let resp = self
            .client
            .post(self.url("sync"))
            .bearer_auth(&self.token)
            .timeout(wait)
            .json(req)
            .send()
            .await
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        match resp.status().as_u16() {
            200 => resp
                .json::<SyncResponse>()
                .await
                .map_err(|e| SyncError::Transport(format!("bad sync response: {e}"))),
            401 | 403 => Err(SyncError::Unauthorized),
            s => Err(SyncError::Transport(format!("sync answered {s}"))),
        }
    }

    /// The registry credentials this host may pull with (`servers` empty = all it is scoped
    /// for). A server that predates the route answers 404, which reads as "none" — the pull
    /// then runs exactly as it did before, on whatever the host itself is logged in to.
    pub async fn registry_auth(&self, servers: &[String]) -> Result<Vec<RegistryAuth>, String> {
        let resp = self
            .client
            .post(self.url("registry-auth"))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(30))
            .json(&RegistryAuthRequest {
                servers: servers.to_vec(),
            })
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if resp.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        if !resp.status().is_success() {
            return Err(format!("registry-auth answered {}", resp.status()));
        }
        Ok(resp
            .json::<RegistryAuthResponse>()
            .await
            .map_err(|e| e.to_string())?
            .credentials)
    }

    pub async fn secrets(&self, stack_id: &str) -> Result<BTreeMap<String, String>, String> {
        let resp = self
            .client
            .post(self.url("secrets"))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(30))
            .json(&SecretsRequest {
                stack_id: stack_id.to_string(),
            })
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("secrets answered {}", resp.status()));
        }
        Ok(resp
            .json::<SecretsResponse>()
            .await
            .map_err(|e| e.to_string())?
            .values)
    }
}

#[derive(Debug)]
pub enum SyncError {
    Unauthorized,
    Transport(String),
}

/// Run one action if it is new and not expired. `None` = already done (skip silently).
pub fn action_disposition(
    state: &mut LocalState,
    spec: &ActionSpec,
    now: i64,
) -> Option<Result<(), String>> {
    if state.has_seen(&spec.id) {
        return None;
    }
    state.mark_seen(&spec.id);
    if spec.expires_ms > 0 && spec.expires_ms < now {
        return Some(Err("expired before the agent picked it up".into()));
    }
    Some(Ok(()))
}

/// The main loop. Returns only on a fatal configuration error.
pub async fn run(
    cfg: AgentConfig,
    ingest: SharedTarget,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = cfg
        .server
        .clone()
        .ok_or("--server is required for the control channel")?;
    let token = cfg
        .token
        .clone()
        .ok_or("--token (the agent token) is required with --server")?;
    let client = reqwest::Client::builder().build()?;
    let sync = Arc::new(SyncClient {
        client: client.clone(),
        server: server.clone(),
        token: token.clone(),
    });
    let host = cfg.resolved_host();
    let data_dir = resolve_data_dir(cfg.data_dir.as_deref());
    let state_path = data_dir.join(STATE_FILE);
    let state = Arc::new(Mutex::new(LocalState::load(&state_path)));
    let caps = Caps::from_config(&cfg);
    let runtime = Arc::new(DockerRuntime::new(caps, &cfg.docker_host, &data_dir));
    let skew = Arc::new(Skew::default());
    let identity = HostIdentity {
        name: host.clone(),
        id: crate::sysinfo_sampler::machine_id(),
        arch: std::env::consts::ARCH.to_string(),
        os: std::env::consts::OS.to_string(),
    };
    let poll_secs = cfg.poll.as_secs() as u32;
    eprintln!(
        "photon-agent {}: host {host}, server {server}, capabilities [{}], poll {}s, data {}",
        crate::upgrade::CURRENT_VERSION,
        caps.list()
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        poll_secs,
        data_dir.display()
    );

    let mut pending_results: Vec<ActionResult> = Vec::new();
    let mut pending_stacks: Vec<StackResult> = Vec::new();
    let mut streams: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    // Long work (a stack apply pulling a 2 GB image over a slow link, an image pull action)
    // runs here, off the sync loop: the loop keeps syncing, so the host stays `online`, the UI
    // keeps its logs and shell, and the stack sits in `applying` instead of the host vanishing.
    // Measured before this: a 17-minute pull showed the host offline for 17 minutes.
    let mut jobs: tokio::task::JoinSet<Job> = tokio::task::JoinSet::new();
    let mut busy_stacks: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut backoff = MIN_BACKOFF;

    loop {
        let started = Instant::now();
        // 0. Collect what the background jobs finished; their results ride in this sync.
        let mut did_work = false;
        while let Some(joined) = jobs.try_join_next() {
            match joined {
                Ok(Job::Action(r)) => {
                    pending_results.push(r);
                    did_work = true;
                }
                Ok(Job::Stack { result, name }) => {
                    busy_stacks.remove(&result.stack_id);
                    eprintln!(
                        "photon-agent: stack {} gen {} {:?} → {}",
                        name,
                        result.generation,
                        result.op,
                        if result.ok { "ok" } else { "FAILED" }
                    );
                    let mut st = state.lock().unwrap();
                    if matches!(result.op, photon_deploy::model::StackOp::Destroy) && result.ok {
                        st.forget_stack(&result.stack_id);
                    } else {
                        st.record_stack(&result.stack_id, &name, result.generation);
                    }
                    drop(st);
                    pending_stacks.push(result);
                    did_work = true;
                }
                Err(e) => eprintln!("photon-agent: a background job panicked: {e}"),
            }
        }
        // 1. Observe.
        let observed = if caps.read {
            let st = state.lock().unwrap().clone();
            match runtime.observe(&host, &st).await {
                Ok(o) => Some(o),
                Err(e) => {
                    eprintln!("photon-agent: inventory unavailable: {e}");
                    None
                }
            }
        } else {
            None
        };
        let events = runtime.drain_events().await;
        streams.retain(|_, h| !h.is_finished());
        let known_generation = state.lock().unwrap().known_generation;
        let has_results = !pending_results.is_empty() || !pending_stacks.is_empty();
        let req = SyncRequest {
            protocol: PROTOCOL_VERSION,
            agent_version: crate::upgrade::CURRENT_VERSION.to_string(),
            host: identity.clone(),
            capabilities: caps.list(),
            known_generation,
            observed,
            results: std::mem::take(&mut pending_results),
            stack_results: std::mem::take(&mut pending_stacks),
            events,
            // A short hold while jobs run, so a finished apply is reported within seconds.
            wait_secs: if has_results {
                0
            } else if !jobs.is_empty() {
                JOB_POLL_SECS
            } else {
                poll_secs
            },
            active_streams: streams.keys().cloned().collect(),
        };

        // 2. Sync.
        let sent_at = now_ms();
        let resp = match sync.sync(&req).await {
            Ok(r) => {
                backoff = MIN_BACKOFF;
                r
            }
            Err(SyncError::Unauthorized) => {
                // Results would be lost; put them back so the next successful sync carries them.
                pending_results = req.results;
                pending_stacks = req.stack_results;
                eprintln!("photon-agent: the server rejected the agent token (401) — rotate it in Photon and reinstall; retrying in 30s");
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            Err(SyncError::Transport(e)) => {
                pending_results = req.results;
                pending_stacks = req.stack_results;
                eprintln!(
                    "photon-agent: sync failed: {e}; retrying in {}s",
                    backoff.as_secs()
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        let arrived = now_ms();
        skew.observe(resp.server_time_ms, (sent_at + arrived) / 2);

        // 3a. Ingest hint.
        if cfg.endpoint.is_none() {
            if let Some(h) = &resp.ingest {
                let mut g = ingest.lock().unwrap();
                let t = IngestTarget {
                    endpoint: h.metrics_endpoint.clone(),
                    token: h.token.clone(),
                };
                if g.as_ref() != Some(&t) {
                    eprintln!("photon-agent: metrics go to {}", t.endpoint);
                    *g = Some(t);
                }
            }
        }
        // 3b. Upgrade (never returns on success).
        if let Some(up) = &resp.upgrade {
            if up.version != crate::upgrade::CURRENT_VERSION {
                if let Err(e) = crate::upgrade::apply(&client, &token, up, &data_dir).await {
                    eprintln!("photon-agent: upgrade to {} failed: {e}", up.version);
                }
            }
        }
        // 3c. Streams: start the new, stop the gone.
        let wanted: Vec<String> = resp.streams.iter().map(|s| s.session.clone()).collect();
        for (id, h) in streams.iter() {
            if !wanted.contains(id) {
                h.abort();
            }
        }
        streams.retain(|id, _| wanted.contains(id));
        for spec in &resp.streams {
            if streams.contains_key(&spec.session) {
                continue;
            }
            match start_stream(&runtime, caps, &sync, spec.clone(), skew.clone()).await {
                Ok(h) => {
                    streams.insert(spec.session.clone(), h);
                }
                Err(e) => eprintln!("photon-agent: stream {} refused: {e}", spec.session),
            }
        }
        // 3d. One-shot actions — each on its own task (a pull can take minutes).
        for spec in &resp.actions {
            let disposition = action_disposition(&mut state.lock().unwrap(), spec, skew.now());
            let Some(disp) = disposition else { continue };
            did_work = true;
            let started_ms = skew.now();
            match disp {
                Err(why) => pending_results.push(ActionResult {
                    action_id: spec.id.clone(),
                    status: ActionStatus::Expired,
                    detail: Some(why),
                    started_ms: Some(started_ms),
                    finished_ms: Some(skew.now()),
                }),
                Ok(()) => {
                    let rt = runtime.clone();
                    let spec = spec.clone();
                    let skew = skew.clone();
                    let sync = sync.clone();
                    jobs.spawn(async move {
                        // Only a pulling action asks for credentials; asking narrows to the
                        // image's registry when the spec names the image.
                        let auths = if action_pulls(&spec) {
                            let servers: Vec<String> = spec
                                .image
                                .as_deref()
                                .filter(|i| !i.is_empty())
                                .map(|i| vec![photon_deploy::registry::image_registry(i)])
                                .unwrap_or_default();
                            match sync.registry_auth(&servers).await {
                                Ok(a) => a,
                                Err(e) => {
                                    eprintln!("photon-agent: registry credentials unavailable ({e}); pulling without");
                                    Vec::new()
                                }
                            }
                        } else {
                            Vec::new()
                        };
                        let (status, detail) = match rt.run_action(&spec, &auths).await {
                            Ok(d) => (ActionStatus::Done, d),
                            Err(e) => (ActionStatus::Failed, e),
                        };
                        eprintln!(
                            "photon-agent: action {} {:?} → {:?}: {}",
                            spec.id,
                            spec.kind,
                            status,
                            first_line(&detail)
                        );
                        Job::Action(ActionResult {
                            action_id: spec.id.clone(),
                            status,
                            detail: Some(detail),
                            started_ms: Some(started_ms),
                            finished_ms: Some(skew.now()),
                        })
                    });
                }
            }
        }
        // 3e. Desired stacks — each apply on its own task, one per stack at a time.
        if let Some(desired) = &resp.desired {
            did_work |= start_desired(
                &runtime,
                &sync,
                &state,
                desired,
                &skew,
                &mut jobs,
                &mut busy_stacks,
            );
        }
        if let Err(e) = state.lock().unwrap().save(&state_path) {
            eprintln!("photon-agent: could not save state: {e}");
        }

        // 4. Pace. Results wait for nobody; otherwise honour the server's hint minus the time
        // the long-poll already took.
        if did_work || !pending_results.is_empty() || !pending_stacks.is_empty() {
            continue;
        }
        let mut hint = Duration::from_secs(resp.next_poll_secs.max(1) as u64);
        if !jobs.is_empty() {
            hint = hint.min(Duration::from_secs(JOB_POLL_SECS as u64));
        }
        let spent = started.elapsed();
        if spent < hint {
            tokio::time::sleep((hint - spent).max(Duration::from_secs(1))).await;
        }
    }
}

/// Does this action pull an image? Those are the ones worth a credentials round trip.
fn action_pulls(spec: &ActionSpec) -> bool {
    match spec.kind {
        ActionKind::Pull | ActionKind::ImagePull => true,
        ActionKind::Recreate => spec.args.pull.unwrap_or(false),
        _ => false,
    }
}

fn first_line(s: &str) -> &str {
    s.lines().last().unwrap_or("").trim()
}

async fn start_stream(
    runtime: &DockerRuntime,
    caps: Caps,
    sync: &Arc<SyncClient>,
    spec: photon_deploy::proto::StreamSpec,
    skew: Arc<Skew>,
) -> Result<tokio::task::JoinHandle<()>, String> {
    #[cfg(feature = "docker")]
    {
        let docker = runtime.docker().await?;
        let uplink = crate::docker::streams::Uplink {
            client: sync.client.clone(),
            server: sync.server.clone(),
            token: sync.token.clone(),
        };
        Ok(crate::docker::streams::spawn(
            docker, caps, uplink, spec, skew,
        ))
    }
    #[cfg(not(feature = "docker"))]
    {
        let _ = (runtime, caps, sync, spec, skew);
        Err("built without the docker feature".into())
    }
}

/// Apply every desired stack whose generation the local state has not seen. Returns whether
/// anything was attempted.
/// What a background job hands back to the loop.
enum Job {
    Action(ActionResult),
    Stack { result: StackResult, name: String },
}

/// How often to sync while a job is running, so its result is reported promptly.
const JOB_POLL_SECS: u32 = 2;

/// Start an apply task for every stack in the document that needs one and is not already
/// being applied. The document counts as known only once every stack in it was started: a
/// stack skipped because its previous apply is still running keeps `known_generation` where
/// it was, so the server re-sends the document and the skipped stack is picked up next time.
fn start_desired(
    runtime: &Arc<DockerRuntime>,
    sync: &Arc<SyncClient>,
    state: &Arc<Mutex<LocalState>>,
    desired: &Desired,
    skew: &Arc<Skew>,
    jobs: &mut tokio::task::JoinSet<Job>,
    busy: &mut std::collections::HashSet<String>,
) -> bool {
    let mut started = false;
    let mut skipped = false;
    for stack in &desired.stacks {
        let needs = state
            .lock()
            .unwrap()
            .stack_needs_apply(&stack.id, stack.generation);
        if !needs {
            continue;
        }
        if busy.contains(&stack.id) {
            skipped = true;
            continue;
        }
        busy.insert(stack.id.clone());
        started = true;
        let rt = runtime.clone();
        let sync = sync.clone();
        let skew = skew.clone();
        let stack = stack.clone();
        jobs.spawn(async move {
            let outcome = match rt.caps().require(AgentCapability::Apply) {
                Err(e) => Err(e),
                Ok(()) => {
                    let secrets = if stack.secret_names.is_empty() {
                        Ok(BTreeMap::new())
                    } else {
                        sync.secrets(&stack.id).await
                    };
                    match secrets {
                        Err(e) => Err(format!("could not fetch secrets: {e}")),
                        Ok(s) => {
                            let auths = match sync.registry_auth(&[]).await {
                                Ok(a) => a,
                                Err(e) => {
                                    eprintln!("photon-agent: registry credentials unavailable ({e}); applying without");
                                    Vec::new()
                                }
                            };
                            rt.apply_stack(&stack, &s, &auths).await
                        }
                    }
                }
            };
            let (ok, detail, hashes) = match outcome {
                Ok((d, h)) => (true, d, h),
                Err(e) => (false, e, BTreeMap::new()),
            };
            Job::Stack {
                result: StackResult {
                    stack_id: stack.id.clone(),
                    generation: stack.generation,
                    op: stack.op,
                    ok,
                    detail: Some(detail),
                    finished_ms: skew.now(),
                    config_hashes: hashes,
                },
                name: stack.name.clone(),
            }
        });
    }
    if !skipped {
        state.lock().unwrap().known_generation = desired.generation;
    }
    started
}

#[cfg(test)]
mod tests {
    use super::*;
    use photon_deploy::model::{ActionArgs, ActionKind};

    fn spec(id: &str, expires: i64) -> ActionSpec {
        ActionSpec {
            id: id.into(),
            kind: ActionKind::Restart,
            container_id: Some("c".into()),
            image: None,
            volume: None,
            args: ActionArgs::default(),
            expires_ms: expires,
        }
    }

    #[test]
    fn actions_run_once_and_expire() {
        let mut st = LocalState::default();
        assert!(matches!(
            action_disposition(&mut st, &spec("a", 10_000), 5_000),
            Some(Ok(()))
        ));
        assert!(
            action_disposition(&mut st, &spec("a", 10_000), 5_000).is_none(),
            "seen ids are skipped"
        );
        assert!(matches!(
            action_disposition(&mut st, &spec("b", 1_000), 5_000),
            Some(Err(_))
        ));
        assert!(action_disposition(&mut st, &spec("b", 1_000), 5_000).is_none());
    }
}

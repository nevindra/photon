//! The control channel, server side (section 7.4): `POST /api/agent/sync` under the per-host
//! agent token — the fourth auth system, never the ingest token (R1.4). One round trip: the
//! request carries what the host looks like and what the agent did; the response carries what
//! it should be. `POST /api/agent/secrets` hands a stack's secret values to the one host that
//! desires it; `POST /api/agent/registry-auth` the image-registry credentials the host is scoped
//! for, right before a pull; `POST /api/agent/stream/:session` receives a stream upload;
//! `GET …/input` is the exec input long-poll.

use super::agents::request_base;
use super::hub::StatSample;
use super::{api, DeployApi};
use crate::AppState;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use photon_deploy::diff;
use photon_deploy::ids::{digest_eq, sha256_hex};
use photon_deploy::model::*;
use photon_deploy::proto::*;
use photon_deploy::store::ObservedRecord;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// The longest the server holds a long-poll (the agent asks for its poll interval; a
/// misbehaving one cannot pin a connection for an hour).
const MAX_WAIT_SECS: u32 = 60;

fn reject(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

/// Resolve the `Authorization: Bearer ak_…` header to the host it names, in constant time on
/// the digest. `None` = 401.
async fn authenticate(a: &DeployApi, headers: &HeaderMap) -> Option<HostRecord> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?
        .trim();
    if !token.starts_with("ak_") || token.len() < 20 {
        return None;
    }
    let hash = sha256_hex(token.as_bytes());
    let rec = a.store.get_host_by_token_hash(&hash).await.ok()??;
    if digest_eq(&rec.token_hash, &hash) {
        Some(rec)
    } else {
        None
    }
}

impl DeployApi {
    /// The desired-state document for a host: every stack that is not gone, rendered.
    pub async fn desired_for(
        &self,
        host: &HostRecord,
    ) -> Result<Desired, photon_core::PhotonError> {
        let stacks = self.store.list_stacks().await?;
        let mut out = Desired {
            generation: host.desired_generation,
            stacks: vec![],
        };
        for s in stacks.into_iter().filter(|s| {
            s.host == host.host && s.status != StackStatus::Gone && s.desired_generation > 0
        }) {
            // A stack saved but never applied ("declared"/"rendered" with no pending op) is
            // not desired on the host yet.
            let Some(op) = s.pending_op.or(if s.applied_generation.is_some() {
                Some(StackOp::Up)
            } else {
                None
            }) else {
                continue;
            };
            out.stacks.push(DesiredStack {
                id: s.id.clone(),
                name: s.name.clone(),
                generation: s.desired_generation,
                op,
                files: s.files.clone(),
                secret_names: s.secrets.clone(),
                deploy_options: s.deploy_options.clone(),
                remove_volumes: op == StackOp::Destroy && s.deploy_options.recreate_volumes,
                prepull: matches!(s.deploy_options.pull, PullPolicy::Always),
            });
        }
        Ok(out)
    }

    /// Is there anything for this host right now (used to decide whether to hold the poll)?
    async fn has_work(&self, host: &HostRecord, known_generation: u64) -> bool {
        if host.desired_generation > known_generation {
            return true;
        }
        if self.hub.has_open_streams(&host.host) {
            return true;
        }
        if host.upgrade_to.is_some() && host.upgrade_to != host.agent_version {
            return true;
        }
        matches!(self.store.open_actions(&host.host).await, Ok(v) if v.iter().any(|a| a.status == ActionStatus::Queued))
    }

    /// Fold one sync request into the store: host identity, observed inventory, action and
    /// stack results, events. Returns the refreshed host row.
    async fn absorb(
        &self,
        mut rec: HostRecord,
        req: &SyncRequest,
    ) -> Result<HostRecord, photon_core::PhotonError> {
        let now = super::now_ms();
        let was_pending = rec.last_seen_ms.is_none();
        rec.last_seen_ms = Some(now);
        rec.agent_version = Some(req.agent_version.clone());
        rec.reported_capabilities = req.capabilities.clone();
        rec.host_id = req.host.id.clone().or(rec.host_id.take());
        if !req.host.arch.is_empty() {
            rec.arch = Some(req.host.arch.clone());
        }
        if !req.host.os.is_empty() {
            rec.os = Some(req.host.os.clone());
        }
        if rec.upgrade_to.as_deref() == Some(req.agent_version.as_str()) {
            rec.upgrade_to = None;
        }
        if let Some(obs) = &req.observed {
            if let Some(f) = &obs.facts {
                rec.docker = Some(f.clone());
            }
            self.store
                .put_observed(&ObservedRecord {
                    host: rec.host.clone(),
                    reported_ms: now,
                    observed: obs.clone(),
                })
                .await?;
            let ids: Vec<String> = obs.containers.iter().map(|c| c.id.clone()).collect();
            self.hub.retain_containers(&rec.host, &ids);
            for c in &obs.containers {
                if c.state == ContainerState::Running
                    && (c.cpu_pct.is_some() || c.mem_bytes.is_some())
                {
                    self.hub.push_stat(
                        &rec.host,
                        &c.id,
                        StatSample {
                            t: now,
                            cpu_pct: c.cpu_pct,
                            mem_bytes: c.mem_bytes,
                            mem_limit_bytes: c.mem_limit_bytes,
                            net_rx_bps: c.net_rx_bps,
                            net_tx_bps: c.net_tx_bps,
                            block_read_bps: c.block_read_bps,
                            block_write_bps: c.block_write_bps,
                        },
                    );
                }
            }
        }
        // Write only what the sync owns, atomically — a UI edit or a generation bump that
        // landed since `rec` was read must survive this.
        let snap = rec.clone();
        self.store
            .update_host(
                &rec.host,
                Box::new(move |r| {
                    r.last_seen_ms = snap.last_seen_ms;
                    r.agent_version = snap.agent_version;
                    r.reported_capabilities = snap.reported_capabilities;
                    r.host_id = snap.host_id;
                    r.arch = snap.arch;
                    r.os = snap.os;
                    r.docker = snap.docker;
                    if r.upgrade_to.is_some() && r.upgrade_to == r.agent_version {
                        r.upgrade_to = None;
                    }
                }),
            )
            .await?;
        if was_pending {
            self.audit_system(
                &rec.host,
                Role::Admin,
                "agent.enroll",
                AuditTarget {
                    host: Some(rec.host.clone()),
                    ..Default::default()
                },
                AuditResultKind::Ok,
                Some(format!(
                    "first sync from photon-agent {} ({} {})",
                    req.agent_version, req.host.os, req.host.arch
                )),
            )
            .await;
        }
        if !req.events.is_empty() {
            self.hub.push_events(&rec.host, req.events.clone());
        }
        for r in &req.results {
            self.absorb_action_result(&rec, r).await?;
        }
        for r in &req.stack_results {
            self.absorb_stack_result(&rec, r).await?;
        }
        // Drift: every stack on the host, judged against what it just reported.
        if let Some(obs) = &req.observed {
            for mut s in self.store.list_stacks().await?.into_iter().filter(|s| {
                s.host == rec.host
                    && matches!(s.status, StackStatus::Running | StackStatus::Drifted)
            }) {
                let os = obs.stacks.iter().find(|x| x.name == s.name);
                let drift = diff::compute(&s, &obs.containers, os, &s.drift, now);
                let drifted = !drift.is_empty();
                let changed = drifted != (s.status == StackStatus::Drifted) || drift != s.drift;
                if changed {
                    s.drift = drift;
                    s.status = if drifted {
                        StackStatus::Drifted
                    } else {
                        StackStatus::Running
                    };
                    s.drift_since_ms = if drifted {
                        s.drift_since_ms.or(Some(now))
                    } else {
                        None
                    };
                    s.updated_ms = now;
                    self.store.put_stack(&s).await?;
                }
            }
        }
        Ok(rec)
    }

    async fn absorb_action_result(
        &self,
        rec: &HostRecord,
        r: &ActionResult,
    ) -> Result<(), photon_core::PhotonError> {
        let Some(mut a) = self.store.get_action(&r.action_id).await? else {
            return Ok(());
        };
        if a.host != rec.host
            || matches!(
                a.status,
                ActionStatus::Done | ActionStatus::Failed | ActionStatus::Expired
            )
        {
            return Ok(());
        }
        a.status = r.status;
        a.started_ms = r.started_ms.or(a.started_ms);
        a.finished_ms = r.finished_ms.or(Some(super::now_ms()));
        a.result = r.detail.clone();
        self.store.put_action(&a).await?;
        let (kind, detail) = match r.status {
            ActionStatus::Done => (AuditResultKind::Ok, r.detail.clone()),
            ActionStatus::Expired => (
                AuditResultKind::Failed,
                Some(r.detail.clone().unwrap_or_else(|| "expired".into())),
            ),
            ActionStatus::Failed => (AuditResultKind::Failed, r.detail.clone()),
            _ => return Ok(()),
        };
        self.audit_system(
            &a.actor,
            Role::Operator,
            &a.kind.audit_action(),
            AuditTarget {
                host: Some(rec.host.clone()),
                container: a.container_name.clone(),
                image: a.image.clone(),
                volume: a.volume.clone(),
                ..Default::default()
            },
            kind,
            detail.map(|d| d.lines().last().unwrap_or("").trim().to_string()),
        )
        .await;
        Ok(())
    }

    async fn absorb_stack_result(
        &self,
        rec: &HostRecord,
        r: &StackResult,
    ) -> Result<(), photon_core::PhotonError> {
        let Some(mut s) = self.store.get_stack(&r.stack_id).await? else {
            return Ok(());
        };
        if s.host != rec.host
            || r.generation < s.desired_generation.saturating_sub(0)
                && r.generation < s.applied_generation.unwrap_or(0)
        {
            return Ok(());
        }
        let now = super::now_ms();
        let last = r.detail.as_deref().map(|d| {
            d.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string()
        });
        if r.ok {
            s.applied_generation = Some(r.generation);
            s.last_error = None;
            s.last_applied_ms = Some(r.finished_ms.max(1)).map(|t| if t > 1 { t } else { now });
            s.pending_op = None;
            match r.op {
                StackOp::Destroy => {
                    s.status = StackStatus::Gone;
                }
                StackOp::Down => {
                    s.status = StackStatus::Running; // "applied and matching": nothing runs and nothing should
                    s.baseline_config_hashes = r.config_hashes.clone();
                }
                _ => {
                    s.status = StackStatus::Running;
                    if !r.config_hashes.is_empty() {
                        s.baseline_config_hashes = r.config_hashes.clone();
                    }
                    s.drift = vec![];
                    s.drift_since_ms = None;
                }
            }
            // The release marker (R7.1): a template stack's version lands on the service graphs.
            if let (Some(store), Some(ver)) = (&self.deployments, s.template_version.as_deref()) {
                let env = s
                    .variables
                    .get("ENVIRONMENT")
                    .or_else(|| s.variables.get("DEPLOYMENT_ENVIRONMENT"))
                    .cloned()
                    .unwrap_or_default();
                for svc in &s.apm_services {
                    let key = crate::deployments::DeploymentKey {
                        service: svc.clone(),
                        environment: env.clone(),
                        tenant: String::new(),
                        version: ver.to_string(),
                    };
                    let details = crate::deployments::CiDetails {
                        commit_sha: None,
                        author: Some(s.created_by.clone()),
                        message: Some(format!(
                            "Deployed by Photon: stack {} on {} (generation {})",
                            s.name, s.host, r.generation
                        )),
                        url: None,
                    };
                    let _ = store.record_ci(&key, now, &details).await;
                }
            }
        } else {
            s.status = StackStatus::Failed;
            // The operator needs compose's actual complaint, not "exit status: 1": keep the
            // tail of the transcript (the agent already capped it), not just its last line.
            s.last_error = r
                .detail
                .as_deref()
                .map(|d| {
                    let lines: Vec<&str> = d.lines().filter(|l| !l.trim().is_empty()).collect();
                    let n = lines.len().saturating_sub(12);
                    let tail = lines[n..].join("\n");
                    if tail.len() > 2000 {
                        tail[tail.len() - 2000..].to_string()
                    } else {
                        tail
                    }
                })
                .filter(|t| !t.is_empty())
                .or_else(|| Some("apply failed".into()));
            s.pending_op = None;
        }
        s.updated_ms = now;
        self.store.put_stack(&s).await?;
        let action = match r.op {
            StackOp::Destroy => "stack.destroy",
            StackOp::Down => "stack.down",
            StackOp::Restart => "stack.restart",
            StackOp::Redeploy => "stack.redeploy",
            StackOp::Reconcile => "stack.reconcile",
            StackOp::Up => "stack.apply",
        };
        self.audit_system(
            &s.created_by,
            Role::Operator,
            action,
            AuditTarget {
                host: Some(rec.host.clone()),
                stack: Some(s.name.clone()),
                version: s.template_version.clone(),
                ..Default::default()
            },
            if r.ok {
                AuditResultKind::Ok
            } else {
                AuditResultKind::Failed
            },
            Some(format!(
                "generation {} · {}",
                r.generation,
                last.unwrap_or_default()
            )),
        )
        .await;
        Ok(())
    }

    /// A stack with unapplied desired state whose host document the agent already holds is a
    /// bump that got lost (a write raced the sync fold before `update_host` existed, or a store
    /// restored from backup). The agent asks only "is the generation newer than mine", so left
    /// alone that stack would sit in `applying`/`destroying` forever. Bump the host here, at the
    /// one place that can see both sides, so the next answer carries the document. Failed stacks
    /// are excluded — the agent answered those, and re-sending would retry without anyone asking.
    async fn heal_stale_generation(
        &self,
        rec: HostRecord,
        known_generation: u64,
    ) -> Result<HostRecord, photon_core::PhotonError> {
        if rec.desired_generation > known_generation {
            return Ok(rec);
        }
        let pending = self.store.list_stacks().await?.into_iter().any(|s| {
            s.host == rec.host
                && s.last_error.is_none()
                && s.desired_generation > s.applied_generation.unwrap_or(0)
        });
        if !pending {
            return Ok(rec);
        }
        eprintln!(
            "photon-api: {} has stack work its document did not carry (generation {} ≤ the agent's {}); bumping",
            rec.host, rec.desired_generation, known_generation
        );
        self.store
            .update_host(&rec.host, Box::new(|r| r.desired_generation += 1))
            .await?;
        Ok(self.store.get_host(&rec.host).await?.unwrap_or(rec))
    }

    async fn respond(
        &self,
        rec: &HostRecord,
        req: &SyncRequest,
        base: &str,
    ) -> Result<SyncResponse, photon_core::PhotonError> {
        let now = super::now_ms();
        let desired = if rec.desired_generation > req.known_generation {
            Some(self.desired_for(rec).await?)
        } else {
            None
        };
        let mut actions = Vec::new();
        for mut a in self.store.open_actions(&rec.host).await? {
            if a.expires_ms < now {
                continue; // the supervisor expires it
            }
            if a.status == ActionStatus::Queued {
                a.status = ActionStatus::Running;
                a.started_ms = Some(now);
                self.store.put_action(&a).await?;
            }
            actions.push(ActionSpec {
                id: a.id,
                kind: a.kind,
                container_id: a.container_id,
                image: a.image,
                volume: a.volume,
                args: a.args,
                expires_ms: a.expires_ms,
            });
        }
        let streams = self.hub.streams_for(&rec.host);
        let upgrade = match (&rec.upgrade_to, &rec.agent_version) {
            (Some(v), current) if Some(v) != current.as_ref() => {
                let arch = rec.arch.clone().unwrap_or_else(|| "x86_64".into());
                let sha256 = match self.agent_binary_path(&arch) {
                    Some(p) => super::agents::agent_binary_digest(&p).await,
                    None => None,
                };
                Some(UpgradeSpec {
                    version: v.clone(),
                    url: format!("{base}/api/agent/download?arch={arch}&version={v}"),
                    sha256,
                })
            }
            _ => None,
        };
        let next_poll_secs = if streams.is_empty() { rec.poll_secs } else { 1 };
        Ok(SyncResponse {
            generation: rec.desired_generation,
            desired,
            actions,
            streams,
            next_poll_secs,
            upgrade,
            wanted_capabilities: rec.requested_capabilities.clone(),
            server_time_ms: now,
            // An unconfigured `[deploy].server_origin` leaves `{host}` in the hint: the agent
            // reached us at `base`, so its metrics can reach the same host.
            ingest: self.ingest.as_ref().map(|i| {
                let host = base
                    .split_once("://")
                    .map(|(_, r)| r)
                    .unwrap_or(base)
                    .split('/')
                    .next()
                    .unwrap_or(base)
                    .rsplit_once(':')
                    .map(|(h, _)| h)
                    .unwrap_or(base);
                IngestSpec {
                    metrics_endpoint: i.metrics_endpoint.replace("{host}", host),
                    logs_endpoint: i.logs_endpoint.replace("{host}", host),
                    token: i.token.clone(),
                }
            }),
        })
    }
}

pub(crate) async fn sync(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SyncRequest>,
) -> Response {
    let Some(a) = state.deploy.as_ref() else {
        return reject(StatusCode::NOT_FOUND, "deploy subsystem disabled");
    };
    let Some(rec) = authenticate(a, &headers).await else {
        return reject(StatusCode::UNAUTHORIZED, "unknown agent token");
    };
    if req.protocol > PROTOCOL_VERSION {
        return reject(
            StatusCode::BAD_REQUEST,
            &format!(
                "agent protocol {} is newer than this server's {PROTOCOL_VERSION}",
                req.protocol
            ),
        );
    }
    if req.host.name != rec.host {
        eprintln!(
            "photon-api: agent for {} reports host name {} — the enrolment name wins",
            rec.host, req.host.name
        );
    }
    let rec = match a.absorb(rec, &req).await {
        Ok(r) => r,
        Err(e) => return reject(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let rec = match a.heal_stale_generation(rec, req.known_generation).await {
        Ok(r) => r,
        Err(e) => return reject(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    // Long-poll: hold until there is work, the agent's wait elapses, or something wakes it.
    let wait = req.wait_secs.min(MAX_WAIT_SECS);
    let mut rec = rec;
    if wait > 0 && !a.has_work(&rec, req.known_generation).await {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(wait as u64);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            a.hub.wait(&rec.host, left.as_millis() as u64).await;
            rec = match a.store.get_host(&rec.host).await {
                Ok(Some(r)) => r,
                _ => return reject(StatusCode::UNAUTHORIZED, "host was removed"),
            };
            if a.has_work(&rec, req.known_generation).await {
                break;
            }
        }
        // The hold itself is proof of life.
        let seen = super::now_ms();
        rec.last_seen_ms = Some(seen);
        let _ = a
            .store
            .update_host(&rec.host, Box::new(move |r| r.last_seen_ms = Some(seen)))
            .await;
    }
    let base = request_base(a, &headers);
    match a.respond(&rec, &req, &base).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

pub(crate) async fn secrets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SecretsRequest>,
) -> Response {
    let Some(a) = state.deploy.as_ref() else {
        return reject(StatusCode::NOT_FOUND, "deploy subsystem disabled");
    };
    let Some(rec) = authenticate(a, &headers).await else {
        return reject(StatusCode::UNAUTHORIZED, "unknown agent token");
    };
    let Ok(Some(stack)) = a.store.get_stack(&req.stack_id).await else {
        return reject(StatusCode::NOT_FOUND, "no such stack");
    };
    if stack.host != rec.host {
        return reject(
            StatusCode::FORBIDDEN,
            "that stack is desired on another host",
        );
    }
    match a.secret_values(&stack.id).await {
        Ok(values) => Json(SecretsResponse { values }).into_response(),
        Err((status, body)) => (status, body).into_response(),
    }
}

/// The pull credentials for this host — decrypted for the one agent that authenticated, for the
/// registries it names (or all it is scoped for). Nothing is written on the server side: the
/// pull itself is what the audit log records, as the action or stack result it belongs to.
pub(crate) async fn registry_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegistryAuthRequest>,
) -> Response {
    let Some(a) = state.deploy.as_ref() else {
        return reject(StatusCode::NOT_FOUND, "deploy subsystem disabled");
    };
    let Some(rec) = authenticate(a, &headers).await else {
        return reject(StatusCode::UNAUTHORIZED, "unknown agent token");
    };
    match a.registry_auth_for(&rec, &req.servers).await {
        Ok(credentials) => Json(RegistryAuthResponse { credentials }).into_response(),
        Err((status, body)) => (status, body).into_response(),
    }
}

/// The stream upload: NDJSON chunks until the agent's source ends or we close the session.
pub(crate) async fn stream_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session): Path<String>,
    body: Body,
) -> Response {
    let Some(a) = state.deploy.as_ref() else {
        return reject(StatusCode::NOT_FOUND, "deploy subsystem disabled");
    };
    let Some(rec) = authenticate(a, &headers).await else {
        return reject(StatusCode::UNAUTHORIZED, "unknown agent token");
    };
    let owner = a.hub.with_stream(&session, |s| s.host.clone());
    match owner {
        Some(h) if h == rec.host => {}
        Some(_) => {
            return reject(
                StatusCode::FORBIDDEN,
                "that session belongs to another host",
            )
        }
        None => return reject(StatusCode::GONE, "session closed"),
    }
    let mut stream = body.into_data_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut closed = false;
    while let Some(next) = stream.next().await {
        let Ok(bytes) = next else { break };
        buf.extend_from_slice(&bytes);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            if let Ok(chunk) = serde_json::from_slice::<StreamChunk>(&line) {
                if let StreamChunk::Stat {
                    ts_ms,
                    cpu_pct,
                    mem_bytes,
                    mem_limit_bytes,
                    net_rx_bps,
                    net_tx_bps,
                    block_read_bps,
                    block_write_bps,
                } = &chunk
                {
                    if let Some(cid) = a.hub.with_stream(&session, |s| s.container_id.clone()) {
                        a.hub.push_stat(
                            &rec.host,
                            &cid,
                            StatSample {
                                t: *ts_ms,
                                cpu_pct: *cpu_pct,
                                mem_bytes: *mem_bytes,
                                mem_limit_bytes: *mem_limit_bytes,
                                net_rx_bps: *net_rx_bps,
                                net_tx_bps: *net_tx_bps,
                                block_read_bps: *block_read_bps,
                                block_write_bps: *block_write_bps,
                            },
                        );
                    }
                }
                let ended = matches!(chunk, StreamChunk::End { .. });
                let alive = a.hub.with_stream(&session, |s| {
                    s.push(chunk);
                    !s.closed
                });
                match alive {
                    None => {
                        closed = true;
                    }
                    Some(false) => {
                        closed = true;
                    }
                    Some(true) => {}
                }
                if ended {
                    closed = true;
                }
            }
            if closed {
                break;
            }
        }
        if closed {
            break;
        }
        if !a.hub.stream_exists(&session) {
            break;
        }
    }
    StatusCode::OK.into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct InputParams {
    #[serde(default)]
    pub wait: Option<u64>,
}

/// Exec input long-poll: lines typed since the last call, or `closed` once the UI hung up.
pub(crate) async fn stream_input(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session): Path<String>,
    Query(p): Query<InputParams>,
) -> Response {
    let Some(a) = state.deploy.as_ref() else {
        return reject(StatusCode::NOT_FOUND, "deploy subsystem disabled");
    };
    let Some(rec) = authenticate(a, &headers).await else {
        return reject(StatusCode::UNAUTHORIZED, "unknown agent token");
    };
    let wait = Duration::from_secs(p.wait.unwrap_or(20).min(30));
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let taken = a.hub.with_stream(&session, |s| {
            if s.host != rec.host {
                return None;
            }
            let lines: Vec<String> = s.input.drain(..).collect();
            Some((lines, s.closed, s.input_notify.clone()))
        });
        match taken {
            None => {
                return Json(ExecInput {
                    lines: vec![],
                    closed: true,
                })
                .into_response()
            }
            Some(None) => {
                return reject(
                    StatusCode::FORBIDDEN,
                    "that session belongs to another host",
                )
            }
            Some(Some((lines, closed, notify))) => {
                if !lines.is_empty() || closed {
                    return Json(ExecInput { lines, closed }).into_response();
                }
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return Json(ExecInput {
                        lines: vec![],
                        closed: false,
                    })
                    .into_response();
                }
                let _ = tokio::time::timeout(left, notify.notified()).await;
            }
        }
    }
}

#[allow(dead_code)]
fn _keep(a: &AppState) -> Result<&DeployApi, super::ApiErr> {
    api(a)
}

//! Read-only Docker per host (R2): the inventory the agent reported on its last sync, container
//! logs and live stats through agent-initiated streams, daemon events, the prune preview, the
//! compose projects Photon does not manage yet, and the exec shell (R3.4).
//!
//! Logs and stats are served from the hub's session buffers. The first read for a container
//! opens a session (the agent picks it up on its next poll — immediately, thanks to the long
//! poll — and starts uploading); every later read refreshes the session's idle clock; a session
//! nobody reads for `STREAM_IDLE_MS` is closed and the agent stops.

use super::hub::{find_container, stat_series, StatSample};
use super::{api, bad, not_found, require, store_err, ApiErr, DeployApi};
use crate::auth::CurrentUser;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use photon_deploy::model::*;
use photon_deploy::proto::{StreamChunk, StreamKind, StreamSpec};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

/// How long the first read of a fresh log/stat session waits for the agent's first chunk.
const FIRST_CHUNK_WAIT: Duration = Duration::from_secs(6);

impl DeployApi {
    /// The host's inventory as the UI reads it, or `Err(404)` when the host is not enrolled.
    pub async fn inventory(&self, host: &str) -> Result<HostInventory, ApiErr> {
        let rec = self.host(host).await?;
        let now = super::now_ms();
        let status = rec.status(now);
        let obs = self.observed(host).await?;
        let stale_after = (rec.poll_secs as i64 * 2 + 30) * 1000;
        let stacks = self.store.list_stacks().await.map_err(store_err)?;
        Ok(match obs {
            Some(o) => {
                let mut containers = o.observed.containers;
                // Resolve the managed stack id for containers whose project is a stack here
                // (the agent labels the ones it created; adopted projects are matched by name).
                for c in containers.iter_mut() {
                    if c.managed_stack_id.is_none() {
                        if let Some(p) = &c.compose_project {
                            if let Some(s) = stacks.iter().find(|s| {
                                s.host == host && &s.name == p && s.status != StackStatus::Gone
                            }) {
                                c.managed_stack_id = Some(s.id.clone());
                            }
                        }
                    }
                }
                HostInventory {
                    host: host.to_string(),
                    agent_status: status,
                    fetched_ms: o.reported_ms,
                    stale: now - o.reported_ms > stale_after,
                    containers,
                    images: o.observed.images,
                    volumes: o.observed.volumes,
                    networks: o.observed.networks,
                    facts: o.observed.facts.or(rec.docker.clone()),
                }
            }
            None => HostInventory {
                host: host.to_string(),
                agent_status: status,
                fetched_ms: rec.last_seen_ms.unwrap_or(rec.enrolled_ms),
                stale: true,
                containers: vec![],
                images: vec![],
                volumes: vec![],
                networks: vec![],
                facts: rec.docker.clone(),
            },
        })
    }

    /// A container on a host, by id or name, from the latest inventory.
    pub async fn container(&self, host: &str, id: &str) -> Result<Container, ApiErr> {
        let inv = self.inventory(host).await?;
        find_container(&inv.containers, id)
            .cloned()
            .ok_or_else(|| not_found(format!("no container {id} on {host} in its last inventory")))
    }

    /// Ensure a log/stat stream session exists for a container and return its id + whether it
    /// was just created.
    fn ensure_stream(
        &self,
        host: &str,
        kind: StreamKind,
        c: &Container,
        tail: Option<u32>,
        since_ms: Option<i64>,
    ) -> (String, bool) {
        let spec = StreamSpec {
            session: String::new(),
            kind,
            container_id: c.id.clone(),
            tail,
            since_ms,
            shell: None,
        };
        let (id, created) =
            self.hub
                .open_stream(host, kind, &c.id, &c.name, spec, None, super::now_ms());
        if created {
            self.hub.wake(host);
        }
        (id, created)
    }
}

pub(crate) async fn inventory(
    State(state): State<AppState>,
    Path(host): Path<String>,
) -> Result<Json<HostInventory>, ApiErr> {
    let a = api(&state)?;
    Ok(Json(a.inventory(&host).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct LogsParams {
    #[serde(default)]
    pub tail: Option<u32>,
    #[serde(default)]
    pub since: Option<i64>,
}

pub(crate) async fn logs(
    State(state): State<AppState>,
    Path((host, id)): Path<(String, String)>,
    Query(p): Query<LogsParams>,
) -> Result<Json<ContainerLogsResult>, ApiErr> {
    let a = api(&state)?;
    let rec = a.host(&host).await?;
    if !rec.capabilities().contains(&AgentCapability::Read) {
        return Err(bad(
            "the agent on this host was installed without --docker-read",
        ));
    }
    let c = a.container(&host, &id).await?;
    let tail = p.tail.unwrap_or(200).min(5000);
    let (sid, created) = a.ensure_stream(&host, StreamKind::Logs, &c, Some(tail), None);
    if created && rec.status(super::now_ms()) == AgentStatus::Online {
        wait_for_chunk(a, &sid, FIRST_CHUNK_WAIT).await;
    }
    let chunks = a.hub.with_stream(&sid, |s| {
        s.last_read_ms = super::now_ms();
        s.since(0)
    });
    let Some(chunks) = chunks else {
        return Ok(Json(ContainerLogsResult {
            lines: vec![],
            truncated: false,
        }));
    };
    let since = p.since.unwrap_or(0);
    let lines: Vec<ContainerLogLine> = chunks
        .into_iter()
        .filter_map(|ch| match ch {
            StreamChunk::Log {
                ts_ms,
                stream,
                text,
            } if ts_ms > since => Some(ContainerLogLine {
                ts_ms,
                stream,
                text,
            }),
            _ => None,
        })
        .collect();
    Ok(Json(ContainerLogsResult {
        lines,
        truncated: false,
    }))
}

/// Block until the session has any chunk, the agent ended it, or the wait elapses.
async fn wait_for_chunk(a: &DeployApi, sid: &str, max: Duration) {
    let deadline = tokio::time::Instant::now() + max;
    loop {
        let notify = a.hub.with_stream(sid, |s| {
            (s.buffer.is_empty() && !s.closed, s.output_notify.clone())
        });
        match notify {
            Some((true, n)) => {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return;
                }
                let _ = tokio::time::timeout(left, n.notified()).await;
                if tokio::time::Instant::now() >= deadline {
                    return;
                }
            }
            _ => return,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct StatsParams {
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
}

pub(crate) async fn stats(
    State(state): State<AppState>,
    Path((host, id)): Path<(String, String)>,
    Query(p): Query<StatsParams>,
) -> Result<Json<ContainerStats>, ApiErr> {
    let a = api(&state)?;
    let rec = a.host(&host).await?;
    let c = a.container(&host, &id).await?;
    // Time bounds arrive as nanosecond strings (the app-wide window); the ring is in ms.
    let to_ms = |s: &Option<String>, dflt: i64| {
        s.as_deref()
            .and_then(|v| v.parse::<i128>().ok())
            .map(|ns| (ns / 1_000_000) as i64)
            .unwrap_or(dflt)
    };
    let now = super::now_ms();
    let from = to_ms(&p.start, now - 30 * 60_000);
    let to = to_ms(&p.end, now);
    if rec.capabilities().contains(&AgentCapability::Read) && c.state == ContainerState::Running {
        let (sid, created) = a.ensure_stream(&host, StreamKind::Stats, &c, None, None);
        if created && rec.status(now) == AgentStatus::Online {
            wait_for_chunk(a, &sid, Duration::from_secs(3)).await;
        }
        // Fold whatever the stream has delivered into the ring, then read the ring.
        if let Some(chunks) = a.hub.with_stream(&sid, |s| {
            s.last_read_ms = now;
            s.since(0)
        }) {
            for ch in chunks {
                if let StreamChunk::Stat {
                    ts_ms,
                    cpu_pct,
                    mem_bytes,
                    mem_limit_bytes,
                    net_rx_bps,
                    net_tx_bps,
                    block_read_bps,
                    block_write_bps,
                } = ch
                {
                    a.hub.push_stat(
                        &host,
                        &c.id,
                        StatSample {
                            t: ts_ms,
                            cpu_pct,
                            mem_bytes,
                            mem_limit_bytes,
                            net_rx_bps,
                            net_tx_bps,
                            block_read_bps,
                            block_write_bps,
                        },
                    );
                }
            }
        }
    }
    let samples = a.hub.stats(&host, &c.id, from, to);
    let mut out = stat_series(&samples);
    if out.mem_limit_bytes.is_none() {
        out.mem_limit_bytes = c.mem_limit_bytes;
    }
    Ok(Json(out))
}

#[derive(Debug, Default, Deserialize)]
pub struct LimitParams {
    #[serde(default)]
    pub limit: Option<usize>,
}

pub(crate) async fn events(
    State(state): State<AppState>,
    Path(host): Path<String>,
    Query(p): Query<LimitParams>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    let _ = a.host(&host).await?;
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    Ok(Json(json!({ "events": a.hub.events(&host, limit) })))
}

pub(crate) async fn prune_preview(
    State(state): State<AppState>,
    Path(host): Path<String>,
) -> Result<Json<PrunePreview>, ApiErr> {
    let a = api(&state)?;
    let _ = a.host(&host).await?;
    let obs = a.observed(&host).await?;
    let preview = obs
        .and_then(|o| o.observed.prune_preview)
        .unwrap_or(PrunePreview {
            host: host.clone(),
            images: PruneCategory::default(),
            containers: PruneCategory::default(),
            volumes: PruneCategory::default(),
            build_cache: PruneCategory::default(),
        });
    Ok(Json(PrunePreview { host, ..preview }))
}

pub(crate) async fn compose_projects(
    State(state): State<AppState>,
    Path(host): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    let inv = a.inventory(&host).await?;
    let managed: Vec<String> = a
        .store
        .list_stacks()
        .await
        .map_err(store_err)?
        .into_iter()
        .filter(|s| s.host == host && s.status != StackStatus::Gone)
        .map(|s| s.name)
        .collect();
    Ok(Json(
        json!({ "projects": super::hub::discover_projects(&host, &inv.containers, &managed) }),
    ))
}

// --- exec (R3.4) ---

pub(crate) async fn open_exec(
    State(state): State<AppState>,
    me: CurrentUser,
    Path((host, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Exec)?;
    let rec = a.host(&host).await?;
    if !rec.capabilities().contains(&AgentCapability::Exec) {
        a.audit(
            &me,
            "container.exec",
            AuditTarget {
                host: Some(host.clone()),
                container: Some(id.clone()),
                ..Default::default()
            },
            AuditResultKind::Denied,
            Some("agent installed without --docker-exec".into()),
            None,
        )
        .await;
        return Err(bad(
            "the agent on this host was installed without --docker-exec",
        ));
    }
    if rec.status(super::now_ms()) != AgentStatus::Online {
        return Err(bad(format!(
            "{host} is not online — a shell needs a live agent"
        )));
    }
    let c = a.container(&host, &id).await?;
    if c.state != ContainerState::Running {
        return Err(bad(format!(
            "{} is {}, not running",
            c.name,
            c.state.as_str()
        )));
    }
    let spec = StreamSpec {
        session: String::new(),
        kind: StreamKind::Exec,
        container_id: c.id.clone(),
        tail: None,
        since_ms: None,
        shell: Some("sh".into()),
    };
    let now = super::now_ms();
    let (sid, _) = a.hub.open_stream(
        &host,
        StreamKind::Exec,
        &c.id,
        &c.name,
        spec,
        Some((me.username.clone(), me.role)),
        now,
    );
    a.hub.wake(&host);
    a.audit(
        &me,
        "container.exec",
        AuditTarget {
            host: Some(host.clone()),
            container: Some(c.name.clone()),
            ..Default::default()
        },
        AuditResultKind::Ok,
        Some("shell opened".into()),
        None,
    )
    .await;
    Ok(Json(
        json!({ "ok": true, "session": ExecSession { id: sid, host, container_id: c.id, container_name: c.name, shell: "sh".into(), opened_ms: now } }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct ExecLine {
    pub line: String,
}

pub(crate) async fn exec_input(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(sid): Path<String>,
    Json(body): Json<ExecLine>,
) -> Result<Json<ExecOutput>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Exec)?;
    let now = super::now_ms();
    let pushed = a.hub.with_stream(&sid, |s| {
        if s.kind != StreamKind::Exec {
            return Err(bad("not an exec session"));
        }
        if s.closed {
            // Hand over whatever the shell printed before it went, without waiting.
            return Ok((
                s.delivered_seq.max(s.base_seq),
                s.next_seq(),
                s.output_notify.clone(),
            ));
        }
        s.last_read_ms = now;
        // Read from where the last call left off, not from "now": output the shell printed
        // after the previous call returned (a slow command, a late echo) belongs to this read.
        let seq = s.delivered_seq.max(s.base_seq);
        let at_send = s.next_seq();
        if !body.line.is_empty() {
            s.input.push_back(body.line.clone());
            s.commands += 1;
            s.input_notify.notify_one();
        }
        Ok((seq, at_send, s.output_notify.clone()))
    });
    let Some(pushed) = pushed else {
        return Err(super::err(StatusCode::NOT_FOUND, "session closed"));
    };
    let (seq, at_send, notify) = pushed?;
    // Give the shell a moment to answer. A sent line waits for output that lands AFTER it
    // (what was already buffered is the previous command's late output and comes along, but
    // is not the answer); an empty line is the UI polling and takes anything undelivered.
    // The notify may hold a stale permit, so the wait is a loop that re-checks, not one await.
    let sent = !body.line.is_empty();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    loop {
        let ready = a
            .hub
            .with_stream(&sid, |s| {
                s.closed || s.next_seq() > if sent { at_send } else { seq }
            })
            .unwrap_or(true);
        if ready {
            break;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let _ = tokio::time::timeout(left, notify.notified()).await;
    }
    let out = a.hub.with_stream(&sid, |s| {
        let chunks = s.since(seq);
        s.delivered_seq = s.next_seq();
        let mut lines = Vec::new();
        for ch in chunks {
            match ch {
                StreamChunk::Exec { text } => lines.push(text),
                StreamChunk::End { error: Some(e), .. } => {
                    lines.push(format!("[session ended: {e}]"))
                }
                _ => {}
            }
        }
        ExecOutput {
            lines,
            exit_code: s.exit_code,
            closed: s.closed,
            answered: s.next_seq() > at_send,
        }
    });
    Ok(Json(out.unwrap_or(ExecOutput {
        lines: vec![],
        exit_code: None,
        closed: true,
        answered: false,
    })))
}

pub(crate) async fn close_exec(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(sid): Path<String>,
) -> Result<StatusCode, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Exec)?;
    if let Some(s) = a.hub.close_stream(&sid) {
        let secs = (super::now_ms() - s.opened_ms) / 1000;
        a.audit(
            &me,
            "container.exec",
            AuditTarget {
                host: Some(s.host.clone()),
                container: Some(s.container_name.clone()),
                ..Default::default()
            },
            AuditResultKind::Ok,
            Some(format!(
                "shell closed after {secs}s, {} command{}",
                s.commands,
                if s.commands == 1 { "" } else { "s" }
            )),
            None,
        )
        .await;
        a.hub.wake(&s.host);
    }
    Ok(StatusCode::NO_CONTENT)
}

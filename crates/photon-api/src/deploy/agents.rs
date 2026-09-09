//! Managed hosts (R1): enrol (mint a token, shown once), read, patch, rotate, remove, ask for
//! an upgrade, and the SSH bootstrap job (R1.9). Plus `GET /api/agent/download` — the server
//! serves the agent binary itself so an air-gapped site never needs the internet (R1.9.3).

use super::{api, bad, not_found, require, store_err, ApiErr, DeployApi};
use crate::auth::CurrentUser;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use photon_deploy::ids::{mint_token, new_id, sha256_hex};
use photon_deploy::model::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const MIN_POLL_SECS: u32 = 5;
pub const DEFAULT_POLL_SECS: u32 = 15;

fn valid_host_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
}

fn valid_site(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn normalize_caps(caps: &[AgentCapability]) -> Vec<AgentCapability> {
    let mut v: Vec<AgentCapability> = caps.to_vec();
    if !v.is_empty() && !v.contains(&AgentCapability::Read) {
        v.push(AgentCapability::Read);
    }
    v.sort();
    v.dedup();
    v
}

impl DeployApi {
    /// Enrol a host: mint its token, store the hash, return the token ONCE.
    pub async fn enroll(
        &self,
        me: &CurrentUser,
        input: &EnrollInput,
    ) -> Result<(ManagedHost, String), ApiErr> {
        let host = input.host.trim();
        if !valid_host_name(host) {
            return Err(bad(
                "host name must be 1–253 characters of letters, digits, '-', '.' or '_'",
            ));
        }
        if let Some(site) = input.site.as_deref().filter(|s| !s.trim().is_empty()) {
            if !valid_site(site.trim()) {
                return Err(bad("site must be letters, digits, '-', '_' or '.'"));
            }
        }
        if self
            .store
            .get_host(host)
            .await
            .map_err(store_err)?
            .is_some()
        {
            return Err(super::err(
                StatusCode::CONFLICT,
                format!("{host} is already enrolled — rotate its token instead"),
            ));
        }
        let poll = input
            .poll_secs
            .unwrap_or(DEFAULT_POLL_SECS)
            .max(MIN_POLL_SECS);
        let token = mint_token();
        let rec = HostRecord {
            id: new_id("mh"),
            host: host.to_string(),
            site: input
                .site
                .as_deref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            token_hash: sha256_hex(token.as_bytes()),
            requested_capabilities: normalize_caps(&input.capabilities),
            reported_capabilities: vec![],
            agent_version: None,
            upgrade_to: None,
            poll_secs: poll,
            last_seen_ms: None,
            enrolled_ms: super::now_ms(),
            docker: None,
            host_id: None,
            arch: None,
            os: None,
            labels: input.labels.clone(),
            desired_generation: 0,
        };
        self.store.put_host(&rec).await.map_err(store_err)?;
        self.audit(
            me,
            "agent.enroll",
            AuditTarget {
                host: Some(host.to_string()),
                ..Default::default()
            },
            AuditResultKind::Ok,
            Some(format!(
                "capabilities: {}",
                rec.requested_capabilities
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            None,
        )
        .await;
        Ok((self.host_view(&rec), token))
    }
}

pub(crate) async fn list(State(state): State<AppState>) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    Ok(Json(json!({ "hosts": a.host_views().await? })))
}

pub(crate) async fn get(
    State(state): State<AppState>,
    Path(host): Path<String>,
) -> Result<Json<ManagedHost>, ApiErr> {
    let a = api(&state)?;
    let rec = a.host(&host).await?;
    Ok(Json(a.host_view(&rec)))
}

/// The origin agents should dial: `[deploy].server_origin` when configured, else the request's
/// own `Host` (+ `X-Forwarded-Proto`). Shared by the sync upgrade URL, the install snippet and
/// the SSH bootstrap.
pub(crate) fn request_base(a: &DeployApi, headers: &HeaderMap) -> String {
    if let Some(o) = &a.server_origin {
        return o.trim_end_matches('/').to_string();
    }
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost:8080");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("http");
    format!("{proto}://{host}")
}

/// What the install snippet needs beyond the token, so the UI never has to guess it from its
/// own origin: the URL the host should dial (a Vite dev server on :5173 is NOT where the sync
/// endpoint lives, and neither is a reverse proxy's internal name), and which architectures
/// this server can hand a binary to — a snippet whose first line would 404 is worse than one
/// that says so up front.
fn install_context(a: &DeployApi, headers: &HeaderMap) -> Value {
    json!({
        "server_url": request_base(a, headers),
        "agent_archs": a.available_agent_archs(),
    })
}

pub(crate) async fn enroll(
    State(state): State<AppState>,
    me: CurrentUser,
    headers: HeaderMap,
    Json(input): Json<EnrollInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Agents)?;
    let (host, token) = a.enroll(&me, &input).await?;
    let mut out = json!({ "ok": true, "host": host, "token": token });
    merge(&mut out, install_context(a, &headers));
    Ok(Json(out))
}

fn merge(into: &mut Value, extra: Value) {
    if let (Some(dst), Some(src)) = (into.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
}

pub(crate) async fn patch(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(host): Path<String>,
    Json(p): Json<AgentPatch>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Agents)?;
    let mut rec = a.host(&host).await?;
    let mut changes = Vec::new();
    if let Some(poll) = p.poll_secs {
        let poll = poll.max(MIN_POLL_SECS);
        if poll != rec.poll_secs {
            changes.push(format!("poll {}s → {}s", rec.poll_secs, poll));
            rec.poll_secs = poll;
        }
    }
    if let Some(labels) = p.labels {
        if labels != rec.labels {
            changes.push(format!(
                "labels: {}",
                labels
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
            rec.labels = labels;
        }
    }
    if let Some(site) = p.site {
        let site = site.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(s) = &site {
            if !valid_site(s) {
                return Err(bad("site must be letters, digits, '-', '_' or '.'"));
            }
        }
        if site != rec.site {
            changes.push(format!("site → {}", site.as_deref().unwrap_or("(none)")));
            rec.site = site;
        }
    }
    let (poll_secs, labels, site) = (rec.poll_secs, rec.labels.clone(), rec.site.clone());
    a.store
        .update_host(
            &host,
            Box::new(move |r| {
                r.poll_secs = poll_secs;
                r.labels = labels;
                r.site = site;
            }),
        )
        .await
        .map_err(store_err)?;
    a.hub.wake(&host);
    if !changes.is_empty() {
        a.audit(
            &me,
            "agent.update",
            AuditTarget {
                host: Some(host.clone()),
                ..Default::default()
            },
            AuditResultKind::Ok,
            Some(changes.join(" · ")),
            None,
        )
        .await;
    }
    Ok(Json(json!({ "ok": true, "host": a.host_view(&rec) })))
}

pub(crate) async fn rotate(
    State(state): State<AppState>,
    me: CurrentUser,
    headers: HeaderMap,
    Path(host): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Agents)?;
    let mut rec = a.host(&host).await?;
    let token = mint_token();
    rec.token_hash = sha256_hex(token.as_bytes());
    let hash = rec.token_hash.clone();
    a.store
        .update_host(&host, Box::new(move |r| r.token_hash = hash))
        .await
        .map_err(store_err)?;
    a.audit(
        &me,
        "agent.rotate_token",
        AuditTarget {
            host: Some(host.clone()),
            ..Default::default()
        },
        AuditResultKind::Ok,
        Some("the previous token stops working at the agent's next sync".into()),
        None,
    )
    .await;
    let mut out = json!({ "ok": true, "token": token });
    merge(&mut out, install_context(a, &headers));
    Ok(Json(out))
}

pub(crate) async fn remove(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(host): Path<String>,
) -> Result<StatusCode, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Agents)?;
    let _ = a.host(&host).await?;
    // Stacks on the host stay as history but are no longer desired anywhere.
    for s in a.store.list_stacks().await.map_err(store_err)? {
        if s.host == host && s.status != StackStatus::Gone {
            let mut s = s;
            s.status = StackStatus::Gone;
            s.updated_ms = super::now_ms();
            a.store.put_stack(&s).await.map_err(store_err)?;
        }
    }
    a.store.delete_host(&host).await.map_err(store_err)?;
    a.audit(
        &me,
        "agent.remove",
        AuditTarget {
            host: Some(host.clone()),
            ..Default::default()
        },
        AuditResultKind::Ok,
        Some("containers keep running; the agent's token is revoked".into()),
        None,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn upgrade(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(host): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Agents)?;
    let mut rec = a.host(&host).await?;
    let Some(latest) = a.latest_agent_version.clone() else {
        return Err(bad("this server has no agent binary to upgrade to — place one under the agent binaries directory"));
    };
    if rec.agent_version.as_deref() == Some(latest.as_str()) {
        return Err(bad(format!("{host} already runs {latest}")));
    }
    if a.agent_binary_path(rec.arch.as_deref().unwrap_or("x86_64"))
        .is_none()
    {
        return Err(bad(format!("no agent binary for {} is available on this server — place one under the agent binaries directory", rec.arch.as_deref().unwrap_or("x86_64"))));
    }
    rec.upgrade_to = Some(latest.clone());
    let to = latest.clone();
    a.store
        .update_host(&host, Box::new(move |r| r.upgrade_to = Some(to)))
        .await
        .map_err(store_err)?;
    a.hub.wake(&host);
    a.audit(
        &me,
        "agent.upgrade",
        AuditTarget {
            host: Some(host.clone()),
            ..Default::default()
        },
        AuditResultKind::Pending,
        Some(format!(
            "{} → {latest} on the next sync",
            rec.agent_version.as_deref().unwrap_or("?")
        )),
        None,
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

// --- SSH bootstrap ---

pub(crate) async fn start_bootstrap(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(input): Json<SshBootstrapInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Agents)?;
    let Some(runner) = a.bootstrapper.clone() else {
        return Err(bad("SSH bootstrap is not available on this server — install the agent with the snippet instead"));
    };
    let addresses: Vec<String> = input
        .addresses
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if addresses.is_empty() {
        return Err(bad("at least one address is required"));
    }
    if input.username.trim().is_empty() {
        return Err(bad("an SSH username is required"));
    }
    if addresses.len() > 1
        && input
            .host_name
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    {
        return Err(bad("a host name can only be given for a single address"));
    }
    let job = BootstrapJob {
        id: new_id("bj"),
        status: JobStatus::Running,
        started_ms: super::now_ms(),
        credentials_kept: input.remember_credentials,
        hosts: addresses
            .iter()
            .map(|addr| BootstrapHostProgress::new(addr))
            .collect(),
    };
    a.hub.put_job(job.clone());
    a.audit(
        &me,
        "agent.bootstrap",
        AuditTarget {
            host: input
                .host_name
                .clone()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| Some(addresses.join(", "))),
            ..Default::default()
        },
        AuditResultKind::Pending,
        Some(format!(
            "{} address{} over SSH as {}",
            addresses.len(),
            if addresses.len() == 1 { "" } else { "es" },
            input.username.trim()
        )),
        None,
    )
    .await;
    let ctx = super::BootstrapCtx {
        api: a.clone(),
        server_origin: a.server_origin.clone().unwrap_or_default(),
    };
    runner.start(
        job.id.clone(),
        SshBootstrapInput { addresses, ..input },
        ctx,
    );
    Ok(Json(json!({ "ok": true, "job": job })))
}

pub(crate) async fn bootstrap_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<BootstrapJob>, ApiErr> {
    let a = api(&state)?;
    a.hub
        .job(&id)
        .map(Json)
        .ok_or_else(|| not_found("no such bootstrap job"))
}

// --- agent binary download ---

#[derive(Debug, Default, Deserialize)]
pub struct DownloadParams {
    #[serde(default)]
    pub arch: Option<String>,
}

impl DeployApi {
    /// Where the agent binary for `arch` lives on this server, if anywhere: the configured
    /// binaries directory (`photon-agent-<arch>`), else — for this server's own architecture —
    /// a `photon-agent` sitting beside the running `photon-server`.
    pub fn agent_binary_path(&self, arch: &str) -> Option<std::path::PathBuf> {
        let arch = normalize_arch(arch);
        if let Some(dir) = &self.agent_binaries_dir {
            for name in [
                format!("photon-agent-{arch}"),
                format!("photon-agent-{arch}-unknown-linux-gnu"),
                format!("photon-agent-{arch}-unknown-linux-musl"),
            ] {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        if normalize_arch(std::env::consts::ARCH) == arch {
            if let Ok(exe) = std::env::current_exe() {
                let sib = exe.with_file_name("photon-agent");
                if sib.is_file() {
                    return Some(sib);
                }
            }
        }
        None
    }

    /// The architectures `GET /api/agent/download` can answer for right now. The install
    /// snippet shows this so an operator learns "no aarch64 binary here" from the panel,
    /// not from a 404 on the host.
    pub fn available_agent_archs(&self) -> Vec<String> {
        KNOWN_ARCHS
            .iter()
            .filter(|a| self.agent_binary_path(a).is_some())
            .map(|a| a.to_string())
            .collect()
    }
}

/// The architectures the release pipeline can produce; `available_agent_archs` probes each.
const KNOWN_ARCHS: &[&str] = &["x86_64", "aarch64", "armv7"];

/// `uname -m` and Rust spell architectures differently; the download route accepts both.
pub fn normalize_arch(a: &str) -> String {
    match a.trim() {
        "amd64" | "x64" => "x86_64".into(),
        "arm64" => "aarch64".into(),
        "armv7l" | "armhf" => "armv7".into(),
        other => other.to_string(),
    }
}

/// `GET /api/agent/download?arch=` — open (the install snippet runs it before any token
/// exists). Serves a static binary; nothing here is secret.
pub(crate) async fn download(
    State(state): State<AppState>,
    Query(p): Query<DownloadParams>,
) -> Response {
    let Some(a) = state.deploy.as_ref() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "deploy subsystem disabled" })),
        )
            .into_response();
    };
    let arch = p.arch.unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let Some(path) = a.agent_binary_path(&arch) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("no agent binary for {arch} on this server; build photon-agent and place it under the agent binaries directory") }))).into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let digest = sha256_hex(&bytes);
            (
                [
                    (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        "attachment; filename=\"photon-agent\"".to_string(),
                    ),
                    (header::ETAG, format!("\"{digest}\"")),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("reading {}: {e}", path.display()) })),
        )
            .into_response(),
    }
}

/// The digest the sync response advertises beside an upgrade URL.
pub async fn agent_binary_digest(path: &std::path::Path) -> Option<String> {
    tokio::fs::read(path).await.ok().map(|b| sha256_hex(&b))
}

#[allow(dead_code)]
fn _labels_type_check(_: BTreeMap<String, String>) {}

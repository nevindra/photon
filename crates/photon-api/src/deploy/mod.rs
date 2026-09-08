//! Docker fleet management — the HTTP layer over `photon_deploy` (see `docs/subsystems/deploy.md`).
//!
//! Two auth systems side by side: the session cookie + role for everything an operator clicks
//! (`agents.rs`, `inventory.rs`, `actions.rs`, `stacks.rs`, `templates.rs`, `rollouts.rs`,
//! `audit.rs`, `image_registries.rs`), and the per-host agent token for the control channel the
//! agent polls (`sync.rs`). `hub.rs` is the in-memory side: per-host wake-ups for the long-poll, stream
//! sessions (logs / stats / exec), event rings, bootstrap jobs. `supervisor.rs` is the periodic
//! tick photon-server spawns: rollout advancement, action expiry, stream expiry.
//!
//! Persistence is `photon_deploy::store::DeployStore` (photon-server's SQLite); secret values
//! pass through the `SecretCipher` seam so the store only ever holds ciphertext (R8.5).

pub mod actions;
pub mod agents;
pub mod audit;
pub mod hub;
pub mod image_registries;
pub mod inventory;
pub mod rollouts;
pub mod stacks;
pub mod supervisor;
pub mod sync;
pub mod templates;

use crate::auth::CurrentUser;
use crate::AppState;
use axum::http::StatusCode;
use axum::Json;
use photon_core::PhotonError;
use photon_deploy::model::*;
use photon_deploy::store::{DeployStore, ObservedRecord};
use serde_json::json;
use std::sync::Arc;

pub use hub::DeployHub;

/// Encrypt-at-rest for secret variable values (R8.5). Implemented in photon-server with AES-GCM
/// under a key that is not the session secret; the API never sees where the key lives.
pub trait SecretCipher: Send + Sync {
    /// Plaintext → opaque blob (nonce + ciphertext, base64).
    fn encrypt(&self, plaintext: &str) -> String;
    /// The inverse; `Err` when the blob was written under another key or is corrupt.
    fn decrypt(&self, blob: &str) -> Result<String, String>;
}

/// The SSH bootstrap runner (R1.9): photon-server implements it with an SSH client; the API
/// records progress in the hub and enrols the host once the agent's first sync lands.
pub trait Bootstrapper: Send + Sync {
    /// Start one job in the background. `ctx` carries everything the runner needs to enrol.
    fn start(&self, job_id: String, input: SshBootstrapInput, ctx: BootstrapCtx);
}

/// What a bootstrap run needs from the server side: the API to enrol with, and the agent
/// binary to push.
#[derive(Clone)]
pub struct BootstrapCtx {
    pub api: DeployApi,
    /// The URL agents should dial (this Photon as the customer LAN sees it).
    pub server_origin: String,
}

/// Where the server's own OTLP door is, for the sync response's ingest hint.
#[derive(Debug, Clone)]
pub struct IngestHint {
    pub metrics_endpoint: String,
    pub logs_endpoint: String,
    pub token: String,
}

#[derive(Clone)]
pub struct DeployApi {
    pub store: Arc<dyn DeployStore>,
    pub cipher: Arc<dyn SecretCipher>,
    pub hub: Arc<DeployHub>,
    pub bootstrapper: Option<Arc<dyn Bootstrapper>>,
    /// Directory holding `photon-agent-<arch>` binaries for `GET /api/agent/download` (R1.9.3).
    pub agent_binaries_dir: Option<std::path::PathBuf>,
    pub ingest: Option<IngestHint>,
    /// The version this server would advertise to agents (its own).
    /// The agent version this server can hand out (read off the bundled agent binary at
    /// startup); `None` when no binary is available, so no host is ever told to upgrade to
    /// a version that cannot be downloaded.
    pub latest_agent_version: Option<String>,
    /// The origin agents should dial, when configured; else derived from the request.
    pub server_origin: Option<String>,
    /// Release markers (R7.1).
    pub deployments: Option<Arc<dyn crate::deployments::DeploymentStore>>,
}

pub type ApiErr = (StatusCode, Json<serde_json::Value>);

pub fn err(code: StatusCode, msg: impl std::fmt::Display) -> ApiErr {
    (code, Json(json!({ "error": msg.to_string() })))
}

pub fn internal(e: impl std::fmt::Display) -> ApiErr {
    err(StatusCode::INTERNAL_SERVER_ERROR, e)
}

pub fn bad(e: impl std::fmt::Display) -> ApiErr {
    err(StatusCode::BAD_REQUEST, e)
}

pub fn not_found(e: impl std::fmt::Display) -> ApiErr {
    err(StatusCode::NOT_FOUND, e)
}

pub fn store_err(e: PhotonError) -> ApiErr {
    internal(e)
}

/// The "subsystem disabled ⇒ 404 with a JSON body" accessor every handler starts with. A JSON
/// body is deliberate: the frontend reads an EMPTY 404 as "this backend predates the subsystem"
/// and a JSON one as a real answer.
pub(crate) fn api(state: &AppState) -> Result<&DeployApi, ApiErr> {
    state
        .deploy
        .as_ref()
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "deploy subsystem disabled"))
}

/// The role gate, as the `ApiErr` the deploy handlers return.
pub(crate) fn require(me: &CurrentUser, perm: Permission) -> Result<(), ApiErr> {
    if me.role.allows(perm) {
        Ok(())
    } else {
        let label = match Role::required(perm) {
            Role::Viewer => "Viewer",
            Role::Operator => "Operator",
            Role::Admin => "Admin",
        };
        Err(err(
            StatusCode::FORBIDDEN,
            format!("Requires the {label} role"),
        ))
    }
}

pub fn now_ms() -> i64 {
    photon_deploy::now_ms()
}

impl DeployApi {
    /// Append one audit row. Never fails the caller: an audit write error is logged, because
    /// the operation it describes already happened.
    pub async fn audit(
        &self,
        me: &CurrentUser,
        action: &str,
        target: AuditTarget,
        result: AuditResultKind,
        detail: Option<String>,
        payload: Option<&[u8]>,
    ) {
        let e = AuditEntry {
            id: photon_deploy::ids::new_id("au"),
            ts_ms: now_ms(),
            actor: me.username.clone(),
            role: me.role,
            action: action.to_string(),
            target,
            result,
            detail,
            payload_digest: payload.map(photon_deploy::ids::payload_digest),
        };
        if let Err(e) = self.store.append_audit(&e).await {
            eprintln!("photon-api: audit write failed: {e}");
        }
    }

    /// An audit row written on the agent's behalf (a result coming back over sync).
    pub async fn audit_system(
        &self,
        actor: &str,
        role: Role,
        action: &str,
        target: AuditTarget,
        result: AuditResultKind,
        detail: Option<String>,
    ) {
        let e = AuditEntry {
            id: photon_deploy::ids::new_id("au"),
            ts_ms: now_ms(),
            actor: actor.to_string(),
            role,
            action: action.to_string(),
            target,
            result,
            detail,
            payload_digest: None,
        };
        if let Err(e) = self.store.append_audit(&e).await {
            eprintln!("photon-api: audit write failed: {e}");
        }
    }

    pub async fn host(&self, host: &str) -> Result<HostRecord, ApiErr> {
        self.store
            .get_host(host)
            .await
            .map_err(store_err)?
            .ok_or_else(|| not_found(format!("{host} is not enrolled")))
    }

    pub fn host_view(&self, rec: &HostRecord) -> ManagedHost {
        rec.view(now_ms(), self.latest_agent_version.as_deref())
    }

    pub async fn host_views(&self) -> Result<Vec<ManagedHost>, ApiErr> {
        let mut hosts: Vec<ManagedHost> = self
            .store
            .list_hosts()
            .await
            .map_err(store_err)?
            .iter()
            .map(|h| self.host_view(h))
            .collect();
        hosts.sort_by(|a, b| a.host.cmp(&b.host));
        Ok(hosts)
    }

    pub async fn observed(&self, host: &str) -> Result<Option<ObservedRecord>, ApiErr> {
        self.store.get_observed(host).await.map_err(store_err)
    }

    /// Bump a host's desired generation and wake its long-poll: something on it changed.
    pub async fn bump_host(&self, host: &str) -> Result<(), ApiErr> {
        self.store
            .update_host(host, Box::new(|r| r.desired_generation += 1))
            .await
            .map_err(store_err)?;
        self.hub.wake(host);
        Ok(())
    }

    /// A stack as the UI reads it: services and drift resolved against the host's inventory.
    pub async fn stack_view(&self, stack: &Stack) -> Result<Stack, ApiErr> {
        let obs = self.observed(&stack.host).await?;
        Ok(stacks::view(stack, obs.as_ref()))
    }

    /// Decrypt a stack's secret values for the agent (`POST /api/agent/secrets`).
    pub async fn secret_values(
        &self,
        stack_id: &str,
    ) -> Result<std::collections::BTreeMap<String, String>, ApiErr> {
        let mut out = std::collections::BTreeMap::new();
        if let Some(blob) = self.store.get_secrets(stack_id).await.map_err(store_err)? {
            for (k, v) in blob.values {
                match self.cipher.decrypt(&v) {
                    Ok(p) => {
                        out.insert(k, p);
                    }
                    Err(e) => return Err(internal(format!("secret {k} cannot be decrypted: {e}"))),
                }
            }
        }
        Ok(out)
    }
}

/// A `SecretCipher` that stores plaintext with a marker — for tests only, never wired by
/// photon-server.
pub struct PlainCipher;

impl SecretCipher for PlainCipher {
    fn encrypt(&self, plaintext: &str) -> String {
        format!("plain:{plaintext}")
    }
    fn decrypt(&self, blob: &str) -> Result<String, String> {
        blob.strip_prefix("plain:")
            .map(|s| s.to_string())
            .ok_or_else(|| "not a plain blob".into())
    }
}

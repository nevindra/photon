//! The agent sync wire types (section 7.4 of the spec): one request shape, one response shape,
//! shared byte-for-byte by photon-api (the server side) and photon-agent (the client side).
//! Everything the agent does is one periodic `POST /api/agent/sync` carrying what I am, what I
//! see, and what I did, and returning what I should be. Streams (logs, stats, exec) are
//! agent-initiated uploads to `POST /api/agent/stream/:session`, same direction, same token.

use crate::model::{
    ActionArgs, ActionKind, ActionStatus, AgentCapability, DeployOptions, DockerEvent, Observed,
    StackFile, StackOp,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The protocol version this crate speaks. The server supports at least one older version
/// (R9.4); today there is only v1.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostIdentity {
    pub name: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub os: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncRequest {
    pub protocol: u32,
    pub agent_version: String,
    pub host: HostIdentity,
    /// What the operator allowed locally (the agent's own flags). Reported, never assigned.
    #[serde(default)]
    pub capabilities: Vec<AgentCapability>,
    /// The desired generation the agent has already applied; the server answers `desired: null`
    /// when nothing changed since.
    #[serde(default)]
    pub known_generation: u64,
    #[serde(default)]
    pub observed: Option<Observed>,
    #[serde(default)]
    pub results: Vec<ActionResult>,
    #[serde(default)]
    pub stack_results: Vec<StackResult>,
    /// Docker daemon events since the last sync (R2.4).
    #[serde(default)]
    pub events: Vec<DockerEvent>,
    /// Long-poll: hold the response up to this many seconds until there is work. `0` answers
    /// immediately. Interactive latency without a second port or a faster poll (R1.6).
    #[serde(default)]
    pub wait_secs: u32,
    /// Sessions the agent is currently streaming, so the server can tell it which stopped.
    #[serde(default)]
    pub active_streams: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    pub action_id: String,
    pub status: ActionStatus,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub started_ms: Option<i64>,
    #[serde(default)]
    pub finished_ms: Option<i64>,
}

/// What happened when the agent applied one desired stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackResult {
    pub stack_id: String,
    pub generation: u64,
    pub op: StackOp,
    pub ok: bool,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub finished_ms: i64,
    /// `com.docker.compose.config-hash` per service right after a successful apply — the
    /// drift baseline.
    #[serde(default)]
    pub config_hashes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncResponse {
    /// The server's current desired generation for this host.
    pub generation: u64,
    /// `None` = nothing changed since `known_generation`.
    #[serde(default)]
    pub desired: Option<Desired>,
    #[serde(default)]
    pub actions: Vec<ActionSpec>,
    #[serde(default)]
    pub streams: Vec<StreamSpec>,
    pub next_poll_secs: u32,
    #[serde(default)]
    pub upgrade: Option<UpgradeSpec>,
    /// Capabilities the server would like to use, for the agent's log line — never authority.
    #[serde(default)]
    pub wanted_capabilities: Vec<AgentCapability>,
    /// The server's wall clock, so an agent with a skewed clock (R9.3) can timestamp uniformly.
    pub server_time_ms: i64,
    /// Where this agent should send its resource metrics (and container stats, R2.3): the
    /// server's own OTLP/HTTP door plus the ingest token, so a host enrolled with one agent token
    /// needs nothing else. Absent = keep whatever `--endpoint` says.
    #[serde(default)]
    pub ingest: Option<IngestSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestSpec {
    /// e.g. `http://photon.example:4318/v1/metrics`
    pub metrics_endpoint: String,
    /// e.g. `http://photon.example:4318/v1/logs` (Docker events, R2.4)
    pub logs_endpoint: String,
    pub token: String,
}

/// The desired-state document: every stack this host should be running, rendered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Desired {
    pub generation: u64,
    #[serde(default)]
    pub stacks: Vec<DesiredStack>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesiredStack {
    pub id: String,
    pub name: String,
    pub generation: u64,
    /// The one-shot operation this generation asks for (`Up` for steady state).
    pub op: StackOp,
    /// Rendered files. Secrets are NOT here: `.env` lists their names with no value and the agent
    /// resolves them via `POST /api/agent/secrets` at apply time (R5.9).
    pub files: Vec<StackFile>,
    #[serde(default)]
    pub secret_names: Vec<String>,
    pub deploy_options: DeployOptions,
    /// `destroy` with volumes.
    #[serde(default)]
    pub remove_volumes: bool,
    /// Pre-pull images before the apply (R6.6).
    #[serde(default)]
    pub prepull: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpec {
    pub id: String,
    pub kind: ActionKind,
    #[serde(default)]
    pub container_id: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// `volume_remove`: the volume's name.
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default)]
    pub args: ActionArgs,
    pub expires_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    Logs,
    Stats,
    Exec,
}

/// A stream the server wants: the agent opens `POST /api/agent/stream/:session` and uploads
/// newline-delimited `StreamChunk`s until the session disappears from the sync response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamSpec {
    pub session: String,
    pub kind: StreamKind,
    pub container_id: String,
    #[serde(default)]
    pub tail: Option<u32>,
    #[serde(default)]
    pub since_ms: Option<i64>,
    #[serde(default)]
    pub shell: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum StreamChunk {
    Log {
        ts_ms: i64,
        stream: String,
        text: String,
    },
    Stat {
        ts_ms: i64,
        cpu_pct: Option<f64>,
        mem_bytes: Option<u64>,
        mem_limit_bytes: Option<u64>,
        net_rx_bps: Option<f64>,
        net_tx_bps: Option<f64>,
        block_read_bps: Option<f64>,
        block_write_bps: Option<f64>,
    },
    Exec {
        text: String,
    },
    /// The stream ended on the agent's side (container gone, exec exited, error).
    End {
        exit_code: Option<i64>,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpgradeSpec {
    pub version: String,
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretsRequest {
    pub stack_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretsResponse {
    pub values: BTreeMap<String, String>,
}

/// `POST /api/agent/registry-auth`: the credentials this host may pull with, for the named
/// registries (`servers` empty = every registry the host is scoped for). Asked right before a
/// pull and held in memory for that pull only — the agent never `docker login`s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RegistryAuthRequest {
    #[serde(default)]
    pub servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RegistryAuthResponse {
    #[serde(default)]
    pub credentials: Vec<RegistryAuth>,
}

/// One pull credential, decrypted, on the wire to the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryAuth {
    /// Normalized registry host (`registry.gitlab.com`, `docker.io`).
    pub server: String,
    pub username: String,
    pub secret: String,
}

/// Exec input, long-polled by the agent from `GET /api/agent/stream/:session/input`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ExecInput {
    #[serde(default)]
    pub lines: Vec<String>,
    #[serde(default)]
    pub closed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_state_response_is_small() {
        let r = SyncResponse {
            generation: 7,
            desired: None,
            actions: vec![],
            streams: vec![],
            next_poll_secs: 15,
            upgrade: None,
            wanted_capabilities: vec![],
            server_time_ms: 0,
            ingest: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"desired\":null"));
        let back: SyncResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn chunks_are_tagged() {
        let c = StreamChunk::Log {
            ts_ms: 1,
            stream: "stdout".into(),
            text: "hi".into(),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.starts_with("{\"t\":\"log\""));
        let e: StreamChunk =
            serde_json::from_str(r#"{"t":"end","exit_code":0,"error":null}"#).unwrap();
        assert_eq!(
            e,
            StreamChunk::End {
                exit_code: Some(0),
                error: None
            }
        );
    }
}

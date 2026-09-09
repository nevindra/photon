//! The Deploy world's domain types, serialized exactly the way `frontend/src/lib/core/api.ts`
//! reads them. Field names mirror the control-plane tables; timestamps are epoch MILLISECONDS
//! (control-plane rows, like alerts/deployments), never the telemetry nanosecond strings.
//!
//! These are plain data. The rules that move them (rendering, drift, planning, state machines)
//! live in their own modules so each is a pure function over these shapes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------------------------

/// UI roles (R8.1). `viewer` is today's read-only behaviour; `operator` may run lifecycle actions
/// and deploy approved templates; `admin` may publish templates, manage agents and users, deploy
/// arbitrary compose, and open a shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Operator,
    Admin,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "viewer" => Some(Role::Viewer),
            "operator" => Some(Role::Operator),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }
    /// The role that may perform a permission (mirrors `frontend/src/lib/deploy/roles.ts`).
    pub fn required(perm: Permission) -> Role {
        match perm {
            Permission::Lifecycle | Permission::Deploy => Role::Operator,
            Permission::Audit => Role::Viewer,
            Permission::Templates
            | Permission::Agents
            | Permission::Compose
            | Permission::Exec
            | Permission::Users => Role::Admin,
        }
    }
    pub fn allows(self, perm: Permission) -> bool {
        self >= Role::required(perm)
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a control does, in the vocabulary the UI gates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Start/stop/restart/kill/remove/pull/recreate a container, prune.
    Lifecycle,
    /// Deploy a template, apply/restart/redeploy/destroy a stack, run a rollout.
    Deploy,
    /// Publish template versions, manage registries.
    Templates,
    /// Enroll, rotate, remove agents; change poll interval.
    Agents,
    /// Deploy arbitrary compose (not from a template).
    Compose,
    /// Shell into a container.
    Exec,
    /// Manage UI users and roles.
    Users,
    /// Read the audit log.
    Audit,
}

// ---------------------------------------------------------------------------------------------
// Managed hosts (agents)
// ---------------------------------------------------------------------------------------------

/// What an agent ADMITS to, reported on every sync — never assigned from the server (R1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AgentCapability {
    Read,
    Lifecycle,
    Apply,
    Exec,
}

impl AgentCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentCapability::Read => "read",
            AgentCapability::Lifecycle => "lifecycle",
            AgentCapability::Apply => "apply",
            AgentCapability::Exec => "exec",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Online,
    Offline,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DockerDisk {
    #[serde(default)]
    pub images_bytes: u64,
    #[serde(default)]
    pub volumes_bytes: u64,
    #[serde(default)]
    pub build_cache_bytes: u64,
    #[serde(default)]
    pub reclaimable_bytes: u64,
    #[serde(default)]
    pub total_bytes: Option<u64>,
    #[serde(default)]
    pub used_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DockerFacts {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub api_version: String,
    #[serde(default)]
    pub storage_driver: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub containers_running: u32,
    #[serde(default)]
    pub containers_total: u32,
    #[serde(default)]
    pub images: u32,
    #[serde(default)]
    pub disk: DockerDisk,
}

/// A host under management: the `managed_hosts` row as the UI sees it. The token itself is never
/// here — only its hash lives in the store, and the hash never leaves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedHost {
    pub id: String,
    pub host: String,
    pub site: Option<String>,
    pub status: AgentStatus,
    pub agent_version: Option<String>,
    pub latest_agent_version: Option<String>,
    pub capabilities: Vec<AgentCapability>,
    pub poll_secs: u32,
    pub last_seen_ms: Option<i64>,
    pub enrolled_ms: i64,
    pub docker: Option<DockerFacts>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// The stored shape behind `ManagedHost`: everything the UI sees plus what it must not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostRecord {
    pub id: String,
    pub host: String,
    pub site: Option<String>,
    /// sha256 hex of the `ak_…` token.
    pub token_hash: String,
    /// Capabilities the operator ASKED for at enrolment (the install snippet); what the agent
    /// admits to is `reported_capabilities`, refreshed every sync.
    pub requested_capabilities: Vec<AgentCapability>,
    pub reported_capabilities: Vec<AgentCapability>,
    pub agent_version: Option<String>,
    /// Set by `POST /api/agents/:host/upgrade`; advertised in the sync response until the agent
    /// reports that version (R1.10).
    pub upgrade_to: Option<String>,
    pub poll_secs: u32,
    pub last_seen_ms: Option<i64>,
    pub enrolled_ms: i64,
    pub docker: Option<DockerFacts>,
    /// Opaque agent identity (`host.id` from the machine), for the metrics tie-in.
    pub host_id: Option<String>,
    pub arch: Option<String>,
    pub os: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// The generation counter of this host's desired-state document: bumped whenever any stack on
    /// it changes, so a current agent gets `desired: null` (cheap steady state).
    pub desired_generation: u64,
}

impl HostRecord {
    /// Online = seen within two poll intervals (plus long-poll slack); pending = never seen.
    pub fn status(&self, now_ms: i64) -> AgentStatus {
        match self.last_seen_ms {
            None => AgentStatus::Pending,
            Some(seen) => {
                let window = (self.poll_secs as i64 * 2 + 30) * 1000;
                if now_ms - seen <= window {
                    AgentStatus::Online
                } else {
                    AgentStatus::Offline
                }
            }
        }
    }

    pub fn capabilities(&self) -> Vec<AgentCapability> {
        if self.last_seen_ms.is_some() {
            self.reported_capabilities.clone()
        } else {
            self.requested_capabilities.clone()
        }
    }

    pub fn view(&self, now_ms: i64, latest_agent_version: Option<&str>) -> ManagedHost {
        ManagedHost {
            id: self.id.clone(),
            host: self.host.clone(),
            site: self.site.clone(),
            status: self.status(now_ms),
            agent_version: self.agent_version.clone(),
            latest_agent_version: latest_agent_version.map(|s| s.to_string()),
            capabilities: self.capabilities(),
            poll_secs: self.poll_secs,
            last_seen_ms: self.last_seen_ms,
            enrolled_ms: self.enrolled_ms,
            docker: self.docker.clone(),
            labels: self.labels.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnrollInput {
    pub host: String,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<AgentCapability>,
    #[serde(default)]
    pub poll_secs: Option<u32>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentPatch {
    #[serde(default)]
    pub poll_secs: Option<u32>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    /// `Some(None)` clears the site; absent leaves it.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub site: Option<Option<String>>,
}

fn deserialize_double_option<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(d)?))
}

// ---------------------------------------------------------------------------------------------
// SSH bootstrap (R1.9)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SshAuth {
    Password {
        password: String,
    },
    Key {
        private_key: String,
        #[serde(default)]
        passphrase: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct SshBootstrapInput {
    pub addresses: Vec<String>,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    #[serde(default)]
    pub host_name: Option<String>,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<AgentCapability>,
    #[serde(default)]
    pub poll_secs: Option<u32>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub remember_credentials: bool,
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BootstrapStepKey {
    Connect,
    Detect,
    Push,
    Configure,
    Start,
    Sync,
}

impl BootstrapStepKey {
    pub const ALL: [BootstrapStepKey; 6] = [
        BootstrapStepKey::Connect,
        BootstrapStepKey::Detect,
        BootstrapStepKey::Push,
        BootstrapStepKey::Configure,
        BootstrapStepKey::Start,
        BootstrapStepKey::Sync,
    ];
    pub fn label(self) -> &'static str {
        match self {
            BootstrapStepKey::Connect => "Connect over SSH",
            BootstrapStepKey::Detect => "Detect OS and architecture",
            BootstrapStepKey::Push => "Push the agent binary",
            BootstrapStepKey::Configure => "Write config and enrol",
            BootstrapStepKey::Start => "Install and start the service",
            BootstrapStepKey::Sync => "Wait for the first sync",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapStep {
    pub key: BootstrapStepKey,
    pub label: String,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapHostProgress {
    pub address: String,
    pub host: Option<String>,
    pub status: JobStatus,
    pub steps: Vec<BootstrapStep>,
    pub error: Option<String>,
}

impl BootstrapHostProgress {
    pub fn new(address: &str) -> Self {
        BootstrapHostProgress {
            address: address.to_string(),
            host: None,
            status: JobStatus::Running,
            steps: BootstrapStepKey::ALL
                .iter()
                .map(|k| BootstrapStep {
                    key: *k,
                    label: k.label().to_string(),
                    status: StepStatus::Pending,
                    detail: None,
                })
                .collect(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapJob {
    pub id: String,
    pub status: JobStatus,
    pub started_ms: i64,
    pub credentials_kept: bool,
    pub hosts: Vec<BootstrapHostProgress>,
}

// ---------------------------------------------------------------------------------------------
// Audit (R3.5 / R8.3)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditResultKind {
    Ok,
    Failed,
    Denied,
    Pending,
}

impl AuditResultKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditResultKind::Ok => "ok",
            AuditResultKind::Failed => "failed",
            AuditResultKind::Denied => "denied",
            AuditResultKind::Pending => "pending",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "failed" => Some(Self::Failed),
            "denied" => Some(Self::Denied),
            "pending" => Some(Self::Pending),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuditTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub ts_ms: i64,
    pub actor: String,
    pub role: Role,
    /// Dotted `noun.verb` — `container.restart`, `stack.apply`, `agent.enroll`, `rollout.promote`.
    pub action: String,
    pub target: AuditTarget,
    pub result: AuditResultKind,
    pub detail: Option<String>,
    /// Digest of the rendered payload; the payload itself is never in the log (secrets, R5.9).
    pub payload_digest: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub since_ms: Option<i64>,
    pub actor: Option<String>,
    /// A noun family (`stack`) or a full action (`stack.apply`).
    pub action: Option<String>,
    pub host: Option<String>,
    pub result: Option<AuditResultKind>,
    pub q: Option<String>,
    pub limit: usize,
}

// ---------------------------------------------------------------------------------------------
// Inventory (read-only Docker, R2)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerState {
    Running,
    Exited,
    Paused,
    Restarting,
    Created,
    Dead,
    Removing,
}

impl ContainerState {
    pub fn parse(s: &str) -> ContainerState {
        match s {
            "running" => ContainerState::Running,
            "paused" => ContainerState::Paused,
            "restarting" => ContainerState::Restarting,
            "created" => ContainerState::Created,
            "dead" => ContainerState::Dead,
            "removing" => ContainerState::Removing,
            _ => ContainerState::Exited,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ContainerState::Running => "running",
            ContainerState::Exited => "exited",
            ContainerState::Paused => "paused",
            ContainerState::Restarting => "restarting",
            ContainerState::Created => "created",
            ContainerState::Dead => "dead",
            ContainerState::Removing => "removing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerHealth {
    Healthy,
    Unhealthy,
    Starting,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerPort {
    pub host_ip: Option<String>,
    pub host_port: Option<u16>,
    pub container_port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerMount {
    #[serde(rename = "type")]
    pub kind: String,
    pub source: String,
    pub destination: String,
    pub rw: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
    pub command: String,
    pub state: ContainerState,
    pub health: ContainerHealth,
    /// Docker's own words ("Up 3 days (healthy)", "Exited (1) 2 hours ago") — shown verbatim.
    pub status_text: String,
    pub created_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub exit_code: Option<i64>,
    pub restart_count: u32,
    pub restart_policy: String,
    #[serde(default)]
    pub ports: Vec<ContainerPort>,
    #[serde(default)]
    pub mounts: Vec<ContainerMount>,
    #[serde(default)]
    pub networks: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Names only — values may be secrets and never leave the host.
    #[serde(default)]
    pub env_keys: Vec<String>,
    /// Values as the AGENT reports them: anything secret-looking is masked before it leaves the host.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    pub managed_stack_id: Option<String>,
    pub cpu_pct: Option<f64>,
    pub mem_bytes: Option<u64>,
    pub mem_limit_bytes: Option<u64>,
    pub net_rx_bps: Option<f64>,
    pub net_tx_bps: Option<f64>,
    pub block_read_bps: Option<f64>,
    pub block_write_bps: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub id: String,
    #[serde(default)]
    pub repo_tags: Vec<String>,
    pub size_bytes: u64,
    pub created_ms: i64,
    pub used_by: u32,
    pub dangling: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub used_by: Vec<String>,
    pub created_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub internal: bool,
    pub containers: u32,
    pub subnet: Option<String>,
}

/// What the agent observed on one sync: the same document shape the UI reads, minus the
/// server-side envelope (`agent_status`, `stale`, `managed_stack_id` resolution).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Observed {
    #[serde(default)]
    pub facts: Option<DockerFacts>,
    #[serde(default)]
    pub containers: Vec<Container>,
    #[serde(default)]
    pub images: Vec<ImageInfo>,
    #[serde(default)]
    pub volumes: Vec<VolumeInfo>,
    #[serde(default)]
    pub networks: Vec<NetworkInfo>,
    /// Compose projects the agent applied from desired state, with what it knows about each.
    #[serde(default)]
    pub stacks: Vec<ObservedStack>,
    /// What a prune would free, as Docker's `system df` estimates it (R3.3 preview).
    #[serde(default)]
    pub prune_preview: Option<PrunePreview>,
}

/// The agent's view of one compose project it manages (or found running).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedStack {
    pub name: String,
    /// The desired generation this project was last applied at (`None` = not applied by Photon).
    pub applied_generation: Option<u64>,
    /// `com.docker.compose.config-hash` per compose service, as Docker labels the containers.
    #[serde(default)]
    pub config_hashes: BTreeMap<String, String>,
    /// Compose working dir + file, when Docker labels carry them (adoption).
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub compose_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostInventory {
    pub host: String,
    pub agent_status: AgentStatus,
    pub fetched_ms: i64,
    pub stale: bool,
    pub containers: Vec<Container>,
    pub images: Vec<ImageInfo>,
    pub volumes: Vec<VolumeInfo>,
    pub networks: Vec<NetworkInfo>,
    pub facts: Option<DockerFacts>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerLogLine {
    pub ts_ms: i64,
    pub stream: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerLogsResult {
    pub lines: Vec<ContainerLogLine>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatPoint {
    pub t: i64,
    pub v: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ContainerStats {
    pub cpu_pct: Vec<StatPoint>,
    pub mem_bytes: Vec<StatPoint>,
    pub net_rx_bps: Vec<StatPoint>,
    pub net_tx_bps: Vec<StatPoint>,
    pub block_read_bps: Vec<StatPoint>,
    pub block_write_bps: Vec<StatPoint>,
    pub mem_limit_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockerEvent {
    pub id: String,
    pub ts_ms: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub action: String,
    pub actor: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------------------------
// One-shot actions (R3, section 5 of the spec: a command with a TTL, not desired state)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Start,
    Stop,
    Restart,
    Kill,
    Pull,
    Recreate,
    Remove,
    Prune,
    ImagePull,
    ImageRemove,
    /// Delete one named volume nothing mounts. Data loss by definition, so the UI wants the
    /// name typed; the server refuses while any container (running or not) still uses it.
    VolumeRemove,
}

impl ActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::Start => "start",
            ActionKind::Stop => "stop",
            ActionKind::Restart => "restart",
            ActionKind::Kill => "kill",
            ActionKind::Pull => "pull",
            ActionKind::Recreate => "recreate",
            ActionKind::Remove => "remove",
            ActionKind::Prune => "prune",
            ActionKind::ImagePull => "image_pull",
            ActionKind::ImageRemove => "image_remove",
            ActionKind::VolumeRemove => "volume_remove",
        }
    }
    /// The audit `noun.verb` for this action.
    pub fn audit_action(self) -> String {
        match self {
            ActionKind::Prune => "prune".to_string(),
            ActionKind::ImagePull => "image.pull".to_string(),
            ActionKind::ImageRemove => "image.remove".to_string(),
            ActionKind::VolumeRemove => "volume.remove".to_string(),
            k => format!("container.{}", k.as_str()),
        }
    }
    /// Which agent capability runs it.
    pub fn capability(self) -> AgentCapability {
        AgentCapability::Lifecycle
    }
    pub fn needs_container(self) -> bool {
        !matches!(
            self,
            ActionKind::Prune
                | ActionKind::ImagePull
                | ActionKind::ImageRemove
                | ActionKind::VolumeRemove
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    Queued,
    Running,
    Done,
    Failed,
    Expired,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_volumes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub containers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volumes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_cache: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionInput {
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub id: String,
    pub host: String,
    pub kind: ActionKind,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    pub args: ActionArgs,
    pub status: ActionStatus,
    pub actor: String,
    pub created_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    /// The lease: an action nobody picked up by then expires instead of firing late.
    pub expires_ms: i64,
    pub result: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PruneCategory {
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrunePreview {
    pub host: String,
    pub images: PruneCategory,
    pub containers: PruneCategory,
    pub volumes: PruneCategory,
    pub build_cache: PruneCategory,
}

// ---------------------------------------------------------------------------------------------
// Stacks (R4)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StackStatus {
    Declared,
    Rendered,
    Pending,
    Applying,
    Running,
    Drifted,
    Failed,
    Destroying,
    Gone,
}

impl StackStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StackStatus::Declared => "declared",
            StackStatus::Rendered => "rendered",
            StackStatus::Pending => "pending",
            StackStatus::Applying => "applying",
            StackStatus::Running => "running",
            StackStatus::Drifted => "drifted",
            StackStatus::Failed => "failed",
            StackStatus::Destroying => "destroying",
            StackStatus::Gone => "gone",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StackSource {
    Template,
    Compose,
    Adopted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackServiceContainer {
    pub id: String,
    pub name: String,
    pub state: ContainerState,
    pub health: ContainerHealth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackServiceState {
    pub name: String,
    pub image: String,
    pub containers: Vec<StackServiceContainer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    ContainerMissing,
    ExtraContainer,
    ImageChanged,
    ConfigChanged,
    StateChanged,
    EnvChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftItem {
    pub kind: DriftKind,
    pub service: Option<String>,
    pub field: Option<String>,
    pub desired: Option<String>,
    pub observed: Option<String>,
    pub since_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullPolicy {
    Missing,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployOptions {
    pub pull: PullPolicy,
    #[serde(default)]
    pub force_recreate: bool,
    #[serde(default)]
    pub recreate_volumes: bool,
}

impl Default for DeployOptions {
    fn default() -> Self {
        DeployOptions {
            pull: PullPolicy::Missing,
            force_recreate: false,
            recreate_volumes: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    Privileged,
    DockerSock,
    HostNetwork,
    HostPid,
    BindMountOutside,
    CapAdd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WarningSeverity {
    Error,
    Warning,
}

/// A dangerous compose construct (R8.4). `id` is stable across edits of the same construct so an
/// acknowledgement sticks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComposeWarning {
    pub id: String,
    pub kind: WarningKind,
    pub severity: WarningSeverity,
    pub service: Option<String>,
    pub line: Option<usize>,
    pub detail: String,
}

/// A stack as the UI reads it. `services` and `drift` are computed against the host's latest
/// inventory when the row is served; the stored row carries everything else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    pub id: String,
    pub name: String,
    pub host: String,
    pub site: Option<String>,
    pub source: StackSource,
    pub template_id: Option<String>,
    pub template_name: Option<String>,
    pub template_version: Option<String>,
    /// The version row a template stack pins (needed for upgrades and rollbacks; the UI keys off
    /// `template_version` text but the server needs the id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_version_id: Option<String>,
    pub status: StackStatus,
    pub desired_generation: u64,
    pub applied_generation: Option<u64>,
    pub files: Vec<StackFile>,
    /// Resolved non-secret variables; secret NAMES only.
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    pub deploy_options: DeployOptions,
    #[serde(default)]
    pub services: Vec<StackServiceState>,
    #[serde(default)]
    pub drift: Vec<DriftItem>,
    #[serde(default)]
    pub apm_services: Vec<String>,
    #[serde(default)]
    pub acknowledged_warnings: Vec<String>,
    pub last_error: Option<String>,
    pub created_by: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub last_applied_ms: Option<i64>,
    /// Server-side bookkeeping, never shown: the one-shot operation the next apply performs
    /// (`up` by default; `down`/`restart`/`destroy` are transient and reset after the agent
    /// reports), and the config hashes observed right after the last successful apply — the
    /// drift baseline.
    #[serde(default)]
    pub pending_op: Option<StackOp>,
    #[serde(default)]
    pub baseline_config_hashes: BTreeMap<String, String>,
    /// The previous generation's files + variables, kept for rollout rollback (R6.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<StackSnapshot>>,
    /// Drift the operator chose to accept: the observed config hashes become the baseline.
    #[serde(default)]
    pub drift_since_ms: Option<i64>,
}

/// What an apply on the agent should do with the stack folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StackOp {
    /// `compose up -d` with the deploy options.
    Up,
    /// `compose down` (containers removed, volumes kept).
    Down,
    /// `compose restart`.
    Restart,
    /// `compose pull` + `up -d --force-recreate`.
    Redeploy,
    /// `compose down` (+ `-v` when `volumes`), then forget the folder.
    Destroy,
    /// `compose up -d --force-recreate` from the stored files (reconcile drift).
    Reconcile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackSnapshot {
    pub template_version_id: Option<String>,
    pub template_version: Option<String>,
    pub files: Vec<StackFile>,
    pub variables: BTreeMap<String, String>,
    pub deploy_options: DeployOptions,
    pub generation: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StackInput {
    pub name: String,
    pub host: String,
    pub files: Vec<StackFile>,
    #[serde(default)]
    pub deploy_options: DeployOptions,
    #[serde(default)]
    pub acknowledged_warnings: Vec<String>,
    #[serde(default)]
    pub apply: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StackFilesPatch {
    pub files: Vec<StackFile>,
    #[serde(default)]
    pub acknowledged_warnings: Vec<String>,
    #[serde(default)]
    pub deploy_options: Option<DeployOptions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StackActionKind {
    Up,
    Down,
    Restart,
    Redeploy,
    Destroy,
    Reconcile,
    Accept,
}

impl StackActionKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "restart" => Some(Self::Restart),
            "redeploy" => Some(Self::Redeploy),
            "destroy" => Some(Self::Destroy),
            "reconcile" => Some(Self::Reconcile),
            "accept" => Some(Self::Accept),
            _ => None,
        }
    }
    pub fn audit_action(self) -> &'static str {
        match self {
            Self::Up => "stack.apply",
            Self::Down => "stack.down",
            Self::Restart => "stack.restart",
            Self::Redeploy => "stack.redeploy",
            Self::Destroy => "stack.destroy",
            Self::Reconcile => "stack.reconcile",
            Self::Accept => "stack.accept_drift",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StackActionArgs {
    #[serde(default)]
    pub pull: Option<bool>,
    #[serde(default)]
    pub volumes: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackValidation {
    pub ok: bool,
    pub warnings: Vec<ComposeWarning>,
    pub services: Vec<String>,
    pub missing_variables: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredProject {
    pub host: String,
    pub name: String,
    pub services: Vec<String>,
    pub containers: u32,
    pub running: u32,
    pub compose_file: Option<String>,
    pub working_dir: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Templates (R5)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VariableType {
    String,
    Number,
    Bool,
    Select,
    Port,
    Path,
    Secret,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateVariable {
    pub name: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub kind: VariableType,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TemplateRequires {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_docker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ram_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_disk_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplatePort {
    pub name: String,
    pub container: u16,
    pub default_host: u16,
    #[serde(default = "default_tcp")]
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn default_tcp() -> String {
    "tcp".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateVolume {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthKind {
    ContainerHealthy,
    UptimeMonitor,
    SloBurn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateHealth {
    pub kind: HealthKind,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub grace_secs: u64,
}

/// The Photon block (R5.8): what the render injects into the deployed services.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TemplateObservability {
    #[serde(default)]
    pub otlp_endpoint: bool,
    #[serde(default)]
    pub ingest_token: bool,
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub environment: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateManifest {
    pub id: String,
    pub version: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub variables: Vec<TemplateVariable>,
    #[serde(default)]
    pub requires: TemplateRequires,
    #[serde(default)]
    pub ports: Vec<TemplatePort>,
    #[serde(default)]
    pub volumes: Vec<TemplateVolume>,
    #[serde(default)]
    pub health: Option<TemplateHealth>,
    #[serde(default)]
    pub observability: TemplateObservability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateVersionRef {
    pub id: String,
    pub version: String,
    pub created_ms: i64,
    pub created_by: String,
    pub notes: Option<String>,
    pub deployed_count: u64,
}

/// An immutable template version (R5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateVersion {
    pub id: String,
    pub template_id: String,
    pub version: String,
    pub created_ms: i64,
    pub created_by: String,
    pub notes: Option<String>,
    pub deployed_count: u64,
    pub manifest: TemplateManifest,
    pub compose_text: String,
}

impl TemplateVersion {
    pub fn as_ref(&self) -> TemplateVersionRef {
        TemplateVersionRef {
            id: self.id.clone(),
            version: self.version.clone(),
            created_ms: self.created_ms,
            created_by: self.created_by.clone(),
            notes: self.notes.clone(),
            deployed_count: self.deployed_count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateSource {
    Local,
    Registry,
    Portainer,
}

/// The catalog row: identity + the versions it carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub categories: Vec<String>,
    pub source: TemplateSource,
    pub registry_id: Option<String>,
    pub latest_version: String,
    pub versions: Vec<TemplateVersionRef>,
    pub updated_ms: i64,
    pub stacks: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateVersionInput {
    #[serde(default)]
    pub template_id: Option<String>,
    pub manifest: TemplateManifest,
    pub compose_text: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryKind {
    Photon,
    PortainerV2,
    PortainerV3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistryStatus {
    Ok,
    Error,
    Syncing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    pub id: String,
    pub name: String,
    pub url: String,
    pub kind: RegistryKind,
    pub last_sync_ms: Option<i64>,
    pub template_count: u64,
    pub status: RegistryStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryInput {
    pub name: String,
    pub url: String,
    pub kind: RegistryKind,
}

// ---------------------------------------------------------------------------------------------
// Image registry credentials (private pulls)
// ---------------------------------------------------------------------------------------------

/// A credential for pulling from a private image registry (a GitLab deploy token, a GHCR PAT,
/// a Hub login), as the UI reads it — the secret itself is write-only and never in this view.
/// Held server-side, encrypted like stack secrets (R8.5), and handed to an agent over its token
/// channel at pull time; nothing is ever `docker login`-ed on a host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryCredential {
    pub id: String,
    /// The registry host as an image reference names it (`registry.gitlab.com`, `ghcr.io`,
    /// `docker.io`), normalized by `registry::normalize_server`.
    pub server: String,
    pub username: String,
    /// What the operator calls it ("GitLab deploy token, nodefluxio group").
    #[serde(default)]
    pub label: Option<String>,
    /// Which managed hosts may receive it; `None` = every host.
    #[serde(default)]
    pub scope: Option<TargetSelector>,
    pub created_ms: i64,
    pub created_by: String,
    pub updated_ms: i64,
}

/// `POST /api/image-registries` and `PATCH …/:id`: `secret` is required on create and optional
/// on update (absent = keep the stored one).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RegistryCredentialInput {
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// `Some(None)` clears the scope back to every host.
    #[serde(default, deserialize_with = "deserialize_double_option_selector")]
    pub scope: Option<Option<TargetSelector>>,
}

fn deserialize_double_option_selector<'de, D>(
    d: D,
) -> Result<Option<Option<TargetSelector>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<TargetSelector>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreflightCheck {
    pub key: String,
    pub label: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateDeployInput {
    pub template_version_id: String,
    pub host: String,
    pub name: String,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub deploy_options: DeployOptions,
    #[serde(default)]
    pub acknowledged_warnings: Vec<String>,
    #[serde(default)]
    pub stack_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DryRunResult {
    pub ok: bool,
    pub host: String,
    pub checks: Vec<PreflightCheck>,
    pub compose: String,
    pub env: String,
    pub previous_compose: Option<String>,
    pub warnings: Vec<ComposeWarning>,
    pub secrets: Vec<String>,
    pub generated: Vec<String>,
    pub error: Option<String>,
}

impl DryRunResult {
    pub fn refused(host: &str, error: impl Into<String>) -> Self {
        DryRunResult {
            ok: false,
            host: host.to_string(),
            checks: vec![],
            compose: String::new(),
            env: String::new(),
            previous_compose: None,
            warnings: vec![],
            secrets: vec![],
            generated: vec![],
            error: Some(error.into()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rollouts (R6)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetSelector {
    Hosts { hosts: Vec<String> },
    Site { sites: Vec<String> },
    Labels { labels: BTreeMap<String, String> },
}

impl TargetSelector {
    /// Does a managed host fall under this selector? Shared by rollout planning and the
    /// registry-credential scope so both answer the same way.
    pub fn matches(
        &self,
        host: &str,
        site: Option<&str>,
        labels: &BTreeMap<String, String>,
    ) -> bool {
        match self {
            TargetSelector::Hosts { hosts } => hosts.iter().any(|x| x == host),
            TargetSelector::Site { sites } => sites.iter().any(|s| s == site.unwrap_or("")),
            TargetSelector::Labels { labels: want } => {
                want.iter().all(|(k, v)| labels.get(k) == Some(v))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RolloutStrategy {
    #[serde(default = "default_canary")]
    pub canary: u32,
    #[serde(default)]
    pub canary_is_percent: bool,
    #[serde(default = "default_canary")]
    pub failure_threshold: u32,
    #[serde(default = "default_true")]
    pub prepull: bool,
    #[serde(default)]
    pub auto_promote: bool,
    #[serde(default = "default_grace")]
    pub health_grace_secs: u64,
}

fn default_canary() -> u32 {
    1
}
fn default_true() -> bool {
    true
}
fn default_grace() -> u64 {
    120
}

impl Default for RolloutStrategy {
    fn default() -> Self {
        RolloutStrategy {
            canary: 1,
            canary_is_percent: false,
            failure_threshold: 1,
            prepull: true,
            auto_promote: false,
            health_grace_secs: 120,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutState {
    Draft,
    Running,
    PausedGate,
    PausedThreshold,
    Completed,
    Aborted,
    RolledBack,
}

impl RolloutState {
    pub fn as_str(self) -> &'static str {
        match self {
            RolloutState::Draft => "draft",
            RolloutState::Running => "running",
            RolloutState::PausedGate => "paused_gate",
            RolloutState::PausedThreshold => "paused_threshold",
            RolloutState::Completed => "completed",
            RolloutState::Aborted => "aborted",
            RolloutState::RolledBack => "rolled_back",
        }
    }
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RolloutState::Completed | RolloutState::Aborted | RolloutState::RolledBack
        )
    }
}

/// `queued` = its stage has not been reached; `pending` = its stage is live but the host is not
/// answering. Two facts, two words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetStatus {
    Queued,
    Pending,
    Prepulling,
    Applying,
    Healthy,
    Failed,
    Drifted,
    Skipped,
}

impl TargetStatus {
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            TargetStatus::Healthy
                | TargetStatus::Failed
                | TargetStatus::Skipped
                | TargetStatus::Drifted
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RolloutTarget {
    pub host: String,
    pub site: Option<String>,
    /// 0 = not eligible (skipped at planning); 1 = canary; 2 = the rest.
    pub stage: u32,
    pub status: TargetStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub stack_id: Option<String>,
    pub previous_version: Option<String>,
    pub agent_status: AgentStatus,
    pub updated_ms: i64,
    /// Server-side: when the stage's apply was handed to the stack, for the health grace clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healthy_since_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rollout {
    pub id: String,
    pub name: String,
    pub template_id: String,
    pub template_name: String,
    pub template_version_id: String,
    pub template_version: String,
    pub previous_version: Option<String>,
    pub stack_name: String,
    pub selector: TargetSelector,
    pub strategy: RolloutStrategy,
    pub state: RolloutState,
    pub stage: u32,
    pub stages: u32,
    pub targets: Vec<RolloutTarget>,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    pub created_by: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    /// Server-side: the deploy options and acknowledgements every target's apply carries.
    #[serde(default)]
    pub deploy_options: DeployOptions,
    #[serde(default)]
    pub acknowledged_warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RolloutInput {
    pub name: String,
    pub template_version_id: String,
    pub stack_name: String,
    pub selector: TargetSelector,
    #[serde(default)]
    pub strategy: RolloutStrategy,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub deploy_options: DeployOptions,
    #[serde(default)]
    pub acknowledged_warnings: Vec<String>,
    #[serde(default)]
    pub start: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RolloutActionKind {
    Start,
    Promote,
    Pause,
    Resume,
    Abort,
    Rollback,
    Retry,
}

impl RolloutActionKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "start" => Some(Self::Start),
            "promote" => Some(Self::Promote),
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "abort" => Some(Self::Abort),
            "rollback" => Some(Self::Rollback),
            "retry" => Some(Self::Retry),
            _ => None,
        }
    }
    pub fn audit_action(self) -> &'static str {
        match self {
            Self::Start => "rollout.start",
            Self::Promote => "rollout.promote",
            Self::Pause => "rollout.pause",
            Self::Resume => "rollout.resume",
            Self::Abort => "rollout.abort",
            Self::Rollback => "rollout.rollback",
            Self::Retry => "rollout.retry",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Exec (R3.4)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecSession {
    pub id: String,
    pub host: String,
    pub container_id: String,
    pub container_name: String,
    pub shell: String,
    pub opened_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecOutput {
    pub lines: Vec<String>,
    pub exit_code: Option<i64>,
    pub closed: bool,
    /// Output arrived AFTER the line this call sent (or, for a poll, during its wait). False
    /// with non-empty `lines` means those are a previous command's late output and the answer
    /// to this one is still coming — the UI keeps polling.
    #[serde(default)]
    pub answered: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_rank_and_gate() {
        assert!(Role::Admin.allows(Permission::Exec));
        assert!(!Role::Operator.allows(Permission::Exec));
        assert!(Role::Operator.allows(Permission::Deploy));
        assert!(Role::Viewer.allows(Permission::Audit));
        assert!(!Role::Viewer.allows(Permission::Lifecycle));
        assert_eq!(Role::parse("operator"), Some(Role::Operator));
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), "\"admin\"");
    }

    #[test]
    fn host_status_from_last_seen() {
        let mut h = HostRecord {
            id: "h".into(),
            host: "web-1".into(),
            site: None,
            token_hash: String::new(),
            requested_capabilities: vec![AgentCapability::Read],
            reported_capabilities: vec![],
            agent_version: None,
            upgrade_to: None,
            poll_secs: 15,
            last_seen_ms: None,
            enrolled_ms: 0,
            docker: None,
            host_id: None,
            arch: None,
            os: None,
            labels: BTreeMap::new(),
            desired_generation: 0,
        };
        assert_eq!(h.status(1_000_000), AgentStatus::Pending);
        assert_eq!(h.capabilities(), vec![AgentCapability::Read]);
        h.last_seen_ms = Some(1_000_000);
        assert_eq!(h.status(1_000_000 + 40_000), AgentStatus::Online);
        assert_eq!(h.status(1_000_000 + 61_000), AgentStatus::Offline);
    }

    #[test]
    fn selector_is_internally_tagged() {
        let s: TargetSelector =
            serde_json::from_str(r#"{"kind":"site","sites":["divtik"]}"#).unwrap();
        assert_eq!(
            s,
            TargetSelector::Site {
                sites: vec!["divtik".into()]
            }
        );
        let a: ActionKind = serde_json::from_str("\"image_pull\"").unwrap();
        assert_eq!(a, ActionKind::ImagePull);
        assert_eq!(
            serde_json::to_string(&RolloutState::PausedGate).unwrap(),
            "\"paused_gate\""
        );
    }

    #[test]
    fn agent_patch_distinguishes_clear_from_absent() {
        let p: AgentPatch = serde_json::from_str(r#"{"site":null}"#).unwrap();
        assert_eq!(p.site, Some(None));
        let p: AgentPatch = serde_json::from_str(r#"{"poll_secs":30}"#).unwrap();
        assert_eq!(p.site, None);
    }
}

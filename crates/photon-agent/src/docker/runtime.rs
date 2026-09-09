//! The one object `sync.rs` talks to: a (lazily connected, reconnecting) bollard client plus the
//! per-agent inventory state and the event subscription, with the capability gate applied at
//! every entry point. A daemon that is down at start is not fatal — the agent keeps syncing
//! (reporting no inventory) and connects when Docker comes back.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bollard::Docker;
use photon_deploy::model::{AgentCapability, DockerEvent, Observed};
use photon_deploy::proto::{ActionSpec, DesiredStack, RegistryAuth};
use tokio::sync::Mutex;

use super::events::EventBuffer;
use super::inventory::Inventory;
use super::{actions, stacks, Caps};
use crate::state::LocalState;

pub struct DockerRuntime {
    caps: Caps,
    docker_host: String,
    data_dir: PathBuf,
    docker: Mutex<Option<Docker>>,
    inventory: Mutex<Inventory>,
    events: Mutex<Option<EventBuffer>>,
}

fn connect(docker_host: &str) -> Result<Docker, String> {
    let d = if let Some(path) = docker_host.strip_prefix("unix://") {
        Docker::connect_with_unix(path, 120, bollard::API_DEFAULT_VERSION)
            .map_err(|e| e.to_string())?
    } else if docker_host.starts_with("tcp://") || docker_host.starts_with("http://") {
        Docker::connect_with_http(docker_host, 120, bollard::API_DEFAULT_VERSION)
            .map_err(|e| e.to_string())?
    } else {
        Docker::connect_with_unix(docker_host, 120, bollard::API_DEFAULT_VERSION)
            .map_err(|e| e.to_string())?
    };
    Ok(d)
}

impl DockerRuntime {
    pub fn new(caps: Caps, docker_host: &str, data_dir: &Path) -> DockerRuntime {
        DockerRuntime {
            caps,
            docker_host: docker_host.to_string(),
            data_dir: data_dir.to_path_buf(),
            docker: Mutex::new(None),
            inventory: Mutex::new(Inventory::default()),
            events: Mutex::new(None),
        }
    }

    pub fn caps(&self) -> Caps {
        self.caps
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// A live client, connecting (and pinging) on first use or after a failure.
    pub async fn docker(&self) -> Result<Docker, String> {
        if !self.caps.any() {
            return Err("no Docker capability enabled on this agent".into());
        }
        let mut g = self.docker.lock().await;
        if let Some(d) = g.as_ref() {
            return Ok(d.clone());
        }
        let d = connect(&self.docker_host)?;
        tokio::time::timeout(Duration::from_secs(10), d.ping())
            .await
            .map_err(|_| "docker ping timed out".to_string())?
            .map_err(|e| format!("docker not reachable at {}: {e}", self.docker_host))?;
        *g = Some(d.clone());
        // The events subscription follows the connection.
        let mut ev = self.events.lock().await;
        if ev.is_none() && self.caps.read {
            *ev = Some(EventBuffer::spawn(d.clone()));
        }
        Ok(d)
    }

    /// Forget a client after an error so the next call reconnects.
    pub async fn reset(&self) {
        *self.docker.lock().await = None;
        *self.events.lock().await = None;
    }

    /// The observed inventory (`read`). `Err` when Docker is unreachable or the capability is
    /// off — the sync then carries `observed: null`.
    pub async fn observe(&self, host: &str, state: &LocalState) -> Result<Observed, String> {
        self.caps.require(AgentCapability::Read)?;
        let d = self.docker().await?;
        let applied = |name: &str| state.applied_generation_for(name);
        let mut inv = self.inventory.lock().await;
        match inv.observe(&d, host, &applied).await {
            Ok(o) => Ok(o),
            Err(e) => {
                drop(inv);
                self.reset().await;
                Err(e)
            }
        }
    }

    pub async fn drain_events(&self) -> Vec<DockerEvent> {
        let g = self.events.lock().await;
        g.as_ref().map(|e| e.drain()).unwrap_or_default()
    }

    /// One lifecycle action (`lifecycle`). `auths` are the registry credentials the server
    /// handed over for this action's pull, if it pulls.
    pub async fn run_action(
        &self,
        spec: &ActionSpec,
        auths: &[RegistryAuth],
    ) -> Result<String, String> {
        self.caps.require(AgentCapability::Lifecycle)?;
        let d = self.docker().await?;
        actions::run(&d, spec, auths).await
    }

    /// Apply one desired stack (`apply`): files + secrets → folder → `docker compose`, with
    /// the registry credentials in a throwaway `DOCKER_CONFIG` for the compose run.
    pub async fn apply_stack(
        &self,
        stack: &DesiredStack,
        secrets: &BTreeMap<String, String>,
        auths: &[RegistryAuth],
    ) -> Result<(String, BTreeMap<String, String>), String> {
        self.caps.require(AgentCapability::Apply)?;
        let d = self.docker().await.ok();
        stacks::apply(d.as_ref(), &self.data_dir, stack, secrets, auths).await
    }
}

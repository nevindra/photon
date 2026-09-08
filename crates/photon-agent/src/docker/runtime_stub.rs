//! `DockerRuntime` for a build without the `docker` feature: the same surface, every request
//! refused with the reason, so `sync.rs` compiles once and the metrics-only agent keeps its
//! footprint.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use photon_deploy::model::{DockerEvent, Observed};
use photon_deploy::proto::{ActionSpec, DesiredStack, RegistryAuth};

use super::Caps;
use crate::state::LocalState;

const REASON: &str = "this photon-agent was built without the docker feature";

pub struct DockerRuntime {
    caps: Caps,
    data_dir: PathBuf,
}

impl DockerRuntime {
    pub fn new(caps: Caps, _docker_host: &str, data_dir: &Path) -> DockerRuntime {
        DockerRuntime {
            caps,
            data_dir: data_dir.to_path_buf(),
        }
    }
    pub fn caps(&self) -> Caps {
        self.caps
    }
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    pub async fn reset(&self) {}
    pub async fn observe(&self, _host: &str, _state: &LocalState) -> Result<Observed, String> {
        Err(REASON.into())
    }
    pub async fn drain_events(&self) -> Vec<DockerEvent> {
        Vec::new()
    }
    pub async fn run_action(
        &self,
        _spec: &ActionSpec,
        _auths: &[RegistryAuth],
    ) -> Result<String, String> {
        Err(REASON.into())
    }
    pub async fn apply_stack(
        &self,
        _stack: &DesiredStack,
        _secrets: &BTreeMap<String, String>,
        _auths: &[RegistryAuth],
    ) -> Result<(String, BTreeMap<String, String>), String> {
        Err(REASON.into())
    }
}

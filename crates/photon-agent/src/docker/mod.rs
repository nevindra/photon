//! Docker on this host. `Caps` (the local capability gate) and the pure helpers (`compose`,
//! `stats`, env masking) are always compiled; everything that talks to the daemon —
//! `inventory`, `events`, `actions`, `stacks`, `streams` — sits behind the `docker` feature
//! and is reached through `DockerRuntime`, which a metrics-only build replaces with a stub
//! that refuses every request with a clear reason. `sync.rs` never sees a `cfg`.
//!
//! Capability gating (R1.3) happens HERE, at the edge, in `Caps::require`: the server
//! intersects what it wants with what the agent reported, but a compromised or confused server
//! still cannot make an agent installed with `--docker-read` restart a container.
use photon_deploy::model::AgentCapability;

pub mod compose;
pub mod stats;

#[cfg(feature = "docker")]
pub mod actions;
#[cfg(feature = "docker")]
pub mod events;
#[cfg(feature = "docker")]
pub mod inventory;
#[cfg(feature = "docker")]
pub mod stacks;
#[cfg(feature = "docker")]
pub mod streams;

#[cfg(feature = "docker")]
mod runtime;
#[cfg(feature = "docker")]
pub use runtime::DockerRuntime;
#[cfg(not(feature = "docker"))]
mod runtime_stub;
#[cfg(not(feature = "docker"))]
pub use runtime_stub::DockerRuntime;

/// What the operator allowed on this host. Built from the CLI flags once; never mutated by a
/// sync response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    pub read: bool,
    pub lifecycle: bool,
    pub apply: bool,
    pub exec: bool,
}

impl Caps {
    pub fn from_config(cfg: &crate::config::AgentConfig) -> Caps {
        let list = cfg.capabilities();
        Caps {
            read: list.contains(&AgentCapability::Read),
            lifecycle: list.contains(&AgentCapability::Lifecycle),
            apply: list.contains(&AgentCapability::Apply),
            exec: list.contains(&AgentCapability::Exec),
        }
    }

    pub fn has(&self, cap: AgentCapability) -> bool {
        match cap {
            AgentCapability::Read => self.read,
            AgentCapability::Lifecycle => self.lifecycle,
            AgentCapability::Apply => self.apply,
            AgentCapability::Exec => self.exec,
        }
    }

    /// The refusal a server-sent request gets when the flag is off — the message the server
    /// stores as the result, so the operator sees WHY nothing happened.
    pub fn require(&self, cap: AgentCapability) -> Result<(), String> {
        if self.has(cap) {
            Ok(())
        } else {
            Err(format!(
                "capability '{}' not enabled on this agent",
                cap.as_str()
            ))
        }
    }

    pub fn list(&self) -> Vec<AgentCapability> {
        let mut v = Vec::new();
        if self.read {
            v.push(AgentCapability::Read);
        }
        if self.lifecycle {
            v.push(AgentCapability::Lifecycle);
        }
        if self.apply {
            v.push(AgentCapability::Apply);
        }
        if self.exec {
            v.push(AgentCapability::Exec);
        }
        v
    }

    pub fn any(&self) -> bool {
        self.read || self.lifecycle || self.apply || self.exec
    }
}

pub const MASK: &str = "••••••••";

/// Whether an env var NAME looks like it holds a secret. Case-insensitive substring match on
/// the usual suspects; a false positive masks a harmless value, a false negative leaks a
/// password to the server, so the list errs wide.
pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "pass",
        "secret",
        "token",
        "key",
        "pwd",
        "credential",
        "auth",
    ]
    .iter()
    .any(|needle| k.contains(needle))
}

/// Split Docker's `KEY=VALUE` env entries into the key list and a map whose secret-looking
/// values are replaced with `MASK` BEFORE anything leaves the host.
pub fn split_env(entries: &[String]) -> (Vec<String>, std::collections::BTreeMap<String, String>) {
    let mut keys = Vec::with_capacity(entries.len());
    let mut map = std::collections::BTreeMap::new();
    for e in entries {
        let (k, v) = match e.split_once('=') {
            Some((k, v)) => (k, v),
            None => (e.as_str(), ""),
        };
        keys.push(k.to_string());
        let shown = if is_secret_key(k) {
            MASK.to_string()
        } else {
            v.to_string()
        };
        map.insert(k.to_string(), shown);
    }
    (keys, map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_secret_looking_env_values() {
        let entries: Vec<String> = [
            "PATH=/usr/bin",
            "POSTGRES_PASSWORD=hunter2",
            "API_TOKEN=abc",
            "aws_secret_access_key=xyz",
            "MyPwd=1",
            "DB_CREDENTIALS=u:p",
            "AUTH_HEADER=Bearer x",
            "SSH_KEY=----",
            "NO_VALUE",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (keys, env) = split_env(&entries);
        assert_eq!(keys.len(), 9);
        assert_eq!(env["PATH"], "/usr/bin");
        assert_eq!(env["POSTGRES_PASSWORD"], MASK);
        assert_eq!(env["API_TOKEN"], MASK);
        assert_eq!(env["aws_secret_access_key"], MASK);
        assert_eq!(env["MyPwd"], MASK);
        assert_eq!(env["DB_CREDENTIALS"], MASK);
        assert_eq!(env["AUTH_HEADER"], MASK);
        assert_eq!(env["SSH_KEY"], MASK);
        assert_eq!(env["NO_VALUE"], "");
        // The raw values never survive into the map.
        assert!(!env.values().any(|v| v == "hunter2"));
    }

    #[test]
    fn caps_refuse_what_the_flags_did_not_allow() {
        let c = Caps {
            read: true,
            ..Caps::default()
        };
        assert!(c.require(AgentCapability::Read).is_ok());
        assert_eq!(
            c.require(AgentCapability::Lifecycle).unwrap_err(),
            "capability 'lifecycle' not enabled on this agent"
        );
        assert_eq!(
            c.require(AgentCapability::Apply).unwrap_err(),
            "capability 'apply' not enabled on this agent"
        );
        assert_eq!(
            c.require(AgentCapability::Exec).unwrap_err(),
            "capability 'exec' not enabled on this agent"
        );
        assert_eq!(c.list(), vec![AgentCapability::Read]);
        assert!(!Caps::default().any());
        let all = Caps {
            read: true,
            lifecycle: true,
            apply: true,
            exec: true,
        };
        assert_eq!(all.list().len(), 4);
        assert!(all.require(AgentCapability::Exec).is_ok());
    }
}

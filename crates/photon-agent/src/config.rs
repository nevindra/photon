//! The CLI. Every flag is env-backed so the systemd unit written by `photon-agent install` can
//! drive the binary from `/etc/photon-agent/agent.env` with no arguments at all.
//!
//! Capabilities are opt-in on THIS side (R1.3): `--docker-read` / `--docker-lifecycle` /
//! `--docker-apply` / `--docker-exec` are the whole authority — the server intersects what it
//! wants with what the agent admits to, and anything not enabled here is refused locally no
//! matter what a sync response asks for. `lifecycle`/`apply`/`exec` each imply `read`, since
//! none of them makes sense without an inventory.
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use photon_deploy::model::AgentCapability;

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4318/v1/metrics";
pub const DEFAULT_DATA_DIR: &str = "/var/lib/photon-agent";
pub const DEFAULT_DOCKER_HOST: &str = "unix:///var/run/docker.sock";
pub const MIN_POLL: Duration = Duration::from_secs(5);
pub const DEFAULT_POLL: Duration = Duration::from_secs(15);

#[derive(Parser, Debug, Clone)]
#[command(
    name = "photon-agent",
    version,
    about = "Photon host agent: resource metrics + the Docker fleet-management control channel",
    long_about = "Photon host agent.\n\n\
With no subcommand it runs: samples host/GPU resource metrics on --interval-secs and, when \
--server is set, polls the server's /api/agent/sync for Docker work. Docker capabilities are \
opt-in here and only here — the server cannot enable one the flags did not."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub cfg: AgentConfig,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Install as a systemd service (writes /etc/photon-agent/agent.env + the unit, enables it).
    /// Needs root; prompts for the agent token when --token is not given.
    Install(AgentConfig),
    /// Stop, disable and remove the systemd service and its env file. Containers keep running.
    Uninstall,
    /// Print the agent version.
    Version,
}

#[derive(Parser, Debug, Clone)]
pub struct AgentConfig {
    /// OTLP/HTTP metrics endpoint. When --server is set and this is not given, the endpoint the
    /// server advertises on sync is used instead (a host then needs only the agent token).
    #[arg(long, env = "PHOTON_AGENT_ENDPOINT")]
    pub endpoint: Option<String>,

    /// Ingest bearer token for the metrics endpoint.
    #[arg(long, env = "PHOTON_INGEST_TOKEN", default_value = "dev-ingest-token")]
    pub ingest_token: String,

    /// Hostname reported as `host.name`. Defaults to $PHOTON_AGENT_HOST, then the OS hostname.
    #[arg(long, env = "PHOTON_AGENT_HOST_NAME")]
    pub host_name: Option<String>,

    /// Seconds between resource samples.
    #[arg(long, env = "PHOTON_AGENT_INTERVAL", default_value_t = 15)]
    pub interval_secs: u64,

    /// Disable GPU sampling even when built with the `gpu` feature.
    #[arg(long, env = "PHOTON_AGENT_NO_GPU", default_value_t = false)]
    pub no_gpu: bool,

    /// Disable the resource sampler entirely (control channel only).
    #[arg(long, env = "PHOTON_AGENT_NO_METRICS", default_value_t = false)]
    pub no_metrics: bool,

    /// Photon server base URL (e.g. https://photon.example). Enables the control channel.
    #[arg(long, env = "PHOTON_SERVER")]
    pub server: Option<String>,

    /// The per-host agent token (`ak_…`) minted by the server when the host was enrolled.
    /// Leave it out at an interactive terminal and the agent prompts for it (input hidden), so
    /// the token never lands in shell history; scripts pass it here or via PHOTON_AGENT_TOKEN.
    #[arg(long, env = "PHOTON_AGENT_TOKEN")]
    pub token: Option<String>,

    /// Allow read-only Docker: inventory, logs, stats, events.
    #[arg(long, env = "PHOTON_AGENT_DOCKER_READ", default_value_t = false)]
    pub docker_read: bool,

    /// Allow container lifecycle actions: start/stop/restart/kill/remove/pull/recreate/prune.
    #[arg(long, env = "PHOTON_AGENT_DOCKER_LIFECYCLE", default_value_t = false)]
    pub docker_lifecycle: bool,

    /// Allow applying desired stacks (docker compose up/down/…) from the server.
    #[arg(long, env = "PHOTON_AGENT_DOCKER_APPLY", default_value_t = false)]
    pub docker_apply: bool,

    /// Allow an interactive shell into a container (audited server-side, admin only).
    #[arg(long, env = "PHOTON_AGENT_DOCKER_EXEC", default_value_t = false)]
    pub docker_exec: bool,

    /// Sync poll interval: `15s`, `1m`, or plain seconds. Minimum 5s.
    #[arg(long, env = "PHOTON_AGENT_POLL", default_value = "15s", value_parser = parse_poll)]
    pub poll: Duration,

    /// Local state + stack folders. Falls back to $HOME/.photon-agent when not writable.
    #[arg(long, env = "PHOTON_AGENT_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Docker daemon address.
    #[arg(long, env = "DOCKER_HOST", default_value = DEFAULT_DOCKER_HOST)]
    pub docker_host: String,

    /// Print the observed Docker inventory as JSON and exit (needs --docker-read).
    #[arg(long, hide = true, default_value_t = false)]
    pub dry_inventory: bool,
}

/// `15s` / `1m` / `2h` / bare `30` (seconds). Clamped to `MIN_POLL` — R1.6 says the default is
/// no faster than 15s, and 5s is the floor even for an operator who wants it snappy.
pub fn parse_poll(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty poll interval".into());
    }
    let (num, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => s.split_at(i),
        None => (s, "s"),
    };
    let n: u64 = num
        .parse()
        .map_err(|_| format!("bad poll interval '{s}': expected e.g. 15s, 1m, 30"))?;
    let secs = match unit.trim() {
        "s" | "sec" | "secs" | "" => n,
        "m" | "min" | "mins" => n * 60,
        "h" | "hr" | "hrs" => n * 3600,
        other => return Err(format!("bad poll unit '{other}' in '{s}': use s, m or h")),
    };
    Ok(Duration::from_secs(secs).max(MIN_POLL))
}

impl AgentConfig {
    pub fn resolved_host(&self) -> String {
        self.host_name
            .clone()
            .or_else(|| {
                std::env::var("PHOTON_AGENT_HOST")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| {
                sysinfo::System::host_name().unwrap_or_else(|| "unknown-host".to_string())
            })
    }

    /// The capabilities this agent admits to. Ordered the way the server ranks them.
    pub fn capabilities(&self) -> Vec<AgentCapability> {
        let mut out = Vec::new();
        if self.docker_read || self.docker_lifecycle || self.docker_apply || self.docker_exec {
            out.push(AgentCapability::Read);
        }
        if self.docker_lifecycle {
            out.push(AgentCapability::Lifecycle);
        }
        if self.docker_apply {
            out.push(AgentCapability::Apply);
        }
        if self.docker_exec {
            out.push(AgentCapability::Exec);
        }
        out
    }

    pub fn any_docker(&self) -> bool {
        !self.capabilities().is_empty()
    }

    pub fn control_channel(&self) -> bool {
        self.server.is_some()
    }

    /// The env lines `photon-agent install` persists — every non-default value, nothing else.
    pub fn env_lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(e) = &self.endpoint {
            v.push(format!("PHOTON_AGENT_ENDPOINT={e}"));
        }
        if self.ingest_token != "dev-ingest-token" {
            v.push(format!("PHOTON_INGEST_TOKEN={}", self.ingest_token));
        }
        if let Some(h) = &self.host_name {
            v.push(format!("PHOTON_AGENT_HOST_NAME={h}"));
        }
        if self.interval_secs != 15 {
            v.push(format!("PHOTON_AGENT_INTERVAL={}", self.interval_secs));
        }
        if self.no_gpu {
            v.push("PHOTON_AGENT_NO_GPU=true".into());
        }
        if self.no_metrics {
            v.push("PHOTON_AGENT_NO_METRICS=true".into());
        }
        if let Some(s) = &self.server {
            v.push(format!("PHOTON_SERVER={s}"));
        }
        if let Some(t) = &self.token {
            v.push(format!("PHOTON_AGENT_TOKEN={t}"));
        }
        if self.docker_read {
            v.push("PHOTON_AGENT_DOCKER_READ=true".into());
        }
        if self.docker_lifecycle {
            v.push("PHOTON_AGENT_DOCKER_LIFECYCLE=true".into());
        }
        if self.docker_apply {
            v.push("PHOTON_AGENT_DOCKER_APPLY=true".into());
        }
        if self.docker_exec {
            v.push("PHOTON_AGENT_DOCKER_EXEC=true".into());
        }
        if self.poll != DEFAULT_POLL {
            v.push(format!("PHOTON_AGENT_POLL={}s", self.poll.as_secs()));
        }
        if let Some(d) = &self.data_dir {
            v.push(format!("PHOTON_AGENT_DATA_DIR={}", d.display()));
        }
        if self.docker_host != DEFAULT_DOCKER_HOST {
            v.push(format!("DOCKER_HOST={}", self.docker_host));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_parses_units_and_clamps() {
        assert_eq!(parse_poll("15s").unwrap(), Duration::from_secs(15));
        assert_eq!(parse_poll("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_poll("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_poll("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_poll(" 45 s ").unwrap(), Duration::from_secs(45));
        // The floor: nothing faster than 5s, whatever the operator typed.
        assert_eq!(parse_poll("1s").unwrap(), MIN_POLL);
        assert_eq!(parse_poll("0").unwrap(), MIN_POLL);
        assert!(parse_poll("").is_err());
        assert!(parse_poll("abc").is_err());
        assert!(parse_poll("15x").is_err());
    }

    fn parse(args: &[&str]) -> Cli {
        <Cli as clap::Parser>::try_parse_from(
            std::iter::once("photon-agent").chain(args.iter().copied()),
        )
        .expect("parse")
    }

    #[test]
    fn capabilities_are_opt_in_and_imply_read() {
        let c = parse(&[]).cfg;
        assert!(c.capabilities().is_empty());
        assert!(!c.any_docker());
        let c = parse(&["--docker-read"]).cfg;
        assert_eq!(c.capabilities(), vec![AgentCapability::Read]);
        let c = parse(&["--docker-apply"]).cfg;
        assert_eq!(
            c.capabilities(),
            vec![AgentCapability::Read, AgentCapability::Apply]
        );
        let c = parse(&["--docker-lifecycle", "--docker-exec"]).cfg;
        assert_eq!(
            c.capabilities(),
            vec![
                AgentCapability::Read,
                AgentCapability::Lifecycle,
                AgentCapability::Exec
            ]
        );
    }

    #[test]
    fn subcommands_and_defaults() {
        let c = parse(&[]);
        assert!(c.command.is_none());
        assert_eq!(c.cfg.poll, DEFAULT_POLL);
        assert_eq!(c.cfg.docker_host, DEFAULT_DOCKER_HOST);
        assert!(c.cfg.endpoint.is_none());
        let c = parse(&[
            "install",
            "--server",
            "http://x:8080",
            "--token",
            "ak_1",
            "--docker-read",
            "--poll",
            "30s",
        ]);
        let Some(Command::Install(cfg)) = c.command else {
            panic!("install")
        };
        assert_eq!(cfg.server.as_deref(), Some("http://x:8080"));
        let env = cfg.env_lines();
        assert!(env.contains(&"PHOTON_SERVER=http://x:8080".to_string()));
        assert!(env.contains(&"PHOTON_AGENT_TOKEN=ak_1".to_string()));
        assert!(env.contains(&"PHOTON_AGENT_DOCKER_READ=true".to_string()));
        assert!(env.contains(&"PHOTON_AGENT_POLL=30s".to_string()));
        assert!(!env.iter().any(|l| l.starts_with("PHOTON_INGEST_TOKEN")));
        assert!(matches!(
            parse(&["version"]).command,
            Some(Command::Version)
        ));
        assert!(matches!(
            parse(&["uninstall"]).command,
            Some(Command::Uninstall)
        ));
    }
}

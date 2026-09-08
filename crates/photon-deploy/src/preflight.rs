//! Dry run / preflight (R5.6): validate a template version against a target host's facts —
//! agent capability, reachability, arch, Docker version, memory, disk, port conflicts — without
//! applying anything. The wording is the frontend mock's, which is what the wizard was designed
//! around.

use crate::model::{
    AgentCapability, AgentStatus, CheckStatus, Container, ContainerState, ManagedHost,
    PreflightCheck, RegistryCredential, TemplateManifest, VariableType,
};
use crate::registry::{credential_for_registry, registries_of, DOCKER_HUB};
use std::collections::BTreeMap;

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Everything preflight needs to know about the target.
pub struct Target<'a> {
    pub host: &'a ManagedHost,
    /// The host's latest observed containers (port conflicts).
    pub containers: &'a [Container],
    /// From the infra vertical when the host reports resource metrics; `None` = unknown.
    pub total_ram_bytes: Option<u64>,
    /// The stack being upgraded in place, so its own ports do not count as conflicts.
    pub stack_id: Option<&'a str>,
    /// The compose project name the deploy will use (its own containers do not conflict).
    pub stack_name: &'a str,
    pub now_ms: i64,
    /// The rendered compose's images, so each registry can be checked for a credential.
    pub images: &'a [String],
    /// The registry credentials this host is scoped to receive.
    pub credentials: &'a [RegistryCredential],
}

fn check(
    key: impl Into<String>,
    label: impl Into<String>,
    status: CheckStatus,
    detail: impl Into<String>,
) -> PreflightCheck {
    PreflightCheck {
        key: key.into(),
        label: label.into(),
        status,
        detail: detail.into(),
    }
}

fn status_label(s: AgentStatus) -> &'static str {
    match s {
        AgentStatus::Online => "Online",
        AgentStatus::Offline => "Offline",
        AgentStatus::Pending => "Waiting for first sync",
    }
}

/// Numeric dotted-version compare (`27.1.1` vs `24.0`): missing components read as 0.
pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.split(['.', '-', '+'])
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (x, y) = (parse(a), parse(b));
    for i in 0..x.len().max(y.len()) {
        let (p, q) = (
            x.get(i).copied().unwrap_or(0),
            y.get(i).copied().unwrap_or(0),
        );
        if p != q {
            return p.cmp(&q);
        }
    }
    std::cmp::Ordering::Equal
}

/// The checks, in the order the wizard lists them. `ok` = no `fail`.
pub fn run(
    m: &TemplateManifest,
    t: &Target<'_>,
    variables: &BTreeMap<String, String>,
    pull_policy: &str,
) -> Vec<PreflightCheck> {
    let mut checks = Vec::new();
    let h = t.host;
    let facts = h.docker.as_ref();

    checks.push(if h.capabilities.contains(&AgentCapability::Apply) {
        check(
            "capability",
            "Agent can apply stacks",
            CheckStatus::Pass,
            "--docker-apply granted",
        )
    } else {
        check(
            "capability",
            "Agent can apply stacks",
            CheckStatus::Fail,
            "installed without --docker-apply; reinstall with it",
        )
    });
    checks.push(if h.status == AgentStatus::Online {
        let ago = (t.now_ms - h.last_seen_ms.unwrap_or(0)).max(0) / 1000;
        check(
            "online",
            "Host is reachable",
            CheckStatus::Pass,
            format!("synced {ago}s ago"),
        )
    } else {
        check(
            "online",
            "Host is reachable",
            CheckStatus::Warn,
            format!(
                "{} — the deploy waits as pending and applies on the next sync",
                status_label(h.status)
            ),
        )
    });

    if let Some(arch) = m.requires.arch.as_ref().filter(|a| !a.is_empty()) {
        checks.push(match facts {
            None => check(
                "arch",
                "CPU architecture",
                CheckStatus::Skip,
                "unknown until the first sync",
            ),
            Some(f) if arch.iter().any(|a| a == &f.arch) => check(
                "arch",
                "CPU architecture",
                CheckStatus::Pass,
                f.arch.clone(),
            ),
            Some(f) => check(
                "arch",
                "CPU architecture",
                CheckStatus::Fail,
                format!("host is {}; template needs {}", f.arch, arch.join(" or ")),
            ),
        });
    }
    if let Some(min) = m.requires.min_docker.as_deref().filter(|s| !s.is_empty()) {
        checks.push(match facts {
            None => check(
                "docker",
                "Docker version",
                CheckStatus::Skip,
                "unknown until the first sync",
            ),
            Some(f) if cmp_version(&f.version, min) != std::cmp::Ordering::Less => check(
                "docker",
                "Docker version",
                CheckStatus::Pass,
                format!("{} ≥ {min}", f.version),
            ),
            Some(f) => check(
                "docker",
                "Docker version",
                CheckStatus::Fail,
                format!("{} < {min} required", f.version),
            ),
        });
    }
    if let Some(min) = m.requires.min_ram_bytes.filter(|b| *b > 0) {
        checks.push(match t.total_ram_bytes {
            None => check(
                "ram",
                "Memory",
                CheckStatus::Skip,
                "no resource metrics from this host",
            ),
            Some(ram) if ram >= min => check(
                "ram",
                "Memory",
                CheckStatus::Pass,
                format!("{:.0} GB ≥ {:.0} GB", ram as f64 / GB, min as f64 / GB),
            ),
            Some(ram) => check(
                "ram",
                "Memory",
                CheckStatus::Fail,
                format!(
                    "{:.0} GB < {:.0} GB required",
                    ram as f64 / GB,
                    min as f64 / GB
                ),
            ),
        });
    }
    if let Some(min) = m.requires.min_disk_bytes.filter(|b| *b > 0) {
        let free = facts.and_then(|f| match (f.disk.total_bytes, f.disk.used_bytes) {
            (Some(t), Some(u)) => Some(t.saturating_sub(u)),
            _ => None,
        });
        checks.push(match free {
            None => check("disk", "Free disk", CheckStatus::Skip, "unknown until the first sync"),
            Some(free) if free >= min => {
                check("disk", "Free disk", CheckStatus::Pass, format!("{:.0} GB free ≥ {:.0} GB", free as f64 / GB, min as f64 / GB))
            }
            Some(free) if free as f64 >= min as f64 * 0.5 => check(
                "disk",
                "Free disk",
                CheckStatus::Warn,
                format!(
                    "{:.0} GB free — under the {:.0} GB the template asks for; a prune frees {:.1} GB",
                    free as f64 / GB,
                    min as f64 / GB,
                    facts.map(|f| f.disk.reclaimable_bytes).unwrap_or(0) as f64 / GB
                ),
            ),
            Some(free) => check("disk", "Free disk", CheckStatus::Fail, format!("{:.0} GB free < {:.0} GB required", free as f64 / GB, min as f64 / GB)),
        });
    }

    for p in &m.ports {
        // The port variable whose default matches this port, if the template exposes one.
        let var = m.variables.iter().find(|v| {
            v.kind == VariableType::Port
                && v.default.as_deref().and_then(|d| d.parse::<u16>().ok()) == Some(p.default_host)
        });
        let wanted = var
            .and_then(|v| variables.get(&v.name))
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(p.default_host);
        let holder = t.containers.iter().find(|c| {
            c.state == ContainerState::Running
                && c.ports.iter().any(|cp| cp.host_port == Some(wanted))
                && !(t.stack_id.is_some() && c.managed_stack_id.as_deref() == t.stack_id)
                && c.compose_project.as_deref() != Some(t.stack_name)
        });
        let key = format!("port-{}", p.name);
        let label = format!("Port {wanted} ({})", p.name);
        checks.push(match holder {
            Some(c) => check(
                key,
                label,
                CheckStatus::Fail,
                format!("already published by {}", c.name),
            ),
            None => check(key, label, CheckStatus::Pass, "free"),
        });
    }
    checks.extend(registry_checks(t.images, t.credentials, pull_policy));
    checks
}

/// One check per registry the images pull from: which credential the agent will present, or
/// that none is stored. Whether an image is private is not knowable from here, so a missing
/// credential is a warning (the pull fails only if it needed one), never a failure; Docker Hub
/// without one is the ordinary anonymous case. `never` skips the lot — nothing gets pulled.
pub fn registry_checks(
    images: &[String],
    credentials: &[RegistryCredential],
    pull_policy: &str,
) -> Vec<PreflightCheck> {
    let registries = registries_of(images.iter().map(|s| s.as_str()));
    if pull_policy == "never" {
        return vec![check(
            "images",
            "Images reachable",
            CheckStatus::Skip,
            "pull policy is never: the host's existing images are used",
        )];
    }
    if registries.is_empty() {
        return vec![check(
            "images",
            "Images reachable",
            CheckStatus::Skip,
            format!("checked by the agent at apply (pull policy: {pull_policy})"),
        )];
    }
    registries
        .iter()
        .map(|r| {
            let key = format!("registry:{r}");
            let label = format!("Pull from {r}");
            match credential_for_registry(r, credentials) {
                Some(c) => check(
                    key,
                    label,
                    CheckStatus::Pass,
                    format!(
                        "as {}{}",
                        c.username,
                        c.label
                            .as_deref()
                            .filter(|l| !l.is_empty())
                            .map(|l| format!(" ({l})"))
                            .unwrap_or_default()
                    ),
                ),
                None if r == DOCKER_HUB => check(
                    key,
                    label,
                    CheckStatus::Skip,
                    "anonymous — fine for public images",
                ),
                None => check(
                    key,
                    label,
                    CheckStatus::Warn,
                    "no credential stored for this registry; the pull fails if the image is private — add one under Templates › Sources › Image registries",
                ),
            }
        })
        .collect()
}

pub fn passed(checks: &[PreflightCheck]) -> bool {
    !checks.iter().any(|c| c.status == CheckStatus::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn manifest() -> TemplateManifest {
        serde_json::from_str(
            r#"{"id":"t","version":"1.0.0","name":"T",
                "variables":[{"name":"API_PORT","label":"API","type":"port","required":true,"default":"8443"}],
                "requires":{"arch":["x86_64"],"min_docker":"24.0","min_disk_bytes":42949672960},
                "ports":[{"name":"api","container":8443,"default_host":8443}]}"#,
        )
        .unwrap()
    }

    fn host(online: bool, apply: bool, facts: Option<DockerFacts>) -> ManagedHost {
        ManagedHost {
            id: "h".into(),
            host: "edge".into(),
            site: None,
            status: if online {
                AgentStatus::Online
            } else {
                AgentStatus::Offline
            },
            agent_version: None,
            latest_agent_version: None,
            capabilities: if apply {
                vec![AgentCapability::Apply]
            } else {
                vec![]
            },
            poll_secs: 15,
            last_seen_ms: Some(1_000),
            enrolled_ms: 0,
            docker: facts,
            labels: Default::default(),
        }
    }

    fn container(name: &str, port: u16, project: Option<&str>) -> Container {
        Container {
            id: name.into(),
            name: name.into(),
            image: "x".into(),
            image_id: "".into(),
            command: "".into(),
            state: ContainerState::Running,
            health: ContainerHealth::None,
            status_text: "".into(),
            created_ms: 0,
            started_ms: None,
            finished_ms: None,
            exit_code: None,
            restart_count: 0,
            restart_policy: "".into(),
            ports: vec![ContainerPort {
                host_ip: None,
                host_port: Some(port),
                container_port: port,
                protocol: "tcp".into(),
            }],
            mounts: vec![],
            networks: vec![],
            labels: Default::default(),
            env_keys: vec![],
            env: Default::default(),
            compose_project: project.map(|s| s.into()),
            compose_service: None,
            managed_stack_id: None,
            cpu_pct: None,
            mem_bytes: None,
            mem_limit_bytes: None,
            net_rx_bps: None,
            net_tx_bps: None,
            block_read_bps: None,
            block_write_bps: None,
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(cmp_version("27.1.1", "24.0"), std::cmp::Ordering::Greater);
        assert_eq!(cmp_version("24.0", "24.0.0"), std::cmp::Ordering::Equal);
        assert_eq!(cmp_version("9.9", "10"), std::cmp::Ordering::Less);
    }

    #[test]
    fn a_taken_port_and_a_full_disk_fail_but_the_own_project_does_not() {
        let m = manifest();
        let facts = DockerFacts {
            version: "27.1.1".into(),
            arch: "x86_64".into(),
            disk: DockerDisk {
                total_bytes: Some(100 << 30),
                used_bytes: Some(95 << 30),
                ..Default::default()
            },
            ..Default::default()
        };
        let h = host(true, true, Some(facts));
        let taken = vec![container("other-api", 8443, Some("other"))];
        let t = Target {
            host: &h,
            containers: &taken,
            total_ram_bytes: None,
            stack_id: None,
            stack_name: "vas",
            now_ms: 61_000,
            images: &[],
            credentials: &[],
        };
        let checks = run(&m, &t, &BTreeMap::new(), "missing");
        assert!(!passed(&checks));
        let port = checks.iter().find(|c| c.key == "port-api").unwrap();
        assert_eq!(
            (port.status, port.detail.as_str()),
            (CheckStatus::Fail, "already published by other-api")
        );
        let disk = checks.iter().find(|c| c.key == "disk").unwrap();
        assert_eq!(disk.status, CheckStatus::Fail);
        assert!(checks
            .iter()
            .any(|c| c.key == "online" && c.detail == "synced 60s ago"));

        let own = vec![container("vas-api-1", 8443, Some("vas"))];
        let t = Target {
            host: &h,
            containers: &own,
            total_ram_bytes: None,
            stack_id: None,
            stack_name: "vas",
            now_ms: 61_000,
            images: &[],
            credentials: &[],
        };
        let checks = run(
            &m,
            &t,
            &BTreeMap::from([("API_PORT".to_string(), "8443".to_string())]),
            "missing",
        );
        assert_eq!(
            checks.iter().find(|c| c.key == "port-api").unwrap().status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn no_apply_and_offline() {
        let m = manifest();
        let h = host(false, false, None);
        let t = Target {
            host: &h,
            containers: &[],
            total_ram_bytes: None,
            stack_id: None,
            stack_name: "vas",
            now_ms: 0,
            images: &[],
            credentials: &[],
        };
        let checks = run(&m, &t, &BTreeMap::new(), "always");
        assert_eq!(checks[0].status, CheckStatus::Fail);
        assert_eq!(checks[1].status, CheckStatus::Warn);
        assert!(
            checks
                .iter()
                .filter(|c| c.status == CheckStatus::Skip)
                .count()
                >= 3,
            "arch/docker/disk unknown before the first sync"
        );
    }

    fn cred(server: &str, user: &str) -> RegistryCredential {
        RegistryCredential {
            id: format!("rc_{server}"),
            server: server.into(),
            username: user.into(),
            label: Some("deploy token".into()),
            scope: None,
            created_ms: 0,
            created_by: "admin".into(),
            updated_ms: 0,
        }
    }

    #[test]
    fn each_registry_says_which_credential_it_pulls_with() {
        let images = vec![
            "registry.gitlab.com/nodefluxio/lenz:1.0".to_string(),
            "timescale/timescaledb-ha:pg17".to_string(),
            "ghcr.io/org/app".to_string(),
        ];
        let creds = vec![cred("https://registry.gitlab.com", "deploy")];
        let checks = registry_checks(&images, &creds, "missing");
        let by_key: BTreeMap<&str, &PreflightCheck> =
            checks.iter().map(|c| (c.key.as_str(), c)).collect();
        let gl = by_key["registry:registry.gitlab.com"];
        assert_eq!(gl.status, CheckStatus::Pass);
        assert_eq!(gl.detail, "as deploy (deploy token)");
        assert_eq!(by_key["registry:docker.io"].status, CheckStatus::Skip);
        assert_eq!(by_key["registry:ghcr.io"].status, CheckStatus::Warn);
        assert!(passed(&checks), "a missing credential warns, never blocks");
        // Nothing is pulled under `never`, so nothing is checked.
        let none = registry_checks(&images, &creds, "never");
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].status, CheckStatus::Skip);
        // No images known: the old generic line.
        assert_eq!(registry_checks(&[], &creds, "always")[0].key, "images");
    }
}

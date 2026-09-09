//! The `read` capability: one `Observed` document per sync (R2.1, R2.5, R2.6). Built from
//! `info` + `version` + `system df` for the facts, `list_containers(all)` + one `inspect` per
//! container (bounded concurrency), one one-shot `stats` per RUNNING container (skipped past
//! `STATS_MAX_RUNNING` — on a 200-container host that is 200 extra round trips per sync, and
//! the live stats stream exists for the container someone is actually looking at), plus the
//! image / volume / network lists. Compose projects are grouped from the labels Docker itself
//! writes, so a stack Photon deployed and one somebody `docker compose up`'d by hand look the
//! same (R2.6).
//!
//! `system df` walks every container's writable layer and every volume, which on a busy host
//! takes seconds, so its answer is cached for `DF_TTL` and refreshed in the background of the
//! sync cadence rather than paid on every poll.
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use bollard::models::{
    ContainerInspectResponse, ContainerStatsResponse, ContainerSummary, HealthStatusEnum,
    ImageSummary, Network, SystemDataUsageResponse, Volume,
};
use bollard::query_parameters::{
    ListContainersOptionsBuilder, ListImagesOptionsBuilder, ListVolumesOptions, StatsOptionsBuilder,
};
use bollard::Docker;
use futures::StreamExt;
use photon_deploy::model::{
    Container, ContainerHealth, ContainerMount, ContainerPort, ContainerState, DockerDisk,
    DockerFacts, ImageInfo, NetworkInfo, Observed, ObservedStack, PruneCategory, PrunePreview,
    VolumeInfo,
};

use super::compose::STACK_LABEL;
use super::split_env;
use super::stats::{Counters, CpuSample, RateTracker};
use crate::clock::{now_ms, parse_rfc3339_ms};

pub const STATS_MAX_RUNNING: usize = 60;
pub const INSPECT_CONCURRENCY: usize = 8;
pub const DF_TTL: Duration = Duration::from_secs(60);

pub const LABEL_PROJECT: &str = "com.docker.compose.project";
pub const LABEL_SERVICE: &str = "com.docker.compose.service";
pub const LABEL_CONFIG_HASH: &str = "com.docker.compose.config-hash";
pub const LABEL_WORKING_DIR: &str = "com.docker.compose.project.working_dir";
pub const LABEL_CONFIG_FILES: &str = "com.docker.compose.project.config_files";

fn es(e: bollard::errors::Error) -> String {
    e.to_string()
}

/// Per-agent inventory state that survives between syncs: the rate tracker (deltas for the
/// `*_bps` and `cpu_pct` fields) and the `df` cache.
#[derive(Default)]
pub struct Inventory {
    tracker: RateTracker,
    df: Option<(Instant, SystemDataUsageResponse)>,
}

impl Inventory {
    /// `applied_generation(project_name)` is the local state's answer for projects this agent
    /// applied itself; anything else is a hand-deployed project (`None`).
    pub async fn observe(
        &mut self,
        docker: &Docker,
        host: &str,
        applied_generation: &(dyn Fn(&str) -> Option<u64> + Sync),
    ) -> Result<Observed, String> {
        let info = docker.info().await.map_err(es)?;
        let version = docker.version().await.map_err(es)?;
        let df = self.df(docker).await;

        // Containers: list (status text, image name) + inspect (everything else).
        let summaries = docker
            .list_containers(Some(ListContainersOptionsBuilder::new().all(true).build()))
            .await
            .map_err(es)?;
        let inspected: Vec<(ContainerSummary, Option<ContainerInspectResponse>)> =
            futures::stream::iter(summaries)
                .map(|s| async move {
                    let id = s.id.clone().unwrap_or_default();
                    let insp = docker.inspect_container(&id, None).await.ok();
                    (s, insp)
                })
                .buffer_unordered(INSPECT_CONCURRENCY)
                .collect()
                .await;
        let mut containers = Vec::with_capacity(inspected.len());
        let mut volume_users: HashMap<String, Vec<String>> = HashMap::new();
        for (summary, insp) in inspected {
            let Some(insp) = insp else { continue }; // vanished between list and inspect
            let (c, volumes) = build_container(&summary, insp);
            for v in volumes {
                volume_users.entry(v).or_default().push(c.name.clone());
            }
            containers.push(c);
        }
        containers.sort_by(|a, b| a.name.cmp(&b.name));

        // Stats for running containers, bounded.
        let running: Vec<String> = containers
            .iter()
            .filter(|c| c.state == ContainerState::Running)
            .map(|c| c.id.clone())
            .collect();
        if running.len() <= STATS_MAX_RUNNING {
            let readings: Vec<(String, Option<ContainerStatsResponse>)> =
                futures::stream::iter(running.clone())
                    .map(|id| async move {
                        let opts = StatsOptionsBuilder::new()
                            .stream(false)
                            .one_shot(true)
                            .build();
                        let r = docker
                            .stats(&id, Some(opts))
                            .next()
                            .await
                            .and_then(|r| r.ok());
                        (id, r)
                    })
                    .buffer_unordered(INSPECT_CONCURRENCY)
                    .collect()
                    .await;
            let now = now_ms();
            for (id, reading) in readings {
                let Some(reading) = reading else { continue };
                let Some(c) = containers.iter_mut().find(|c| c.id == id) else {
                    continue;
                };
                let (cpu, counters, mem, mem_limit) = read_stats(&reading, now);
                let rates = self.tracker.update(&id, cpu, counters);
                c.cpu_pct = rates.cpu_pct;
                c.mem_bytes = mem;
                c.mem_limit_bytes = mem_limit;
                c.net_rx_bps = rates.net_rx_bps;
                c.net_tx_bps = rates.net_tx_bps;
                c.block_read_bps = rates.block_read_bps;
                c.block_write_bps = rates.block_write_bps;
            }
        }
        self.tracker.retain_only(&running);

        // Images.
        let images_raw = docker
            .list_images(Some(ListImagesOptionsBuilder::new().all(false).build()))
            .await
            .map_err(es)?;
        let mut images: Vec<ImageInfo> = images_raw
            .iter()
            .map(|i| build_image(i, &containers))
            .collect();
        images.sort_by_key(|a| std::cmp::Reverse(a.created_ms));

        // Volumes.
        let volume_sizes = df.as_ref().map(volume_sizes_from_df).unwrap_or_default();
        let volumes_raw = docker
            .list_volumes(None::<ListVolumesOptions>)
            .await
            .map_err(es)?;
        let mut volumes: Vec<VolumeInfo> = volumes_raw
            .volumes
            .unwrap_or_default()
            .iter()
            .map(|v| build_volume(v, &volume_users, &volume_sizes))
            .collect();
        volumes.sort_by(|a, b| a.name.cmp(&b.name));

        // Networks.
        let networks_raw = docker.list_networks(None).await.map_err(es)?;
        let mut networks: Vec<NetworkInfo> = networks_raw
            .iter()
            .map(|n| build_network(n, &containers))
            .collect();
        networks.sort_by(|a, b| a.name.cmp(&b.name));

        let facts = build_facts(&info, &version, df.as_ref());
        let stacks = group_stacks(&containers, applied_generation);
        let prune_preview = Some(prune_preview(
            host,
            df.as_ref(),
            &containers,
            &images,
            &volumes,
        ));

        Ok(Observed {
            facts: Some(facts),
            containers,
            images,
            volumes,
            networks,
            stacks,
            prune_preview,
        })
    }

    async fn df(&mut self, docker: &Docker) -> Option<SystemDataUsageResponse> {
        if let Some((at, cached)) = &self.df {
            if at.elapsed() < DF_TTL {
                return Some(cached.clone());
            }
        }
        match docker.df(None).await {
            Ok(v) => {
                self.df = Some((Instant::now(), v.clone()));
                Some(v)
            }
            Err(e) => {
                eprintln!("photon-agent: docker df failed: {e}");
                self.df.as_ref().map(|(_, v)| v.clone())
            }
        }
    }
}

fn build_facts(
    info: &bollard::models::SystemInfo,
    version: &bollard::models::SystemVersion,
    df: Option<&SystemDataUsageResponse>,
) -> DockerFacts {
    let mut disk = DockerDisk::default();
    if let Some(df) = df {
        let total = |u: &Option<i64>| u.map(|v| v.max(0) as u64).unwrap_or(0);
        disk.images_bytes = df
            .image_usage
            .as_ref()
            .map(|u| total(&u.total_size))
            .unwrap_or(0);
        disk.volumes_bytes = df
            .volume_usage
            .as_ref()
            .map(|u| total(&u.total_size))
            .unwrap_or(0);
        disk.build_cache_bytes = df
            .build_cache_usage
            .as_ref()
            .map(|u| total(&u.total_size))
            .unwrap_or(0);
        disk.reclaimable_bytes = df
            .image_usage
            .as_ref()
            .map(|u| total(&u.reclaimable))
            .unwrap_or(0)
            + df.container_usage
                .as_ref()
                .map(|u| total(&u.reclaimable))
                .unwrap_or(0)
            + df.volume_usage
                .as_ref()
                .map(|u| total(&u.reclaimable))
                .unwrap_or(0)
            + df.build_cache_usage
                .as_ref()
                .map(|u| total(&u.reclaimable))
                .unwrap_or(0);
    }
    if let Some((total, used)) = info.docker_root_dir.as_deref().and_then(fs_usage) {
        disk.total_bytes = Some(total);
        disk.used_bytes = Some(used);
    }
    DockerFacts {
        version: version
            .version
            .clone()
            .or_else(|| info.server_version.clone())
            .unwrap_or_default(),
        api_version: version.api_version.clone().unwrap_or_default(),
        storage_driver: info.driver.clone().unwrap_or_default(),
        os: info
            .operating_system
            .clone()
            .or_else(|| version.os.clone())
            .unwrap_or_default(),
        arch: info
            .architecture
            .clone()
            .or_else(|| version.arch.clone())
            .unwrap_or_default(),
        containers_running: info.containers_running.unwrap_or(0).max(0) as u32,
        containers_total: info.containers.unwrap_or(0).max(0) as u32,
        images: info.images.unwrap_or(0).max(0) as u32,
        disk,
    }
}

/// `(total, used)` bytes of the filesystem holding `path`, via `statvfs`. `None` when the path
/// is not visible from here (agent in a container without the daemon's root mounted).
#[cfg(unix)]
pub fn fs_usage(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized out-parameter.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = st.f_frsize as u64;
    let total = st.f_blocks as u64 * frsize;
    let free = st.f_bfree as u64 * frsize;
    Some((total, total.saturating_sub(free)))
}
#[cfg(not(unix))]
pub fn fs_usage(_path: &str) -> Option<(u64, u64)> {
    None
}

/// One `Container` plus the names of the volumes it mounts (for `VolumeInfo.used_by`).
pub fn build_container(
    summary: &ContainerSummary,
    insp: ContainerInspectResponse,
) -> (Container, Vec<String>) {
    let state = insp.state.unwrap_or_default();
    let config = insp.config.unwrap_or_default();
    let host_config = insp.host_config.unwrap_or_default();
    let net = insp.network_settings.unwrap_or_default();
    let labels: BTreeMap<String, String> = config.labels.unwrap_or_default().into_iter().collect();
    let (env_keys, env) = split_env(&config.env.unwrap_or_default());

    let status = state.status.map(|s| s.to_string()).unwrap_or_default();
    let health = match state.health.and_then(|h| h.status) {
        Some(HealthStatusEnum::HEALTHY) => ContainerHealth::Healthy,
        Some(HealthStatusEnum::UNHEALTHY) => ContainerHealth::Unhealthy,
        Some(HealthStatusEnum::STARTING) => ContainerHealth::Starting,
        _ => ContainerHealth::None,
    };
    let restart_policy = host_config
        .restart_policy
        .and_then(|p| p.name)
        .map(|n| n.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "no".to_string());

    let mut ports = Vec::new();
    for (key, bindings) in net.ports.unwrap_or_default() {
        let (port, proto) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
        let Ok(container_port) = port.parse::<u16>() else {
            continue;
        };
        match bindings {
            Some(b) if !b.is_empty() => {
                for pb in b {
                    ports.push(ContainerPort {
                        host_ip: pb.host_ip.filter(|s| !s.is_empty()),
                        host_port: pb.host_port.and_then(|p| p.parse().ok()),
                        container_port,
                        protocol: proto.to_string(),
                    });
                }
            }
            _ => ports.push(ContainerPort {
                host_ip: None,
                host_port: None,
                container_port,
                protocol: proto.to_string(),
            }),
        }
    }
    ports.sort_by_key(|p| (p.container_port, p.host_port));

    let mut volume_names = Vec::new();
    let mounts = insp
        .mounts
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            let kind = m.typ.unwrap_or_default();
            if kind == "volume" {
                if let Some(n) = &m.name {
                    volume_names.push(n.clone());
                }
            }
            ContainerMount {
                kind,
                source: m.source.or(m.name).unwrap_or_default(),
                destination: m.destination.unwrap_or_default(),
                rw: m.rw.unwrap_or(true),
            }
        })
        .collect();

    let mut networks: Vec<String> = net.networks.unwrap_or_default().into_keys().collect();
    networks.sort();

    let id = insp
        .id
        .clone()
        .or_else(|| summary.id.clone())
        .unwrap_or_default();
    let name = insp
        .name
        .as_deref()
        .map(|n| n.trim_start_matches('/').to_string())
        .or_else(|| {
            summary
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
        })
        .unwrap_or_else(|| id.chars().take(12).collect());

    let container = Container {
        id,
        name,
        image: summary
            .image
            .clone()
            .or_else(|| config.image.clone())
            .unwrap_or_default(),
        image_id: insp
            .image
            .clone()
            .or_else(|| summary.image_id.clone())
            .unwrap_or_default(),
        command: summary.command.clone().unwrap_or_else(|| {
            let mut parts = vec![insp.path.clone().unwrap_or_default()];
            parts.extend(insp.args.clone().unwrap_or_default());
            parts.join(" ").trim().to_string()
        }),
        state: ContainerState::parse(&status),
        health,
        status_text: summary.status.clone().unwrap_or_default(),
        created_ms: insp
            .created
            .as_deref()
            .and_then(parse_rfc3339_ms)
            .unwrap_or(0),
        started_ms: state.started_at.as_deref().and_then(parse_rfc3339_ms),
        finished_ms: state.finished_at.as_deref().and_then(parse_rfc3339_ms),
        exit_code: state.exit_code,
        restart_count: insp.restart_count.unwrap_or(0).max(0) as u32,
        restart_policy,
        ports,
        mounts,
        networks,
        compose_project: labels.get(LABEL_PROJECT).cloned(),
        compose_service: labels.get(LABEL_SERVICE).cloned(),
        managed_stack_id: labels.get(STACK_LABEL).cloned(),
        labels,
        env_keys,
        env,
        cpu_pct: None,
        mem_bytes: None,
        mem_limit_bytes: None,
        net_rx_bps: None,
        net_tx_bps: None,
        block_read_bps: None,
        block_write_bps: None,
    };
    (container, volume_names)
}

/// Pull the numbers out of one stats reading: CPU sample, cumulative counters, memory usage
/// (minus the page cache, the way `docker stats` shows it) and the limit.
pub fn read_stats(
    r: &ContainerStatsResponse,
    ts_ms: i64,
) -> (Option<CpuSample>, Counters, Option<u64>, Option<u64>) {
    let cpu = r.cpu_stats.as_ref().and_then(|c| {
        Some(CpuSample {
            total_ns: c.cpu_usage.as_ref()?.total_usage?,
            system_ns: c.system_cpu_usage?,
            online_cpus: c.online_cpus.unwrap_or(0),
        })
    });
    let (mut rx, mut tx) = (0u64, 0u64);
    for n in r
        .networks
        .as_ref()
        .map(|m| m.values().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        rx += n.rx_bytes.unwrap_or(0);
        tx += n.tx_bytes.unwrap_or(0);
    }
    let (mut rd, mut wr) = (0u64, 0u64);
    for e in r
        .blkio_stats
        .as_ref()
        .and_then(|b| b.io_service_bytes_recursive.as_ref())
        .map(|v| v.as_slice())
        .unwrap_or_default()
    {
        match e.op.as_deref().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("read") => rd += e.value.unwrap_or(0),
            Some("write") => wr += e.value.unwrap_or(0),
            _ => {}
        }
    }
    let mem = r.memory_stats.as_ref().and_then(|m| {
        let usage = m.usage?;
        let cache = m
            .stats
            .as_ref()
            .and_then(|s| {
                s.get("inactive_file")
                    .or_else(|| s.get("total_inactive_file"))
                    .or_else(|| s.get("cache"))
            })
            .copied()
            .unwrap_or(0);
        Some(usage.saturating_sub(cache))
    });
    // A limit equal to the host's RAM means "unlimited"; still worth reporting as the ceiling.
    let mem_limit = r
        .memory_stats
        .as_ref()
        .and_then(|m| m.limit)
        .filter(|l| *l > 0);
    (
        cpu,
        Counters {
            ts_ms,
            net_rx: rx,
            net_tx: tx,
            block_read: rd,
            block_write: wr,
        },
        mem,
        mem_limit,
    )
}

fn build_image(i: &ImageSummary, containers: &[Container]) -> ImageInfo {
    let repo_tags: Vec<String> = i
        .repo_tags
        .iter()
        .filter(|t| *t != "<none>:<none>")
        .cloned()
        .collect();
    ImageInfo {
        id: i.id.clone(),
        used_by: containers.iter().filter(|c| c.image_id == i.id).count() as u32,
        dangling: repo_tags.is_empty(),
        repo_tags,
        size_bytes: i.size.max(0) as u64,
        created_ms: i.created * 1000,
    }
}

/// `system df` lists each volume with its measured size; the plain volume list does not.
fn volume_sizes_from_df(df: &SystemDataUsageResponse) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for item in df
        .volume_usage
        .as_ref()
        .and_then(|u| u.items.as_ref())
        .map(|v| v.as_slice())
        .unwrap_or_default()
    {
        let Some(name) = item.get("Name").and_then(|n| n.as_str()) else {
            continue;
        };
        if let Some(size) = item
            .get("UsageData")
            .and_then(|u| u.get("Size"))
            .and_then(|s| s.as_i64())
        {
            if size >= 0 {
                out.insert(name.to_string(), size as u64);
            }
        }
    }
    out
}

fn build_volume(
    v: &Volume,
    users: &HashMap<String, Vec<String>>,
    sizes: &HashMap<String, u64>,
) -> VolumeInfo {
    let mut used_by = users.get(&v.name).cloned().unwrap_or_default();
    used_by.sort();
    used_by.dedup();
    VolumeInfo {
        name: v.name.clone(),
        driver: v.driver.clone(),
        mountpoint: v.mountpoint.clone(),
        size_bytes: sizes
            .get(&v.name)
            .copied()
            .or_else(|| v.usage_data.as_ref().map(|u| u.size.max(0) as u64)),
        used_by,
        created_ms: v
            .created_at
            .as_deref()
            .and_then(parse_rfc3339_ms)
            .unwrap_or(0),
    }
}

fn build_network(n: &Network, containers: &[Container]) -> NetworkInfo {
    let name = n.name.clone().unwrap_or_default();
    NetworkInfo {
        id: n.id.clone().unwrap_or_default(),
        driver: n.driver.clone().unwrap_or_default(),
        scope: n.scope.clone().unwrap_or_default(),
        internal: n.internal.unwrap_or(false),
        containers: containers
            .iter()
            .filter(|c| c.networks.contains(&name))
            .count() as u32,
        subnet: n
            .ipam
            .as_ref()
            .and_then(|i| i.config.as_ref())
            .and_then(|c| c.first())
            .and_then(|c| c.subnet.clone()),
        name,
    }
}

/// Group containers into compose projects. Pure over the container list, so it is testable
/// without a daemon.
pub fn group_stacks(
    containers: &[Container],
    applied_generation: &dyn Fn(&str) -> Option<u64>,
) -> Vec<ObservedStack> {
    let mut by_project: BTreeMap<String, ObservedStack> = BTreeMap::new();
    for c in containers {
        let Some(project) = &c.compose_project else {
            continue;
        };
        let entry = by_project
            .entry(project.clone())
            .or_insert_with(|| ObservedStack {
                name: project.clone(),
                applied_generation: applied_generation(project),
                config_hashes: BTreeMap::new(),
                working_dir: None,
                compose_file: None,
            });
        if let (Some(svc), Some(hash)) = (&c.compose_service, c.labels.get(LABEL_CONFIG_HASH)) {
            entry.config_hashes.insert(svc.clone(), hash.clone());
        }
        if entry.working_dir.is_none() {
            entry.working_dir = c.labels.get(LABEL_WORKING_DIR).cloned();
        }
        if entry.compose_file.is_none() {
            entry.compose_file = c.labels.get(LABEL_CONFIG_FILES).cloned();
        }
    }
    by_project.into_values().collect()
}

fn prune_preview(
    host: &str,
    df: Option<&SystemDataUsageResponse>,
    containers: &[Container],
    images: &[ImageInfo],
    volumes: &[VolumeInfo],
) -> PrunePreview {
    let reclaim =
        |u: Option<&Option<i64>>| u.and_then(|v| *v).map(|v| v.max(0) as u64).unwrap_or(0);
    let counts = |total: Option<&Option<i64>>, active: Option<&Option<i64>>| {
        let t = total.and_then(|v| *v).unwrap_or(0);
        let a = active.and_then(|v| *v).unwrap_or(0);
        (t - a).max(0) as u64
    };
    let img = df.and_then(|d| d.image_usage.as_ref());
    let con = df.and_then(|d| d.container_usage.as_ref());
    let vol = df.and_then(|d| d.volume_usage.as_ref());
    let bc = df.and_then(|d| d.build_cache_usage.as_ref());
    PrunePreview {
        host: host.to_string(),
        images: PruneCategory {
            count: images.iter().filter(|i| i.used_by == 0).count() as u64,
            bytes: reclaim(img.map(|u| &u.reclaimable)),
        },
        containers: PruneCategory {
            count: containers
                .iter()
                .filter(|c| c.state != ContainerState::Running)
                .count() as u64,
            bytes: reclaim(con.map(|u| &u.reclaimable)),
        },
        volumes: PruneCategory {
            count: volumes.iter().filter(|v| v.used_by.is_empty()).count() as u64,
            bytes: reclaim(vol.map(|u| &u.reclaimable)),
        },
        build_cache: PruneCategory {
            count: counts(bc.map(|u| &u.total_count), bc.map(|u| &u.active_count)),
            bytes: reclaim(bc.map(|u| &u.reclaimable)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{
        ContainerConfig, ContainerState as BState, ContainerStateStatusEnum, HostConfig,
        MountPoint, NetworkSettings, PortBinding, RestartPolicy, RestartPolicyNameEnum,
    };

    fn inspect(name: &str, labels: &[(&str, &str)], env: &[&str]) -> ContainerInspectResponse {
        ContainerInspectResponse {
            id: Some(format!("{name}-id")),
            name: Some(format!("/{name}")),
            created: Some("2024-01-01T00:00:00.5Z".into()),
            image: Some("sha256:abc".into()),
            restart_count: Some(3),
            state: Some(BState {
                status: Some(ContainerStateStatusEnum::RUNNING),
                started_at: Some("2024-01-01T00:01:00Z".into()),
                finished_at: Some("0001-01-01T00:00:00Z".into()),
                exit_code: Some(0),
                ..Default::default()
            }),
            config: Some(ContainerConfig {
                image: Some("nginx:1".into()),
                labels: Some(
                    labels
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                ),
                env: Some(env.iter().map(|s| s.to_string()).collect()),
                ..Default::default()
            }),
            host_config: Some(HostConfig {
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                    maximum_retry_count: None,
                }),
                ..Default::default()
            }),
            mounts: Some(vec![MountPoint {
                typ: Some("volume".into()),
                name: Some("data".into()),
                source: Some("/var/lib/docker/volumes/data/_data".into()),
                destination: Some("/data".into()),
                rw: Some(true),
                ..Default::default()
            }]),
            network_settings: Some(NetworkSettings {
                ports: Some(HashMap::from([
                    (
                        "80/tcp".to_string(),
                        Some(vec![PortBinding {
                            host_ip: Some("0.0.0.0".into()),
                            host_port: Some("8080".into()),
                        }]),
                    ),
                    ("9000/udp".to_string(), None),
                ])),
                networks: Some(HashMap::from([(
                    "web_default".to_string(),
                    Default::default(),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn container_maps_labels_ports_mounts_env_and_times() {
        let summary = ContainerSummary {
            id: Some("web-id".into()),
            image: Some("nginx:1".into()),
            status: Some("Up 3 hours".into()),
            command: Some("nginx -g 'daemon off;'".into()),
            ..Default::default()
        };
        let insp = inspect(
            "web",
            &[
                (LABEL_PROJECT, "shop"),
                (LABEL_SERVICE, "web"),
                (LABEL_CONFIG_HASH, "h1"),
                (STACK_LABEL, "stk_1"),
            ],
            &["PATH=/bin", "DB_PASSWORD=pw"],
        );
        let (c, vols) = build_container(&summary, insp);
        assert_eq!(c.name, "web");
        assert_eq!(c.image, "nginx:1");
        assert_eq!(c.image_id, "sha256:abc");
        assert_eq!(c.state, ContainerState::Running);
        assert_eq!(c.status_text, "Up 3 hours");
        assert_eq!(c.created_ms, 1_704_067_200_500);
        assert_eq!(c.started_ms, Some(1_704_067_260_000));
        assert_eq!(c.finished_ms, None);
        assert_eq!(c.restart_count, 3);
        assert_eq!(c.restart_policy, "unless-stopped");
        assert_eq!(c.compose_project.as_deref(), Some("shop"));
        assert_eq!(c.compose_service.as_deref(), Some("web"));
        assert_eq!(c.managed_stack_id.as_deref(), Some("stk_1"));
        assert_eq!(c.env_keys, vec!["PATH", "DB_PASSWORD"]);
        assert_eq!(c.env["DB_PASSWORD"], super::super::MASK);
        assert_eq!(c.ports.len(), 2);
        assert_eq!(c.ports[0].container_port, 80);
        assert_eq!(c.ports[0].host_port, Some(8080));
        assert_eq!(c.ports[1].container_port, 9000);
        assert_eq!(c.ports[1].host_port, None);
        assert_eq!(c.ports[1].protocol, "udp");
        assert_eq!(c.mounts[0].destination, "/data");
        assert_eq!(vols, vec!["data"]);
        assert_eq!(c.networks, vec!["web_default"]);

        let stacks = group_stacks(&[c], &|p| (p == "shop").then_some(4));
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].name, "shop");
        assert_eq!(stacks[0].applied_generation, Some(4));
        assert_eq!(stacks[0].config_hashes["web"], "h1");
    }

    #[test]
    fn stats_reading_sums_networks_and_blkio() {
        use bollard::models::{
            ContainerBlkioStatEntry, ContainerBlkioStats, ContainerCpuStats, ContainerCpuUsage,
            ContainerMemoryStats, ContainerNetworkStats,
        };
        let r = ContainerStatsResponse {
            cpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(500),
                    ..Default::default()
                }),
                system_cpu_usage: Some(10_000),
                online_cpus: Some(2),
                ..Default::default()
            }),
            memory_stats: Some(ContainerMemoryStats {
                usage: Some(1000),
                limit: Some(4000),
                stats: Some(HashMap::from([("inactive_file".to_string(), 200u64)])),
                ..Default::default()
            }),
            networks: Some(HashMap::from([
                (
                    "eth0".to_string(),
                    ContainerNetworkStats {
                        rx_bytes: Some(10),
                        tx_bytes: Some(1),
                        ..Default::default()
                    },
                ),
                (
                    "eth1".to_string(),
                    ContainerNetworkStats {
                        rx_bytes: Some(5),
                        tx_bytes: Some(2),
                        ..Default::default()
                    },
                ),
            ])),
            blkio_stats: Some(ContainerBlkioStats {
                io_service_bytes_recursive: Some(vec![
                    ContainerBlkioStatEntry {
                        op: Some("read".into()),
                        value: Some(100),
                        ..Default::default()
                    },
                    ContainerBlkioStatEntry {
                        op: Some("Write".into()),
                        value: Some(50),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (cpu, counters, mem, limit) = read_stats(&r, 1);
        assert_eq!(
            cpu,
            Some(CpuSample {
                total_ns: 500,
                system_ns: 10_000,
                online_cpus: 2
            })
        );
        assert_eq!(
            counters,
            Counters {
                ts_ms: 1,
                net_rx: 15,
                net_tx: 3,
                block_read: 100,
                block_write: 50
            }
        );
        assert_eq!(mem, Some(800));
        assert_eq!(limit, Some(4000));
    }

    #[test]
    fn fs_usage_reads_the_root_filesystem() {
        let (total, used) = fs_usage("/").expect("statvfs /");
        assert!(total > 0);
        assert!(used <= total);
        assert!(fs_usage("/definitely/not/here").is_none());
    }
}

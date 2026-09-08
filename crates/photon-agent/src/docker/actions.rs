//! The `lifecycle` capability (R3.1–R3.3): one-shot actions with a TTL. Each `ActionSpec`
//! maps to a handful of Engine API calls; the detail string that comes back is what the
//! operator reads in the audit log and the toast ("recreated web on nginx:1.27 in 4.2s"), so
//! it says what happened rather than echoing the request.
//!
//! `recreate` is the delicate one. It renames the old container aside, creates the new one
//! from the SAME Config + HostConfig + networks + name, starts it, and only then removes the
//! old one — so a create that fails (bad image, port taken) leaves the original in place,
//! renamed back and restarted, instead of a host with no container at all.
use std::collections::HashMap;

use bollard::auth::DockerCredentials;
use bollard::exec::CreateExecOptions;
use bollard::models::{
    ContainerConfig, ContainerCreateBody, EndpointSettings, HostConfig, NetworkingConfig,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, PruneImagesOptionsBuilder,
    PruneVolumesOptionsBuilder, RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder,
    RenameContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use bollard::Docker;
use futures::StreamExt;
use photon_deploy::model::ActionKind;
use photon_deploy::proto::{ActionSpec, RegistryAuth};
use photon_deploy::registry::{auth_for_image, config_key};

/// Seconds a `stop` waits before SIGKILL — Docker's own default.
pub const STOP_TIMEOUT_SECS: i32 = 10;

fn es(e: bollard::errors::Error) -> String {
    e.to_string()
}

/// Execute one action. `Ok(detail)` / `Err(reason)`; the caller stamps status and timing.
pub async fn run(
    docker: &Docker,
    spec: &ActionSpec,
    auths: &[RegistryAuth],
) -> Result<String, String> {
    let cid = || -> Result<&str, String> {
        spec.container_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{} needs a container_id", spec.kind.as_str()))
    };
    match spec.kind {
        ActionKind::Start => {
            docker.start_container(cid()?, None).await.map_err(es)?;
            Ok("started".into())
        }
        ActionKind::Stop => {
            let opts = StopContainerOptionsBuilder::new()
                .t(STOP_TIMEOUT_SECS)
                .build();
            docker
                .stop_container(cid()?, Some(opts))
                .await
                .map_err(es)?;
            Ok("stopped".into())
        }
        ActionKind::Restart => {
            docker.restart_container(cid()?, None).await.map_err(es)?;
            Ok("restarted".into())
        }
        ActionKind::Kill => {
            docker.kill_container(cid()?, None).await.map_err(es)?;
            Ok("killed (SIGKILL)".into())
        }
        ActionKind::Remove => {
            let rv = spec.args.remove_volumes.unwrap_or(false);
            let opts = RemoveContainerOptionsBuilder::new()
                .force(true)
                .v(rv)
                .build();
            docker
                .remove_container(cid()?, Some(opts))
                .await
                .map_err(es)?;
            Ok(if rv {
                "removed with its anonymous volumes".into()
            } else {
                "removed".into()
            })
        }
        ActionKind::Pull => {
            let insp = docker.inspect_container(cid()?, None).await.map_err(es)?;
            let image = insp
                .config
                .and_then(|c| c.image)
                .ok_or_else(|| "container has no image reference".to_string())?;
            let status = pull_image(docker, &image, auths).await?;
            Ok(format!("pulled {image}: {status}"))
        }
        ActionKind::Recreate => {
            recreate(docker, cid()?, spec.args.pull.unwrap_or(false), auths).await
        }
        ActionKind::ImagePull => {
            let image = spec
                .image
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or("image_pull needs an image")?;
            let status = pull_image(docker, image, auths).await?;
            Ok(format!("pulled {}: {status}", normalize_image_ref(image)))
        }
        ActionKind::ImageRemove => {
            let image = spec
                .image
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or("image_remove needs an image")?;
            let opts = RemoveImageOptionsBuilder::new().force(false).build();
            let items = docker
                .remove_image(image, Some(opts), None)
                .await
                .map_err(es)?;
            let untagged = items.iter().filter(|i| i.untagged.is_some()).count();
            let deleted = items.iter().filter(|i| i.deleted.is_some()).count();
            Ok(format!("untagged {untagged}, deleted {deleted} layer(s)"))
        }
        ActionKind::VolumeRemove => {
            let name = spec
                .volume
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or("volume_remove needs a volume")?;
            // No `force`: a volume a container still mounts must fail here, not vanish from
            // under it; the server already refused that, this is the last line.
            docker
                .remove_volume(name, None::<bollard::query_parameters::RemoveVolumeOptions>)
                .await
                .map_err(es)?;
            Ok(format!("removed volume {name}"))
        }
        ActionKind::Prune => prune(docker, spec).await,
    }
}

/// `nginx` → `nginx:latest`: the Engine API pulls EVERY tag of an image whose reference has
/// no tag or digest, which is never what a "pull" button means.
pub fn normalize_image_ref(image: &str) -> String {
    if image.contains('@') {
        return image.to_string();
    }
    let last = image.rsplit('/').next().unwrap_or(image);
    if last.contains(':') {
        image.to_string()
    } else {
        format!("{image}:latest")
    }
}

/// The `X-Registry-Auth` credential for one pull, when the server handed us one for the
/// image's registry. The daemon reads no `config.json` for Engine API pulls: this header is
/// the only way a private image gets pulled through bollard.
fn credentials_for(image: &str, auths: &[RegistryAuth]) -> Option<DockerCredentials> {
    auth_for_image(image, auths).map(|a| DockerCredentials {
        username: Some(a.username.clone()),
        password: Some(a.secret.clone()),
        serveraddress: Some(config_key(&a.server)),
        ..Default::default()
    })
}

/// Pull an image, draining the progress stream; the last status line is the summary
/// ("Status: Image is up to date for nginx:latest" / "Downloaded newer image …").
pub async fn pull_image(
    docker: &Docker,
    image: &str,
    auths: &[RegistryAuth],
) -> Result<String, String> {
    let reference = normalize_image_ref(image);
    let opts = CreateImageOptionsBuilder::new()
        .from_image(&reference)
        .build();
    let creds = credentials_for(image, auths);
    let mut stream = docker.create_image(Some(opts), None, creds);
    let mut last = String::new();
    while let Some(item) = stream.next().await {
        let info = item.map_err(es)?;
        if let Some(d) = info.error_detail.and_then(|d| d.message) {
            return Err(d);
        }
        if let Some(s) = info.status {
            last = s;
        }
    }
    Ok(last)
}

async fn recreate(
    docker: &Docker,
    id: &str,
    pull: bool,
    auths: &[RegistryAuth],
) -> Result<String, String> {
    let started = std::time::Instant::now();
    let insp = docker.inspect_container(id, None).await.map_err(es)?;
    let old_id = insp.id.clone().unwrap_or_else(|| id.to_string());
    let name = insp
        .name
        .clone()
        .map(|n| n.trim_start_matches('/').to_string())
        .ok_or("container has no name")?;
    let config = insp.config.clone().unwrap_or_default();
    let image = config
        .image
        .clone()
        .ok_or("container has no image reference")?;
    let was_running = insp.state.as_ref().and_then(|s| s.running).unwrap_or(false);
    if pull {
        pull_image(docker, &image, auths).await?;
    }
    let body = create_body(
        config,
        insp.host_config.clone().unwrap_or_default(),
        insp.network_settings
            .and_then(|n| n.networks)
            .unwrap_or_default(),
        &old_id,
    );

    if was_running {
        let opts = StopContainerOptionsBuilder::new()
            .t(STOP_TIMEOUT_SECS)
            .build();
        docker
            .stop_container(&old_id, Some(opts))
            .await
            .map_err(es)?;
    }
    let aside = format!("{name}.photon-old");
    docker
        .rename_container(
            &old_id,
            RenameContainerOptionsBuilder::new().name(&aside).build(),
        )
        .await
        .map_err(es)?;

    let created = docker
        .create_container(
            Some(CreateContainerOptionsBuilder::new().name(&name).build()),
            body,
        )
        .await;
    let new_id = match created {
        Ok(r) => r.id,
        Err(e) => {
            // Put the original back exactly as it was.
            let _ = docker
                .rename_container(
                    &old_id,
                    RenameContainerOptionsBuilder::new().name(&name).build(),
                )
                .await;
            if was_running {
                let _ = docker.start_container(&old_id, None).await;
            }
            return Err(format!("create failed, original kept: {e}"));
        }
    };
    if let Err(e) = docker.start_container(&new_id, None).await {
        let _ = docker
            .remove_container(
                &new_id,
                Some(RemoveContainerOptionsBuilder::new().force(true).build()),
            )
            .await;
        let _ = docker
            .rename_container(
                &old_id,
                RenameContainerOptionsBuilder::new().name(&name).build(),
            )
            .await;
        if was_running {
            let _ = docker.start_container(&old_id, None).await;
        }
        return Err(format!("start failed, original kept: {e}"));
    }
    docker
        .remove_container(
            &old_id,
            Some(RemoveContainerOptionsBuilder::new().force(true).build()),
        )
        .await
        .map_err(|e| format!("new container running, old one could not be removed: {e}"))?;
    Ok(format!(
        "recreated {name} on {image} as {} in {:.1}s",
        new_id.chars().take(12).collect::<String>(),
        started.elapsed().as_secs_f64()
    ))
}

/// The create body for an identical container: the inspected Config + HostConfig, and the
/// networks re-expressed as endpoints. Two things must NOT carry over: a hostname that was the
/// OLD container's id (Docker's default, which would pin the new container to a stale name),
/// and the old id's short form among the network aliases (Docker adds it automatically).
pub fn create_body(
    mut config: ContainerConfig,
    host_config: HostConfig,
    networks: HashMap<String, EndpointSettings>,
    old_id: &str,
) -> ContainerCreateBody {
    let short: String = old_id.chars().take(12).collect();
    if config.hostname.as_deref() == Some(short.as_str()) {
        config.hostname = None;
    }
    let mut endpoints: HashMap<String, EndpointSettings> = HashMap::new();
    let mode = host_config.network_mode.clone().unwrap_or_default();
    if !(mode.starts_with("container:") || mode == "host" || mode == "none") {
        for (name, ep) in networks {
            let aliases = ep
                .aliases
                .map(|a| a.into_iter().filter(|x| *x != short).collect::<Vec<_>>())
                .filter(|a| !a.is_empty());
            let ipam = ep
                .ipam_config
                .filter(|c| c.ipv4_address.is_some() || c.ipv6_address.is_some());
            endpoints.insert(
                name,
                EndpointSettings {
                    ipam_config: ipam,
                    aliases,
                    links: ep.links,
                    driver_opts: ep.driver_opts,
                    ..Default::default()
                },
            );
        }
    }
    ContainerCreateBody {
        hostname: config.hostname,
        domainname: config.domainname,
        user: config.user,
        attach_stdin: config.attach_stdin,
        attach_stdout: config.attach_stdout,
        attach_stderr: config.attach_stderr,
        exposed_ports: config.exposed_ports,
        tty: config.tty,
        open_stdin: config.open_stdin,
        stdin_once: config.stdin_once,
        env: config.env,
        cmd: config.cmd,
        healthcheck: config.healthcheck,
        args_escaped: config.args_escaped,
        image: config.image,
        volumes: config.volumes,
        working_dir: config.working_dir,
        entrypoint: config.entrypoint,
        network_disabled: config.network_disabled,
        on_build: config.on_build,
        labels: config.labels,
        stop_signal: config.stop_signal,
        stop_timeout: config.stop_timeout,
        shell: config.shell,
        host_config: Some(host_config),
        networking_config: if endpoints.is_empty() {
            None
        } else {
            Some(NetworkingConfig {
                endpoints_config: Some(endpoints),
            })
        },
    }
}

async fn prune(docker: &Docker, spec: &ActionSpec) -> Result<String, String> {
    let a = &spec.args;
    let (images, containers, volumes, build_cache) = (
        a.images.unwrap_or(false),
        a.containers.unwrap_or(false),
        a.volumes.unwrap_or(false),
        a.build_cache.unwrap_or(false),
    );
    if !(images || containers || volumes || build_cache) {
        return Err("prune selected nothing (images/containers/volumes/build_cache)".into());
    }
    let mut parts = Vec::new();
    let mb = |b: Option<i64>| format!("{:.1} MB", b.unwrap_or(0).max(0) as f64 / 1e6);
    if containers {
        let r = docker.prune_containers(None).await.map_err(es)?;
        parts.push(format!(
            "containers: {} ({})",
            r.containers_deleted.map(|v| v.len()).unwrap_or(0),
            mb(r.space_reclaimed)
        ));
    }
    if images {
        // All UNUSED images, matching what `system df` reported as reclaimable — not just the
        // dangling ones.
        let filters: HashMap<&str, Vec<&str>> = HashMap::from([("dangling", vec!["false"])]);
        let r = docker
            .prune_images(Some(
                PruneImagesOptionsBuilder::new().filters(&filters).build(),
            ))
            .await
            .map_err(es)?;
        parts.push(format!(
            "images: {} ({})",
            r.images_deleted.map(|v| v.len()).unwrap_or(0),
            mb(r.space_reclaimed)
        ));
    }
    if volumes {
        // Since Docker 23 `volume prune` skips named volumes unless `all=true`; the preview
        // counted every unused volume, so remove every unused volume.
        let filters: HashMap<&str, Vec<&str>> = HashMap::from([("all", vec!["true"])]);
        let r = docker
            .prune_volumes(Some(
                PruneVolumesOptionsBuilder::new().filters(&filters).build(),
            ))
            .await
            .map_err(es)?;
        parts.push(format!(
            "volumes: {} ({})",
            r.volumes_deleted.map(|v| v.len()).unwrap_or(0),
            mb(r.space_reclaimed)
        ));
    }
    if build_cache {
        let r = docker
            .prune_build(None::<bollard::query_parameters::PruneBuildOptions>)
            .await
            .map_err(es)?;
        parts.push(format!(
            "build cache: {} ({})",
            r.caches_deleted.map(|v| v.len()).unwrap_or(0),
            mb(r.space_reclaimed)
        ));
    }
    Ok(parts.join(", "))
}

/// Used by the exec stream; here so every place that builds an exec agrees on the shape.
/// The shell behind the UI's Shell tab. Deliberately NOT a TTY: the UI is a line-at-a-time
/// console that prints the command itself, so it needs no echo and no prompt — and a pty
/// makes busybox `ash` start its line editor, which queries the terminal for the cursor
/// position (`ESC[6n`) and stalls waiting for an answer nothing will send. Without a pty
/// the shell reads stdin line by line and its stdout/stderr flow back as plain text.
pub fn exec_options(shell: Option<&str>) -> CreateExecOptions<String> {
    CreateExecOptions {
        attach_stdin: Some(true),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        tty: Some(false),
        cmd: Some(vec![shell
            .filter(|s| !s.is_empty())
            .unwrap_or("sh")
            .to_string()]),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::EndpointIpamConfig;

    #[test]
    fn image_refs_get_a_tag_unless_they_have_one() {
        assert_eq!(normalize_image_ref("nginx"), "nginx:latest");
        assert_eq!(normalize_image_ref("nginx:1.27"), "nginx:1.27");
        assert_eq!(
            normalize_image_ref("registry.local:5000/team/app"),
            "registry.local:5000/team/app:latest"
        );
        assert_eq!(
            normalize_image_ref("registry.local:5000/team/app:v2"),
            "registry.local:5000/team/app:v2"
        );
        assert_eq!(normalize_image_ref("nginx@sha256:abc"), "nginx@sha256:abc");
    }

    #[test]
    fn create_body_drops_stale_hostname_and_alias_but_keeps_static_ip() {
        let old = "0123456789abcdef0123";
        let config = ContainerConfig {
            hostname: Some("0123456789ab".into()),
            image: Some("nginx".into()),
            env: Some(vec!["A=1".into()]),
            ..Default::default()
        };
        let nets = HashMap::from([(
            "shop_default".to_string(),
            EndpointSettings {
                aliases: Some(vec!["web".into(), "0123456789ab".into()]),
                ipam_config: Some(EndpointIpamConfig {
                    ipv4_address: Some("10.0.0.5".into()),
                    ..Default::default()
                }),
                endpoint_id: Some("stale".into()),
                ..Default::default()
            },
        )]);
        let body = create_body(config, HostConfig::default(), nets, old);
        assert_eq!(body.hostname, None);
        assert_eq!(body.image.as_deref(), Some("nginx"));
        let eps = body.networking_config.unwrap().endpoints_config.unwrap();
        let ep = &eps["shop_default"];
        assert_eq!(ep.aliases.as_ref().unwrap(), &vec!["web".to_string()]);
        assert_eq!(
            ep.ipam_config.as_ref().unwrap().ipv4_address.as_deref(),
            Some("10.0.0.5")
        );
        assert_eq!(ep.endpoint_id, None);

        // A user-chosen hostname survives; host networking carries no endpoints.
        let config = ContainerConfig {
            hostname: Some("db-1".into()),
            ..Default::default()
        };
        let hc = HostConfig {
            network_mode: Some("host".into()),
            ..Default::default()
        };
        let body = create_body(
            config,
            hc,
            HashMap::from([("host".to_string(), EndpointSettings::default())]),
            old,
        );
        assert_eq!(body.hostname.as_deref(), Some("db-1"));
        assert!(body.networking_config.is_none());
    }

    #[test]
    fn exec_defaults_to_sh_without_a_tty() {
        let o = exec_options(None);
        assert_eq!(o.cmd, Some(vec!["sh".to_string()]));
        assert_eq!(
            o.tty,
            Some(false),
            "a pty makes busybox ash stall on ESC[6n"
        );
        assert_eq!(
            exec_options(Some("/bin/bash")).cmd,
            Some(vec!["/bin/bash".to_string()])
        );
    }
}

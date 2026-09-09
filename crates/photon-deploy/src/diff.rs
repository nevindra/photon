//! Drift (R6.7): desired versus observed for one stack, as one pure function. Every sync carries
//! observed state; the server diffs it against what it rendered, and a mismatch marks the stack
//! `Drifted`. Reconciling is an operator's deliberate act — nothing here reverts anything.

use crate::compose::{compose_services, service_images, substitute};
use crate::model::{
    Container, ContainerState, DriftItem, DriftKind, ObservedStack, Stack, StackServiceContainer,
    StackServiceState,
};
use std::collections::BTreeMap;

fn compose_text(stack: &Stack) -> &str {
    stack
        .files
        .iter()
        .find(|f| crate::compose::is_compose_file(&f.path))
        .map(|f| f.content.as_str())
        .unwrap_or("")
}

/// Desired `service → image` with the stack's non-secret variables substituted.
pub fn desired_images(stack: &Stack) -> BTreeMap<String, String> {
    let text = substitute(compose_text(stack), &stack.variables);
    service_images(&text)
}

/// Desired service names, in compose order.
pub fn desired_services(stack: &Stack) -> Vec<String> {
    compose_services(compose_text(stack))
}

/// The containers on a host that belong to this stack's compose project.
pub fn project_containers<'a>(stack: &Stack, containers: &'a [Container]) -> Vec<&'a Container> {
    containers
        .iter()
        .filter(|c| {
            c.compose_project.as_deref() == Some(stack.name.as_str())
                || c.managed_stack_id.as_deref() == Some(stack.id.as_str())
        })
        .collect()
}

/// What the UI shows per compose service: the image it should run and the containers that exist.
pub fn observed_services(stack: &Stack, containers: &[Container]) -> Vec<StackServiceState> {
    let images = desired_images(stack);
    let mine = project_containers(stack, containers);
    desired_services(stack)
        .into_iter()
        .map(|name| {
            let cs: Vec<StackServiceContainer> = mine
                .iter()
                .filter(|c| c.compose_service.as_deref() == Some(name.as_str()))
                .map(|c| StackServiceContainer {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    state: c.state,
                    health: c.health,
                })
                .collect();
            let image = images
                .get(&name)
                .cloned()
                .or_else(|| {
                    mine.iter()
                        .find(|c| c.compose_service.as_deref() == Some(name.as_str()))
                        .map(|c| c.image.clone())
                })
                .unwrap_or_else(|| "?".to_string());
            StackServiceState {
                name,
                image,
                containers: cs,
            }
        })
        .collect()
}

/// Two image references name the same thing when they differ only by an implicit `:latest`.
fn same_image(desired: &str, observed: &str) -> bool {
    if desired == observed {
        return true;
    }
    let norm = |s: &str| -> String {
        let s = s.trim();
        // `name` and `name:latest` are the same reference; a digest form is compared verbatim.
        if s.contains('@') {
            return s.to_string();
        }
        let last_slash = s.rfind('/').map(|i| i + 1).unwrap_or(0);
        if s[last_slash..].contains(':') {
            s.to_string()
        } else {
            format!("{s}:latest")
        }
    };
    let (d, o) = (norm(desired), norm(observed));
    d == o || o.ends_with(&format!("/{d}")) || d.ends_with(&format!("/{o}"))
}

/// Compute the drift between a stack's desired state and what the host reports. `previous`
/// carries the last computed drift so an item that persists keeps its `since_ms`.
pub fn compute(
    stack: &Stack,
    containers: &[Container],
    observed_stack: Option<&ObservedStack>,
    previous: &[DriftItem],
    now_ms: i64,
) -> Vec<DriftItem> {
    let since = |kind: DriftKind, service: Option<&str>, field: Option<&str>| -> i64 {
        previous
            .iter()
            .find(|p| {
                p.kind == kind && p.service.as_deref() == service && p.field.as_deref() == field
            })
            .map(|p| p.since_ms)
            .unwrap_or(now_ms)
    };
    let mut out = Vec::new();
    let images = desired_images(stack);
    let services = desired_services(stack);
    let mine = project_containers(stack, containers);

    for name in &services {
        let cs: Vec<&&Container> = mine
            .iter()
            .filter(|c| c.compose_service.as_deref() == Some(name.as_str()))
            .collect();
        if cs.is_empty() {
            out.push(DriftItem {
                kind: DriftKind::ContainerMissing,
                service: Some(name.clone()),
                field: None,
                desired: Some("running".into()),
                observed: Some("no container".into()),
                since_ms: since(DriftKind::ContainerMissing, Some(name), None),
            });
            continue;
        }
        for c in cs {
            if c.state != ContainerState::Running {
                out.push(DriftItem {
                    kind: DriftKind::StateChanged,
                    service: Some(name.clone()),
                    field: Some("state".into()),
                    desired: Some("running".into()),
                    observed: Some(c.state.as_str().into()),
                    since_ms: since(DriftKind::StateChanged, Some(name), Some("state")),
                });
            }
            if let Some(want) = images.get(name) {
                if !want.contains("${") && !same_image(want, &c.image) {
                    out.push(DriftItem {
                        kind: DriftKind::ImageChanged,
                        service: Some(name.clone()),
                        field: Some("image".into()),
                        desired: Some(want.clone()),
                        observed: Some(c.image.clone()),
                        since_ms: since(DriftKind::ImageChanged, Some(name), Some("image")),
                    });
                }
            }
        }
        // Config drift: the compose config hash Docker stamped on the container differs from the
        // one recorded right after our last apply (somebody re-upped with an edited file).
        if let (Some(base), Some(obs)) = (
            stack.baseline_config_hashes.get(name),
            observed_stack.and_then(|o| o.config_hashes.get(name)),
        ) {
            if base != obs {
                out.push(DriftItem {
                    kind: DriftKind::ConfigChanged,
                    service: Some(name.clone()),
                    field: Some("config".into()),
                    desired: Some(short(base)),
                    observed: Some(short(obs)),
                    since_ms: since(DriftKind::ConfigChanged, Some(name), Some("config")),
                });
            }
        }
    }
    for c in &mine {
        let svc = c.compose_service.clone();
        if svc.as_ref().map(|s| !services.contains(s)).unwrap_or(true) {
            out.push(DriftItem {
                kind: DriftKind::ExtraContainer,
                service: svc.clone(),
                field: None,
                desired: Some("absent".into()),
                observed: Some(c.name.clone()),
                since_ms: since(DriftKind::ExtraContainer, svc.as_deref(), None),
            });
        }
    }
    out
}

fn short(h: &str) -> String {
    h.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn stack(compose: &str) -> Stack {
        Stack {
            id: "st_1".into(),
            name: "vas".into(),
            host: "edge".into(),
            site: None,
            source: StackSource::Compose,
            template_id: None,
            template_name: None,
            template_version: None,
            template_version_id: None,
            status: StackStatus::Running,
            desired_generation: 1,
            applied_generation: Some(1),
            files: vec![StackFile {
                path: "compose.yaml".into(),
                content: compose.into(),
            }],
            variables: BTreeMap::from([("VER".to_string(), "2.4.1".to_string())]),
            secrets: vec![],
            deploy_options: DeployOptions::default(),
            services: vec![],
            drift: vec![],
            apm_services: vec![],
            acknowledged_warnings: vec![],
            last_error: None,
            created_by: "x".into(),
            created_ms: 0,
            updated_ms: 0,
            last_applied_ms: None,
            pending_op: None,
            baseline_config_hashes: BTreeMap::from([("api".to_string(), "abc".to_string())]),
            previous: None,
            drift_since_ms: None,
        }
    }

    fn container(
        name: &str,
        service: Option<&str>,
        image: &str,
        state: ContainerState,
    ) -> Container {
        Container {
            id: name.into(),
            name: name.into(),
            image: image.into(),
            image_id: "".into(),
            command: "".into(),
            state,
            health: ContainerHealth::None,
            status_text: "".into(),
            created_ms: 0,
            started_ms: None,
            finished_ms: None,
            exit_code: None,
            restart_count: 0,
            restart_policy: "".into(),
            ports: vec![],
            mounts: vec![],
            networks: vec![],
            labels: Default::default(),
            env_keys: vec![],
            env: Default::default(),
            compose_project: Some("vas".into()),
            compose_service: service.map(|s| s.into()),
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

    const COMPOSE: &str =
        "services:\n  api:\n    image: nodeflux/api:${VER}\n  db:\n    image: postgres:16\n";

    #[test]
    fn matching_state_has_no_drift() {
        let s = stack(COMPOSE);
        let cs = vec![
            container(
                "vas-api-1",
                Some("api"),
                "nodeflux/api:2.4.1",
                ContainerState::Running,
            ),
            container(
                "vas-db-1",
                Some("db"),
                "postgres:16",
                ContainerState::Running,
            ),
        ];
        let obs = ObservedStack {
            name: "vas".into(),
            applied_generation: Some(1),
            config_hashes: BTreeMap::from([("api".into(), "abc".into())]),
            working_dir: None,
            compose_file: None,
        };
        assert!(compute(&s, &cs, Some(&obs), &[], 5).is_empty());
        let svcs = observed_services(&s, &cs);
        assert_eq!(svcs[0].image, "nodeflux/api:2.4.1");
        assert_eq!(svcs[1].containers.len(), 1);
    }

    #[test]
    fn missing_changed_extra_and_config() {
        let s = stack(COMPOSE);
        let cs = vec![
            container(
                "vas-api-1",
                Some("api"),
                "nodeflux/api:2.3.0",
                ContainerState::Exited,
            ),
            container(
                "vas-cache-1",
                Some("cache"),
                "redis:7",
                ContainerState::Running,
            ),
        ];
        let obs = ObservedStack {
            name: "vas".into(),
            applied_generation: Some(1),
            config_hashes: BTreeMap::from([("api".into(), "zzz".into())]),
            working_dir: None,
            compose_file: None,
        };
        let prev = vec![DriftItem {
            kind: DriftKind::ContainerMissing,
            service: Some("db".into()),
            field: None,
            desired: None,
            observed: None,
            since_ms: 1,
        }];
        let d = compute(&s, &cs, Some(&obs), &prev, 9);
        let kinds: Vec<DriftKind> = d.iter().map(|x| x.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DriftKind::StateChanged,
                DriftKind::ImageChanged,
                DriftKind::ConfigChanged,
                DriftKind::ContainerMissing,
                DriftKind::ExtraContainer
            ]
        );
        assert_eq!(
            d.iter()
                .find(|x| x.kind == DriftKind::ContainerMissing)
                .unwrap()
                .since_ms,
            1,
            "a persisting item keeps its since"
        );
        assert_eq!(d[0].since_ms, 9);
    }

    #[test]
    fn implicit_latest_and_registry_prefix_are_the_same_image() {
        assert!(same_image("redis", "redis:latest"));
        assert!(same_image("nginx:1.25", "docker.io/library/nginx:1.25"));
        assert!(!same_image("redis:7", "redis:6"));
    }
}

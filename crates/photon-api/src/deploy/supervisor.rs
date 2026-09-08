//! The periodic tick photon-server spawns (mirrors the alert scheduler): rollout advancement,
//! action expiry, stream/job expiry, and stacks stuck in `applying` on a host that went away.
//! It writes desired state and reads observed state; it never waits on a host.

use super::DeployApi;
use crate::auth::CurrentUser;
use photon_deploy::model::*;
use photon_deploy::store::SecretBlob;
use std::collections::BTreeMap;
use std::time::Duration;

pub const TICK: Duration = Duration::from_secs(3);

/// Run forever.
pub async fn run(api: DeployApi) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Err(e) = tick(&api).await {
            eprintln!("photon-api: deploy supervisor tick failed: {e}");
        }
    }
}

/// One pass. Public so tests can drive it without the clock.
pub async fn tick(api: &DeployApi) -> Result<(), photon_core::PhotonError> {
    let now = super::now_ms();
    expire_actions(api, now).await?;
    for s in api.hub.expire_streams(now) {
        if s.kind == photon_deploy::proto::StreamKind::Exec {
            if let Some((who, role)) = s.opened_by {
                api.audit_system(
                    &who,
                    role,
                    "container.exec",
                    AuditTarget {
                        host: Some(s.host.clone()),
                        container: Some(s.container_name.clone()),
                        ..Default::default()
                    },
                    AuditResultKind::Ok,
                    Some(format!(
                        "shell closed (idle) after {}s, {} command{}",
                        (now - s.opened_ms) / 1000,
                        s.commands,
                        if s.commands == 1 { "" } else { "s" }
                    )),
                )
                .await;
            }
        }
        api.hub.wake(&s.host);
    }
    api.hub.expire_jobs(now);
    advance_rollouts(api, now).await?;
    Ok(())
}

async fn expire_actions(api: &DeployApi, now: i64) -> Result<(), photon_core::PhotonError> {
    for h in api.store.list_hosts().await? {
        for mut a in api.store.open_actions(&h.host).await? {
            if a.expires_ms < now {
                a.status = ActionStatus::Expired;
                a.finished_ms = Some(now);
                a.result = Some("expired before the host picked it up".into());
                api.store.put_action(&a).await?;
                api.audit_system(
                    &a.actor,
                    Role::Operator,
                    &a.kind.audit_action(),
                    AuditTarget {
                        host: Some(h.host.clone()),
                        container: a.container_name.clone(),
                        image: a.image.clone(),
                        volume: a.volume.clone(),
                        ..Default::default()
                    },
                    AuditResultKind::Failed,
                    Some("expired: the host did not sync within 15 minutes".into()),
                )
                .await;
            }
        }
    }
    Ok(())
}

fn system_user(name: &str) -> CurrentUser {
    CurrentUser {
        username: name.to_string(),
        role: Role::Operator,
    }
}

/// Move every running rollout along: apply queued targets of the live stage, read the stacks'
/// fates back into target statuses, settle the stage.
async fn advance_rollouts(api: &DeployApi, now: i64) -> Result<(), photon_core::PhotonError> {
    for mut r in api.store.list_rollouts().await? {
        if r.state != RolloutState::Running {
            continue;
        }
        let mut changed = false;
        let me = system_user(&r.created_by);
        let secrets: BTreeMap<String, String> = match api.store.get_secrets(&r.id).await? {
            Some(blob) => blob
                .values
                .iter()
                .filter_map(|(k, v)| api.cipher.decrypt(v).ok().map(|p| (k.clone(), p)))
                .collect(),
            None => BTreeMap::new(),
        };
        // 1. Queued targets of the live stage → apply.
        let stage = r.stage;
        let queued: Vec<usize> = r
            .targets
            .iter()
            .enumerate()
            .filter(|(_, t)| t.stage == stage && t.status == TargetStatus::Queued)
            .map(|(i, _)| i)
            .collect();
        for i in queued {
            let host = r.targets[i].host.clone();
            let input = TemplateDeployInput {
                template_version_id: r.template_version_id.clone(),
                host: host.clone(),
                name: r.stack_name.clone(),
                variables: r.variables.clone(),
                secrets: secrets.clone(),
                deploy_options: r.deploy_options.clone(),
                acknowledged_warnings: r.acknowledged_warnings.clone(),
                stack_id: None,
            };
            let outcome = match api.prepare(&input).await {
                Ok(p) => {
                    api.deploy_template(&me, &input, p, r.strategy.prepull, Some(&r.name))
                        .await
                }
                Err(e) => Err(e),
            };
            let t = &mut r.targets[i];
            t.attempts += 1;
            t.updated_ms = now;
            match outcome {
                Ok(stack) => {
                    t.stack_id = Some(stack.id.clone());
                    t.applied_generation = Some(stack.desired_generation);
                    t.status = match stack.status {
                        StackStatus::Applying => {
                            if r.strategy.prepull {
                                TargetStatus::Prepulling
                            } else {
                                TargetStatus::Applying
                            }
                        }
                        _ => TargetStatus::Pending,
                    };
                    t.last_error = None;
                }
                Err((_, axum::Json(body))) => {
                    t.status = TargetStatus::Failed;
                    t.last_error = Some(
                        body.get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("apply refused")
                            .to_string(),
                    );
                }
            }
            changed = true;
        }
        // 2. Targets in flight → read their stack's fate.
        let grace = (r.strategy.health_grace_secs as i64) * 1000;
        for t in r.targets.iter_mut().filter(|t| {
            t.stage == stage
                && matches!(
                    t.status,
                    TargetStatus::Pending | TargetStatus::Prepulling | TargetStatus::Applying
                )
        }) {
            let Some(sid) = &t.stack_id else { continue };
            let Some(s) = api.store.get_stack(sid).await? else {
                continue;
            };
            let applied_this =
                s.applied_generation.is_some() && s.applied_generation >= t.applied_generation;
            let new_status = match s.status {
                StackStatus::Failed if applied_this || s.last_error.is_some() => {
                    Some(TargetStatus::Failed)
                }
                StackStatus::Applying => Some(
                    if r.strategy.prepull && t.status == TargetStatus::Prepulling {
                        TargetStatus::Prepulling
                    } else {
                        TargetStatus::Applying
                    },
                ),
                StackStatus::Pending => Some(TargetStatus::Pending),
                StackStatus::Drifted if applied_this => Some(TargetStatus::Drifted),
                StackStatus::Running if applied_this => {
                    // Healthy once the grace period passed with the stack still running and
                    // every service's containers up.
                    let all_up = api
                        .observed(&s.host)
                        .await
                        .ok()
                        .flatten()
                        .map(|o| {
                            photon_deploy::diff::observed_services(&s, &o.observed.containers)
                                .iter()
                                .all(|svc| {
                                    !svc.containers.is_empty()
                                        && svc.containers.iter().all(|c| {
                                            c.state == ContainerState::Running
                                                && c.health != ContainerHealth::Unhealthy
                                        })
                                })
                        })
                        .unwrap_or(false);
                    let since = t
                        .healthy_since_ms
                        .get_or_insert(s.last_applied_ms.unwrap_or(now));
                    if !all_up {
                        t.healthy_since_ms = None;
                        Some(TargetStatus::Applying)
                    } else if now - *since >= grace {
                        Some(TargetStatus::Healthy)
                    } else {
                        Some(TargetStatus::Applying)
                    }
                }
                _ => None,
            };
            if let Some(ns) = new_status {
                if ns != t.status {
                    t.status = ns;
                    t.updated_ms = now;
                    t.last_error = if ns == TargetStatus::Failed {
                        s.last_error.clone()
                    } else {
                        None
                    };
                    changed = true;
                }
            }
        }
        // 3. Settle the stage.
        let in_stage: Vec<&RolloutTarget> = r.targets.iter().filter(|t| t.stage == stage).collect();
        let failed = in_stage
            .iter()
            .filter(|t| t.status == TargetStatus::Failed)
            .count() as u32;
        let unsettled = in_stage.iter().filter(|t| !t.status.is_settled()).count();
        if failed >= r.strategy.failure_threshold.max(1) {
            r.state = RolloutState::PausedThreshold;
            changed = true;
            api.audit_system(
                &r.created_by,
                Role::Operator,
                "rollout.pause",
                AuditTarget {
                    rollout: Some(r.name.clone()),
                    ..Default::default()
                },
                AuditResultKind::Failed,
                Some(format!(
                    "stage {stage}: {failed} failure{} crossed the threshold of {}",
                    if failed == 1 { "" } else { "s" },
                    r.strategy.failure_threshold
                )),
            )
            .await;
        } else if unsettled == 0 {
            if stage >= r.stages {
                r.state = RolloutState::Completed;
                r.finished_ms = Some(now);
                let healthy = r
                    .targets
                    .iter()
                    .filter(|t| t.status == TargetStatus::Healthy)
                    .count();
                api.audit_system(
                    &r.created_by,
                    Role::Operator,
                    "rollout.complete",
                    AuditTarget {
                        rollout: Some(r.name.clone()),
                        version: Some(r.template_version.clone()),
                        ..Default::default()
                    },
                    AuditResultKind::Ok,
                    Some(format!(
                        "{healthy} healthy of {} targets",
                        r.targets.iter().filter(|t| t.stage > 0).count()
                    )),
                )
                .await;
            } else if r.strategy.auto_promote {
                r.stage += 1;
                api.audit_system(
                    &r.created_by,
                    Role::Operator,
                    "rollout.promote",
                    AuditTarget {
                        rollout: Some(r.name.clone()),
                        ..Default::default()
                    },
                    AuditResultKind::Ok,
                    Some(format!("auto-promoted to stage {}", r.stage)),
                )
                .await;
            } else {
                r.state = RolloutState::PausedGate;
                api.audit_system(
                    &r.created_by,
                    Role::Operator,
                    "rollout.pause",
                    AuditTarget {
                        rollout: Some(r.name.clone()),
                        ..Default::default()
                    },
                    AuditResultKind::Ok,
                    Some(format!("stage {stage} settled; waiting at the gate")),
                )
                .await;
            }
            changed = true;
        }
        if changed {
            r.updated_ms = now;
            api.store.put_rollout(&r).await?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn _keep(_: SecretBlob) {}

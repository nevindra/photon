//! Rollouts (R6): one template version to many hosts, staged. The rollout row is the plan and
//! the per-target ledger; the supervisor (`supervisor.rs`) advances it by writing desired state
//! for each target's stack and reading what comes back. It never waits on a host: an offline
//! site is a `pending` row, not a stuck job.

use super::{api, bad, not_found, require, store_err, ApiErr, DeployApi};
use crate::auth::CurrentUser;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use photon_deploy::ids::new_id;
use photon_deploy::model::*;
use photon_deploy::store::SecretBlob;
use photon_deploy::{plan, preflight, render};
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl DeployApi {
    pub async fn rollout(&self, id: &str) -> Result<Rollout, ApiErr> {
        self.store
            .get_rollout(id)
            .await
            .map_err(store_err)?
            .ok_or_else(|| not_found("no such rollout"))
    }

    /// A rollout as the UI reads it: every target's agent status refreshed.
    pub async fn rollout_view(&self, r: &Rollout) -> Result<Rollout, ApiErr> {
        let hosts = self.host_views().await?;
        let mut out = r.clone();
        for t in out.targets.iter_mut() {
            if let Some(h) = hosts.iter().find(|h| h.host == t.host) {
                t.agent_status = h.status;
                t.site = h.site.clone();
            }
        }
        Ok(out)
    }

    /// Plan a rollout: resolve targets, preflight the eligible ones, stage the survivors.
    pub async fn create_rollout(
        &self,
        me: &CurrentUser,
        input: &RolloutInput,
    ) -> Result<Rollout, ApiErr> {
        let name = input.name.trim().to_string();
        if name.is_empty() {
            return Err(bad("a rollout name is required"));
        }
        let stack_name = input.stack_name.trim().to_string();
        if stack_name.is_empty() {
            return Err(bad("a stack name is required"));
        }
        let version = self.version(&input.template_version_id).await?;
        let template = self.template(&version.template_id).await?;
        let hosts = self.host_views().await?;
        let resolved = plan::resolve_targets(&input.selector, &hosts);
        if resolved.is_empty() {
            return Err(bad("the selector matches no managed host"));
        }
        // Preflight each eligible host; a host that cannot take the version is skipped at
        // planning and never a canary.
        let mut blocked: BTreeMap<String, String> = BTreeMap::new();
        let mut previous: BTreeMap<String, Option<String>> = BTreeMap::new();
        for t in resolved.iter().filter(|t| t.eligible) {
            let di = TemplateDeployInput {
                template_version_id: input.template_version_id.clone(),
                host: t.host.clone(),
                name: stack_name.clone(),
                variables: input.variables.clone(),
                secrets: input.secrets.clone(),
                deploy_options: input.deploy_options.clone(),
                acknowledged_warnings: input.acknowledged_warnings.clone(),
                stack_id: None,
            };
            match self.prepare(&di).await {
                Ok(p) => {
                    previous.insert(
                        t.host.clone(),
                        p.existing.as_ref().and_then(|s| s.template_version.clone()),
                    );
                    if !preflight::passed(&p.checks) {
                        blocked.insert(
                            t.host.clone(),
                            p.checks
                                .iter()
                                .filter(|c| c.status == CheckStatus::Fail)
                                .map(|c| c.detail.clone())
                                .collect::<Vec<_>>()
                                .join("; "),
                        );
                    } else if let Some(msg) =
                        super::stacks::unacknowledged(&p.warnings, &input.acknowledged_warnings)
                    {
                        return Err(bad(msg));
                    }
                }
                Err((_, Json(body))) => {
                    blocked.insert(
                        t.host.clone(),
                        body.get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("preflight refused")
                            .to_string(),
                    );
                }
            }
        }
        let plannable: Vec<plan::ResolvedTarget> = resolved
            .iter()
            .map(|t| {
                if blocked.contains_key(&t.host) {
                    plan::ResolvedTarget {
                        eligible: false,
                        note: Some(format!("blocked by preflight: {}", blocked[&t.host])),
                        ..t.clone()
                    }
                } else {
                    t.clone()
                }
            })
            .collect();
        if !plannable.iter().any(|t| t.eligible) {
            return Err(bad("no host passes preflight — nothing to roll out to"));
        }
        let stages = plan::plan_stages(&plannable, &input.strategy);
        let now = super::now_ms();
        let targets: Vec<RolloutTarget> = plannable
            .iter()
            .map(|t| {
                let stage = stages
                    .iter()
                    .find(|(h, _)| h == &t.host)
                    .map(|(_, s)| *s)
                    .unwrap_or(0);
                RolloutTarget {
                    host: t.host.clone(),
                    site: t.site.clone(),
                    stage,
                    status: if stage == 0 {
                        TargetStatus::Skipped
                    } else {
                        TargetStatus::Queued
                    },
                    attempts: 0,
                    last_error: if stage == 0 { t.note.clone() } else { None },
                    stack_id: None,
                    previous_version: previous.get(&t.host).cloned().flatten(),
                    agent_status: t.agent_status,
                    updated_ms: now,
                    applied_generation: None,
                    healthy_since_ms: None,
                }
            })
            .collect();
        let stage_count = targets.iter().map(|t| t.stage).max().unwrap_or(1).max(1);
        let previous_version = targets.iter().find_map(|t| t.previous_version.clone());
        let rollout = Rollout {
            id: new_id("ro"),
            name,
            template_id: template.id.clone(),
            template_name: template.name.clone(),
            template_version_id: version.id.clone(),
            template_version: version.version.clone(),
            previous_version,
            stack_name,
            selector: input.selector.clone(),
            strategy: input.strategy.clone(),
            state: RolloutState::Draft,
            stage: 0,
            stages: stage_count,
            targets,
            variables: input.variables.clone(),
            created_by: me.username.clone(),
            created_ms: now,
            updated_ms: now,
            started_ms: None,
            finished_ms: None,
            deploy_options: input.deploy_options.clone(),
            acknowledged_warnings: input.acknowledged_warnings.clone(),
        };
        // The secrets typed once ride with the rollout (encrypted) and are copied to each
        // target's stack when its stage applies.
        if !input.secrets.is_empty() {
            let blob = SecretBlob {
                stack_id: rollout.id.clone(),
                values: input
                    .secrets
                    .iter()
                    .filter(|(_, v)| !v.is_empty())
                    .map(|(k, v)| (k.clone(), self.cipher.encrypt(v)))
                    .collect(),
            };
            self.store.put_secrets(&blob).await.map_err(store_err)?;
        }
        self.store.put_rollout(&rollout).await.map_err(store_err)?;
        let eligible = rollout.targets.iter().filter(|t| t.stage > 0).count();
        self.audit(
            me,
            "rollout.create",
            AuditTarget {
                rollout: Some(rollout.name.clone()),
                template: Some(template.id.clone()),
                version: Some(version.version.clone()),
                ..Default::default()
            },
            AuditResultKind::Ok,
            Some(format!(
                "{eligible} host{} in {} stage{} · canary {}{}",
                if eligible == 1 { "" } else { "s" },
                rollout.stages,
                if rollout.stages == 1 { "" } else { "s" },
                rollout.strategy.canary,
                if rollout.strategy.canary_is_percent {
                    "%"
                } else {
                    ""
                }
            )),
            None,
        )
        .await;
        let mut rollout = rollout;
        if input.start.unwrap_or(true) {
            self.rollout_action(me, &mut rollout, RolloutActionKind::Start)
                .await?;
        }
        Ok(rollout)
    }

    /// An operator's decision on a rollout. State moves here; the supervisor does the applying.
    pub async fn rollout_action(
        &self,
        me: &CurrentUser,
        r: &mut Rollout,
        kind: RolloutActionKind,
    ) -> Result<(), ApiErr> {
        let now = super::now_ms();
        let refuse = |what: &str| -> ApiErr {
            super::err(
                StatusCode::CONFLICT,
                format!(
                    "cannot {what} a rollout that is {}",
                    r.state.as_str().replace('_', " ")
                ),
            )
        };
        let detail = match kind {
            RolloutActionKind::Start => {
                if r.state != RolloutState::Draft {
                    return Err(refuse("start"));
                }
                r.state = RolloutState::Running;
                r.stage = 1;
                r.started_ms = Some(now);
                format!("stage 1 (canary) of {}", r.stages)
            }
            RolloutActionKind::Promote => {
                if r.state != RolloutState::PausedGate {
                    return Err(refuse("promote"));
                }
                r.stage += 1;
                r.state = RolloutState::Running;
                format!("stage {} of {}", r.stage, r.stages)
            }
            RolloutActionKind::Pause => {
                if r.state != RolloutState::Running {
                    return Err(refuse("pause"));
                }
                r.state = RolloutState::PausedGate;
                "paused by the operator; queued targets wait".into()
            }
            RolloutActionKind::Resume => {
                if !matches!(
                    r.state,
                    RolloutState::PausedGate | RolloutState::PausedThreshold
                ) {
                    return Err(refuse("resume"));
                }
                r.state = RolloutState::Running;
                format!("resumed at stage {}", r.stage)
            }
            RolloutActionKind::Retry => {
                if !matches!(
                    r.state,
                    RolloutState::PausedThreshold
                        | RolloutState::Running
                        | RolloutState::PausedGate
                ) {
                    return Err(refuse("retry"));
                }
                let mut n = 0;
                for t in r
                    .targets
                    .iter_mut()
                    .filter(|t| t.stage == r.stage && t.status == TargetStatus::Failed)
                {
                    t.status = TargetStatus::Queued;
                    t.last_error = None;
                    t.updated_ms = now;
                    n += 1;
                }
                r.state = RolloutState::Running;
                format!(
                    "{n} failed target{} queued again",
                    if n == 1 { "" } else { "s" }
                )
            }
            RolloutActionKind::Abort => {
                if r.state.is_terminal() {
                    return Err(refuse("abort"));
                }
                r.state = RolloutState::Aborted;
                r.finished_ms = Some(now);
                let queued = r
                    .targets
                    .iter()
                    .filter(|t| t.status == TargetStatus::Queued)
                    .count();
                format!(
                    "{queued} queued target{} never applied; applied hosts keep what they run",
                    if queued == 1 { "" } else { "s" }
                )
            }
            RolloutActionKind::Rollback => {
                if r.state == RolloutState::Draft {
                    return Err(refuse("roll back"));
                }
                // Apply the previous version with the previous variables on every host this
                // rollout touched — just another apply (R6.5).
                let mut n = 0;
                for t in r
                    .targets
                    .iter_mut()
                    .filter(|t| t.stack_id.is_some() && t.status != TargetStatus::Queued)
                {
                    let Some(sid) = t.stack_id.clone() else {
                        continue;
                    };
                    let Some(mut s) = self.store.get_stack(&sid).await.map_err(store_err)? else {
                        continue;
                    };
                    let Some(prev) = s.previous.take() else {
                        t.last_error = Some("no previous generation to roll back to".into());
                        continue;
                    };
                    let snapshot = StackSnapshot {
                        template_version_id: s.template_version_id.clone(),
                        template_version: s.template_version.clone(),
                        files: s.files.clone(),
                        variables: s.variables.clone(),
                        deploy_options: s.deploy_options.clone(),
                        generation: s.desired_generation,
                    };
                    s.template_version_id = prev.template_version_id;
                    s.template_version = prev.template_version;
                    s.files = prev.files;
                    s.variables = prev.variables;
                    s.deploy_options = prev.deploy_options;
                    s.previous = Some(Box::new(snapshot));
                    self.push_generation(&mut s, StackOp::Up).await?;
                    t.status = if s.status == StackStatus::Applying {
                        TargetStatus::Applying
                    } else {
                        TargetStatus::Pending
                    };
                    t.attempts += 1;
                    t.updated_ms = now;
                    t.applied_generation = Some(s.desired_generation);
                    t.healthy_since_ms = None;
                    n += 1;
                }
                r.state = RolloutState::RolledBack;
                r.finished_ms = Some(now);
                format!(
                    "{n} host{} re-applying {}",
                    if n == 1 { "" } else { "s" },
                    r.previous_version
                        .clone()
                        .unwrap_or_else(|| "the previous generation".into())
                )
            }
        };
        r.updated_ms = now;
        self.store.put_rollout(r).await.map_err(store_err)?;
        self.audit(
            me,
            kind.audit_action(),
            AuditTarget {
                rollout: Some(r.name.clone()),
                template: Some(r.template_id.clone()),
                version: Some(r.template_version.clone()),
                ..Default::default()
            },
            AuditResultKind::Ok,
            Some(detail),
            None,
        )
        .await;
        Ok(())
    }
}

pub(crate) async fn list(State(state): State<AppState>) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    let mut out = Vec::new();
    for r in a.store.list_rollouts().await.map_err(store_err)? {
        out.push(a.rollout_view(&r).await?);
    }
    out.sort_by_key(|x| std::cmp::Reverse(x.updated_ms));
    Ok(Json(json!({ "rollouts": out })))
}

pub(crate) async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Rollout>, ApiErr> {
    let a = api(&state)?;
    let r = a.rollout(&id).await?;
    Ok(Json(a.rollout_view(&r).await?))
}

pub(crate) async fn create(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(input): Json<RolloutInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Deploy)?;
    let r = a.create_rollout(&me, &input).await?;
    Ok(Json(
        json!({ "ok": true, "rollout": a.rollout_view(&r).await? }),
    ))
}

pub(crate) async fn action(
    State(state): State<AppState>,
    me: CurrentUser,
    Path((id, kind)): Path<(String, String)>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Deploy)?;
    let kind =
        RolloutActionKind::parse(&kind).ok_or_else(|| not_found("no such rollout action"))?;
    let mut r = a.rollout(&id).await?;
    a.rollout_action(&me, &mut r, kind).await?;
    Ok(Json(
        json!({ "ok": true, "rollout": a.rollout_view(&r).await? }),
    ))
}

#[allow(dead_code)]
fn _keep(_: fn() -> String) {
    let _ = render::generate_secret;
}

//! Stacks (R4): one compose project on one host under desired state. The stored row is what
//! the operator declared; `services` and `drift` are resolved against the host's latest
//! inventory when a row is served, so the list never lies about what runs.
//!
//! Writing a stack bumps its `desired_generation` and the host's document generation; the agent
//! sees the new generation on its next poll and applies it. Nothing leaves the building until
//! the agent asks.

use super::{api, bad, not_found, require, store_err, ApiErr, DeployApi};
use crate::auth::CurrentUser;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use photon_deploy::compose::{is_compose_file, validate_stack_files};
use photon_deploy::diff;
use photon_deploy::ids::new_id;
use photon_deploy::model::*;
use photon_deploy::store::ObservedRecord;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// A stack as the UI reads it: services + drift from the host's last inventory.
pub fn view(stack: &Stack, obs: Option<&ObservedRecord>) -> Stack {
    let mut out = stack.clone();
    if let Some(o) = obs {
        out.services = diff::observed_services(stack, &o.observed.containers);
        let os = o.observed.stacks.iter().find(|s| s.name == stack.name);
        out.drift = if matches!(stack.status, StackStatus::Running | StackStatus::Drifted) {
            diff::compute(
                stack,
                &o.observed.containers,
                os,
                &stack.drift,
                super::now_ms(),
            )
        } else {
            vec![]
        };
    } else {
        out.services = diff::desired_services(stack)
            .into_iter()
            .map(|name| StackServiceState {
                name: name.clone(),
                image: diff::desired_images(stack)
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| "?".into()),
                containers: vec![],
            })
            .collect();
    }
    // Never hand the UI the server-side bookkeeping it does not read.
    out.previous = None;
    out
}

/// Unacknowledged dangerous constructs (R8.4) → the message the UI shows.
pub fn unacknowledged(warnings: &[ComposeWarning], acked: &[String]) -> Option<String> {
    let missing: Vec<&ComposeWarning> =
        warnings.iter().filter(|w| !acked.contains(&w.id)).collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "{} dangerous construct{} need{} an explicit acknowledgement: {}",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" },
            if missing.len() == 1 { "s" } else { "" },
            missing
                .iter()
                .map(|w| w.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ))
    }
}

impl DeployApi {
    pub async fn stack(&self, id: &str) -> Result<Stack, ApiErr> {
        self.store
            .get_stack(id)
            .await
            .map_err(store_err)?
            .ok_or_else(|| not_found("no such stack"))
    }

    pub async fn stack_on(&self, host: &str, name: &str) -> Result<Option<Stack>, ApiErr> {
        Ok(self
            .store
            .list_stacks()
            .await
            .map_err(store_err)?
            .into_iter()
            .find(|s| s.host == host && s.name == name && s.status != StackStatus::Gone))
    }

    /// Write a new desired generation for a stack and wake its host. `op` is what the agent
    /// should do with it; `status` what the UI shows meanwhile.
    pub async fn push_generation(&self, stack: &mut Stack, op: StackOp) -> Result<(), ApiErr> {
        let now = super::now_ms();
        stack.desired_generation += 1;
        stack.pending_op = Some(op);
        stack.updated_ms = now;
        stack.last_error = None;
        let rec = self.host(&stack.host).await?;
        stack.status = match op {
            StackOp::Destroy => StackStatus::Destroying,
            _ if rec.status(now) == AgentStatus::Online => StackStatus::Applying,
            _ => StackStatus::Pending,
        };
        self.store.put_stack(stack).await.map_err(store_err)?;
        self.bump_host(&stack.host).await
    }

    /// Create a stack from raw compose files (admin; R8.2) or save it declared-only.
    pub async fn create_compose_stack(
        &self,
        me: &CurrentUser,
        input: &StackInput,
    ) -> Result<Stack, ApiErr> {
        let name = input.name.trim();
        if !name_ok(name) {
            return Err(bad("stack name must be lowercase letters, digits, '-' or '_' and start with a letter or digit"));
        }
        let rec = self.host(&input.host).await?;
        if !rec.capabilities().contains(&AgentCapability::Apply) {
            return Err(bad(
                "the agent on this host was installed without --docker-apply",
            ));
        }
        if self.stack_on(&input.host, name).await?.is_some() {
            return Err(super::err(
                StatusCode::CONFLICT,
                format!("a stack named {name} already exists on {}", input.host),
            ));
        }
        let v = validate_stack_files(&input.files);
        if let Some(e) = v.error {
            return Err(bad(e));
        }
        if let Some(msg) = unacknowledged(&v.warnings, &input.acknowledged_warnings) {
            return Err(bad(msg));
        }
        let now = super::now_ms();
        let mut stack = Stack {
            id: new_id("st"),
            name: name.to_string(),
            host: input.host.clone(),
            site: rec.site.clone(),
            source: StackSource::Compose,
            template_id: None,
            template_name: None,
            template_version: None,
            template_version_id: None,
            status: StackStatus::Declared,
            desired_generation: 0,
            applied_generation: None,
            files: input.files.clone(),
            variables: BTreeMap::new(),
            secrets: vec![],
            deploy_options: input.deploy_options.clone(),
            services: vec![],
            drift: vec![],
            apm_services: vec![],
            acknowledged_warnings: input.acknowledged_warnings.clone(),
            last_error: None,
            created_by: me.username.clone(),
            created_ms: now,
            updated_ms: now,
            last_applied_ms: None,
            pending_op: None,
            baseline_config_hashes: BTreeMap::new(),
            previous: None,
            drift_since_ms: None,
        };
        let apply = input.apply.unwrap_or(true);
        if apply {
            self.push_generation(&mut stack, StackOp::Up).await?;
        } else {
            self.store.put_stack(&stack).await.map_err(store_err)?;
        }
        let payload = serde_json::to_vec(&stack.files).unwrap_or_default();
        self.audit(
            me,
            "stack.create",
            AuditTarget {
                host: Some(stack.host.clone()),
                stack: Some(stack.name.clone()),
                ..Default::default()
            },
            if apply {
                AuditResultKind::Pending
            } else {
                AuditResultKind::Ok
            },
            Some(format!(
                "{} service{} · {}",
                v.services.len(),
                if v.services.len() == 1 { "" } else { "s" },
                if apply {
                    "applying"
                } else {
                    "saved, not applied"
                }
            )),
            Some(&payload),
        )
        .await;
        Ok(stack)
    }
}

pub(crate) async fn list(State(state): State<AppState>) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    let stacks = a.store.list_stacks().await.map_err(store_err)?;
    let mut out = Vec::with_capacity(stacks.len());
    let mut observed: BTreeMap<String, Option<ObservedRecord>> = BTreeMap::new();
    for s in stacks.iter().filter(|s| s.status != StackStatus::Gone) {
        if !observed.contains_key(&s.host) {
            observed.insert(s.host.clone(), a.observed(&s.host).await?);
        }
        out.push(view(s, observed.get(&s.host).and_then(|o| o.as_ref())));
    }
    out.sort_by_key(|x| std::cmp::Reverse(x.updated_ms));
    Ok(Json(json!({ "stacks": out })))
}

pub(crate) async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Stack>, ApiErr> {
    let a = api(&state)?;
    let s = a.stack(&id).await?;
    Ok(Json(a.stack_view(&s).await?))
}

pub(crate) async fn create(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(input): Json<StackInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Compose)?;
    let s = a.create_compose_stack(&me, &input).await?;
    Ok(Json(
        json!({ "ok": true, "stack": a.stack_view(&s).await? }),
    ))
}

pub(crate) async fn patch(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(id): Path<String>,
    Json(p): Json<StackFilesPatch>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Compose)?;
    let mut s = a.stack(&id).await?;
    if s.status == StackStatus::Gone {
        return Err(bad("this stack was destroyed"));
    }
    let v = validate_stack_files(&p.files);
    if let Some(e) = v.error {
        return Err(bad(e));
    }
    let mut acked = s.acknowledged_warnings.clone();
    acked.extend(p.acknowledged_warnings.iter().cloned());
    if let Some(msg) = unacknowledged(&v.warnings, &acked) {
        return Err(bad(msg));
    }
    s.previous = Some(Box::new(StackSnapshot {
        template_version_id: s.template_version_id.clone(),
        template_version: s.template_version.clone(),
        files: s.files.clone(),
        variables: s.variables.clone(),
        deploy_options: s.deploy_options.clone(),
        generation: s.desired_generation,
    }));
    s.files = p.files.clone();
    s.acknowledged_warnings = acked;
    if let Some(o) = p.deploy_options {
        s.deploy_options = o;
    }
    // Saving is declaring a new generation; it does not apply by itself.
    s.desired_generation += 1;
    s.pending_op = Some(StackOp::Up);
    s.updated_ms = super::now_ms();
    if s.source == StackSource::Template {
        // Hand-edited: it no longer IS the template version.
        s.source = StackSource::Compose;
    }
    if matches!(
        s.status,
        StackStatus::Running | StackStatus::Drifted | StackStatus::Failed | StackStatus::Declared
    ) {
        s.status = StackStatus::Rendered;
    }
    a.store.put_stack(&s).await.map_err(store_err)?;
    let payload = serde_json::to_vec(&s.files).unwrap_or_default();
    a.audit(
        &me,
        "stack.edit",
        AuditTarget {
            host: Some(s.host.clone()),
            stack: Some(s.name.clone()),
            ..Default::default()
        },
        AuditResultKind::Ok,
        Some(format!(
            "saved as generation {} — apply to ship it",
            s.desired_generation
        )),
        Some(&payload),
    )
    .await;
    Ok(Json(
        json!({ "ok": true, "stack": a.stack_view(&s).await? }),
    ))
}

pub(crate) async fn action(
    State(state): State<AppState>,
    me: CurrentUser,
    Path((id, kind)): Path<(String, String)>,
    Json(args): Json<StackActionArgs>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Deploy)?;
    let kind = StackActionKind::parse(&kind).ok_or_else(|| not_found("no such stack action"))?;
    let mut s = a.stack(&id).await?;
    if s.status == StackStatus::Gone {
        return Err(bad("this stack was destroyed"));
    }
    let rec = a.host(&s.host).await?;
    if !rec.capabilities().contains(&AgentCapability::Apply) && kind != StackActionKind::Accept {
        a.audit(
            &me,
            kind.audit_action(),
            AuditTarget {
                host: Some(s.host.clone()),
                stack: Some(s.name.clone()),
                ..Default::default()
            },
            AuditResultKind::Denied,
            Some("agent installed without --docker-apply".into()),
            None,
        )
        .await;
        return Err(bad(
            "the agent on this host was installed without --docker-apply",
        ));
    }
    if matches!(s.status, StackStatus::Applying | StackStatus::Destroying)
        && kind != StackActionKind::Accept
    {
        return Err(super::err(
            StatusCode::CONFLICT,
            format!(
                "{} is {} — wait for the agent to report",
                s.name,
                s.status.as_str()
            ),
        ));
    }
    let detail = match kind {
        StackActionKind::Up => {
            a.push_generation(&mut s, StackOp::Up).await?;
            format!(
                "generation {} · pull={}",
                s.desired_generation,
                match s.deploy_options.pull {
                    PullPolicy::Missing => "missing",
                    PullPolicy::Always => "always",
                    PullPolicy::Never => "never",
                }
            )
        }
        StackActionKind::Down => {
            a.push_generation(&mut s, StackOp::Down).await?;
            "containers stopped and removed; volumes kept".into()
        }
        StackActionKind::Restart => {
            a.push_generation(&mut s, StackOp::Restart).await?;
            "restart every service".into()
        }
        StackActionKind::Redeploy => {
            if args.pull.unwrap_or(true) {
                s.deploy_options.pull = PullPolicy::Always;
            }
            a.push_generation(&mut s, StackOp::Redeploy).await?;
            "pull and recreate every service".into()
        }
        StackActionKind::Destroy => {
            let volumes = args.volumes.unwrap_or(false);
            s.desired_generation += 1;
            s.pending_op = Some(StackOp::Destroy);
            s.status = StackStatus::Destroying;
            s.updated_ms = super::now_ms();
            // The remove-volumes choice rides the deploy options for this last generation.
            s.deploy_options.recreate_volumes = volumes;
            a.store.put_stack(&s).await.map_err(store_err)?;
            a.bump_host(&s.host).await?;
            if volumes {
                "containers, networks AND volumes removed".into()
            } else {
                "containers and networks removed; volumes kept".into()
            }
        }
        StackActionKind::Reconcile => {
            a.push_generation(&mut s, StackOp::Reconcile).await?;
            format!(
                "re-apply generation {} over what runs",
                s.desired_generation
            )
        }
        StackActionKind::Accept => {
            // Take what runs as the baseline: the observed config hashes become the truth.
            if let Some(o) = a.observed(&s.host).await? {
                if let Some(os) = o.observed.stacks.iter().find(|x| x.name == s.name) {
                    s.baseline_config_hashes = os.config_hashes.clone();
                }
                // Images that changed on the host become the desired ones.
                let images: BTreeMap<String, String> = o
                    .observed
                    .containers
                    .iter()
                    .filter(|c| c.compose_project.as_deref() == Some(s.name.as_str()))
                    .filter_map(|c| c.compose_service.clone().map(|svc| (svc, c.image.clone())))
                    .collect();
                if let Some(f) = s.files.iter_mut().find(|f| is_compose_file(&f.path)) {
                    f.content = accept_images(&f.content, &images);
                }
            }
            s.drift = vec![];
            s.drift_since_ms = None;
            s.status = StackStatus::Running;
            s.updated_ms = super::now_ms();
            a.store.put_stack(&s).await.map_err(store_err)?;
            "observed state accepted as desired; drift cleared".into()
        }
    };
    a.audit(
        &me,
        kind.audit_action(),
        AuditTarget {
            host: Some(s.host.clone()),
            stack: Some(s.name.clone()),
            ..Default::default()
        },
        if kind == StackActionKind::Accept {
            AuditResultKind::Ok
        } else {
            AuditResultKind::Pending
        },
        Some(detail),
        None,
    )
    .await;
    Ok(Json(
        json!({ "ok": true, "stack": a.stack_view(&s).await? }),
    ))
}

/// Rewrite `image:` lines for services whose observed image differs — the textual part of
/// "accept drift". Line-based on purpose: the operator's file keeps its shape.
pub fn accept_images(compose: &str, observed: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(compose.len());
    let mut current: Option<String> = None;
    for line in compose.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if indent == 2 && t.ends_with(':') && !t.starts_with('#') {
            current = Some(t.trim_end_matches(':').to_string());
        } else if indent == 0 && !t.is_empty() {
            current = None;
        }
        if let (Some(svc), Some(rest)) = (&current, t.strip_prefix("image:")) {
            if let Some(img) = observed.get(svc) {
                let _ = rest;
                out.push_str(&format!("{}image: {img}\n", &line[..indent]));
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct ValidateBody {
    pub files: Vec<StackFile>,
}

pub(crate) async fn validate(
    State(state): State<AppState>,
    Json(body): Json<ValidateBody>,
) -> Result<Json<StackValidation>, ApiErr> {
    let _ = api(&state)?;
    Ok(Json(validate_stack_files(&body.files)))
}

#[derive(Debug, Deserialize)]
pub struct AdoptBody {
    pub host: String,
    pub project: String,
}

pub(crate) async fn adopt(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(body): Json<AdoptBody>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Deploy)?;
    let rec = a.host(&body.host).await?;
    if a.stack_on(&body.host, &body.project).await?.is_some() {
        return Err(super::err(
            StatusCode::CONFLICT,
            format!("{} on {} is already managed", body.project, body.host),
        ));
    }
    let inv = a.inventory(&body.host).await?;
    let mine: Vec<&Container> = inv
        .containers
        .iter()
        .filter(|c| c.compose_project.as_deref() == Some(body.project.as_str()))
        .collect();
    if mine.is_empty() {
        return Err(not_found(format!(
            "no compose project {} on {} in its last inventory",
            body.project, body.host
        )));
    }
    // Synthesize a compose file from what runs: image + restart policy per service. The real
    // file stays where the customer keeps it; this is the record Photon watches for drift.
    let mut services: Vec<(String, &Container)> = Vec::new();
    for c in &mine {
        if let Some(svc) = &c.compose_service {
            if !services.iter().any(|(s, _)| s == svc) {
                services.push((svc.clone(), c));
            }
        }
    }
    services.sort_by(|x, y| x.0.cmp(&y.0));
    let mut compose = String::from("services:\n");
    for (svc, c) in &services {
        compose.push_str(&format!("  {svc}:\n    image: {}\n", c.image));
        if !c.restart_policy.is_empty() && c.restart_policy != "no" {
            compose.push_str(&format!("    restart: {}\n", c.restart_policy));
        }
    }
    let obs = a.observed(&body.host).await?;
    let hashes = obs
        .as_ref()
        .and_then(|o| {
            o.observed
                .stacks
                .iter()
                .find(|s| s.name == body.project)
                .map(|s| s.config_hashes.clone())
        })
        .unwrap_or_default();
    let now = super::now_ms();
    let stack = Stack {
        id: new_id("st"),
        name: body.project.clone(),
        host: body.host.clone(),
        site: rec.site.clone(),
        source: StackSource::Adopted,
        template_id: None,
        template_name: None,
        template_version: None,
        template_version_id: None,
        status: StackStatus::Running,
        desired_generation: 1,
        applied_generation: Some(1),
        files: vec![StackFile {
            path: "compose.yaml".into(),
            content: compose,
        }],
        variables: BTreeMap::new(),
        secrets: vec![],
        deploy_options: DeployOptions::default(),
        services: vec![],
        drift: vec![],
        apm_services: vec![],
        acknowledged_warnings: vec![],
        last_error: None,
        created_by: me.username.clone(),
        created_ms: now,
        updated_ms: now,
        last_applied_ms: Some(now),
        pending_op: None,
        baseline_config_hashes: hashes,
        previous: None,
        drift_since_ms: None,
    };
    a.store.put_stack(&stack).await.map_err(store_err)?;
    let running = mine
        .iter()
        .filter(|c| c.state == ContainerState::Running)
        .count();
    a.audit(
        &me,
        "stack.adopt",
        AuditTarget {
            host: Some(body.host.clone()),
            stack: Some(body.project.clone()),
            ..Default::default()
        },
        AuditResultKind::Ok,
        Some(format!(
            "{running} running container{} taken under management without recreating them",
            if running == 1 { "" } else { "s" }
        )),
        None,
    )
    .await;
    Ok(Json(
        json!({ "ok": true, "stack": a.stack_view(&stack).await? }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_rewrites_only_changed_images() {
        let compose = "services:\n  api:\n    image: a:1\n    restart: always\n  db:\n    image: postgres:16\n";
        let obs = BTreeMap::from([("api".to_string(), "a:2".to_string())]);
        let out = accept_images(compose, &obs);
        assert!(out.contains("    image: a:2\n"));
        assert!(out.contains("    image: postgres:16\n"));
        assert!(out.contains("restart: always"));
    }

    #[test]
    fn names() {
        assert!(name_ok("nodeflux-vas"));
        assert!(name_ok("a1_b"));
        assert!(!name_ok("Bad"));
        assert!(!name_ok("-x"));
        assert!(!name_ok(""));
    }
}

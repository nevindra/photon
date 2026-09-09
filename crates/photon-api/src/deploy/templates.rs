//! Templates (R5): versioned, parameterized recipes. Versions are immutable (R5.4); publishing a
//! change publishes a new version. Rendering is server-side (R5.5) and the rendered artifact is
//! what a stack stores. Dry run = render + preflight against the host's facts (R5.6). Secrets
//! are encrypted at rest and never read back (R5.9 / R8.5): the render ships their NAMES, the
//! agent fetches the values at apply time.

use super::stacks::unacknowledged;
use super::{api, bad, not_found, require, store_err, ApiErr, DeployApi};
use crate::auth::CurrentUser;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use photon_deploy::compose::{is_compose_file, lint_compose};
use photon_deploy::ids::new_id;
use photon_deploy::model::*;
use photon_deploy::portainer::parse_portainer_templates;
use photon_deploy::render::{self, RenderInput};
use photon_deploy::store::SecretBlob;
use photon_deploy::{plan, preflight};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// What a template deploy resolved to before anything is stored.
pub struct Prepared {
    pub version: TemplateVersion,
    pub host: HostRecord,
    pub existing: Option<Stack>,
    pub values: BTreeMap<String, String>,
    pub rendered: render::Rendered,
    pub checks: Vec<PreflightCheck>,
    pub warnings: Vec<ComposeWarning>,
}

impl DeployApi {
    pub async fn template(&self, id: &str) -> Result<Template, ApiErr> {
        self.store
            .get_template(id)
            .await
            .map_err(store_err)?
            .ok_or_else(|| not_found("no such template"))
    }

    pub async fn version(&self, id: &str) -> Result<TemplateVersion, ApiErr> {
        self.store
            .get_version(id)
            .await
            .map_err(store_err)?
            .ok_or_else(|| not_found("no such template version"))
    }

    /// The catalog row with its version list and stack count refreshed.
    pub async fn template_view(&self, t: &Template) -> Result<Template, ApiErr> {
        let versions = self.store.list_versions(&t.id).await.map_err(store_err)?;
        let stacks = self.store.list_stacks().await.map_err(store_err)?;
        let mut refs: Vec<TemplateVersionRef> = versions
            .iter()
            .map(|v| {
                let mut r = v.as_ref();
                r.deployed_count = stacks
                    .iter()
                    .filter(|s| {
                        s.template_version_id.as_deref() == Some(v.id.as_str())
                            && s.status != StackStatus::Gone
                    })
                    .count() as u64;
                r
            })
            .collect();
        refs.sort_by(|a, b| render::semver_compare(&b.version, &a.version));
        let mut out = t.clone();
        out.latest_version = refs
            .first()
            .map(|r| r.version.clone())
            .unwrap_or_else(|| t.latest_version.clone());
        out.stacks = stacks
            .iter()
            .filter(|s| {
                s.template_id.as_deref() == Some(t.id.as_str()) && s.status != StackStatus::Gone
            })
            .count() as u64;
        out.versions = refs;
        Ok(out)
    }

    /// Publish a new immutable version (creating the template row on first publish).
    pub async fn publish(
        &self,
        me: &CurrentUser,
        input: &TemplateVersionInput,
        source: TemplateSource,
        registry_id: Option<String>,
    ) -> Result<(TemplateVersion, Template), ApiErr> {
        render::validate_manifest(&input.manifest).map_err(bad)?;
        if input.compose_text.trim().is_empty() {
            return Err(bad("compose_text is required"));
        }
        if photon_deploy::compose::compose_services(&input.compose_text).is_empty() {
            return Err(bad("the compose file declares no services"));
        }
        let template_id = input
            .template_id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| input.manifest.id.clone());
        if template_id != input.manifest.id {
            return Err(bad(format!(
                "manifest id '{}' does not match template '{template_id}'",
                input.manifest.id
            )));
        }
        let existing = self
            .store
            .get_template(&template_id)
            .await
            .map_err(store_err)?;
        if let Some(t) = &existing {
            for v in self.store.list_versions(&t.id).await.map_err(store_err)? {
                if v.version == input.manifest.version {
                    return Err(super::err(
                        StatusCode::CONFLICT,
                        format!(
                            "{} {} already exists — versions are immutable, publish {} instead",
                            t.name,
                            v.version,
                            render::next_version(&v.version)
                        ),
                    ));
                }
            }
        }
        let now = super::now_ms();
        let vid = format!(
            "tv_{}_{}",
            template_id,
            input.manifest.version.replace(['.', '+', '-'], "_")
        );
        let version = TemplateVersion {
            id: vid,
            template_id: template_id.clone(),
            version: input.manifest.version.clone(),
            created_ms: now,
            created_by: me.username.clone(),
            notes: input.notes.clone().filter(|s| !s.trim().is_empty()),
            deployed_count: 0,
            manifest: input.manifest.clone(),
            compose_text: input.compose_text.clone(),
        };
        self.store
            .insert_version(&version)
            .await
            .map_err(|e| super::err(StatusCode::CONFLICT, e))?;
        let m = &input.manifest;
        let template = Template {
            id: template_id.clone(),
            name: m.name.clone(),
            description: m.description.clone(),
            icon: m.icon.clone(),
            categories: m.categories.clone(),
            source: existing.as_ref().map(|t| t.source).unwrap_or(source),
            registry_id: existing
                .as_ref()
                .and_then(|t| t.registry_id.clone())
                .or(registry_id),
            latest_version: m.version.clone(),
            versions: vec![],
            updated_ms: now,
            stacks: 0,
        };
        self.store
            .put_template(&template)
            .await
            .map_err(store_err)?;
        let payload = input.compose_text.as_bytes();
        self.audit(
            me,
            "template.publish",
            AuditTarget {
                template: Some(template_id),
                version: Some(m.version.clone()),
                ..Default::default()
            },
            AuditResultKind::Ok,
            Some(format!(
                "{} {} — {} variable{}",
                m.name,
                m.version,
                m.variables.len(),
                if m.variables.len() == 1 { "" } else { "s" }
            )),
            Some(payload),
        )
        .await;
        let view = self.template_view(&template).await?;
        Ok((version, view))
    }

    /// Render + preflight one deploy without storing anything.
    pub async fn prepare(&self, input: &TemplateDeployInput) -> Result<Prepared, ApiErr> {
        let version = self.version(&input.template_version_id).await?;
        let host = self.host(&input.host).await?;
        let name = input.name.trim().to_string();
        if name.is_empty() {
            return Err(bad("a stack name is required"));
        }
        let existing = match &input.stack_id {
            Some(id) if !id.is_empty() => Some(self.stack(id).await?),
            _ => self.stack_on(&input.host, &name).await?,
        };
        if let Some(e) = &existing {
            if e.host != input.host {
                return Err(bad("that stack lives on another host"));
            }
        }
        let m = &version.manifest;
        // Values: the template's defaults + what the form sent; presets always win (an upgrade
        // must not keep the previous version's pin).
        let mut values = render::default_variables(
            m,
            existing
                .as_ref()
                .map(|s| &s.variables)
                .unwrap_or(&BTreeMap::new()),
        );
        for (k, v) in &input.variables {
            if m.variables.iter().any(|x| {
                &x.name == k
                    && x.preset != Some(true)
                    && !matches!(x.kind, VariableType::Secret | VariableType::Generated)
            }) {
                values.insert(k.clone(), v.clone());
            }
        }
        let stored_secrets: BTreeSet<String> = existing
            .as_ref()
            .map(|s| s.secrets.iter().cloned().collect())
            .unwrap_or_default();
        let mut provided: BTreeSet<String> = input
            .secrets
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, _)| k.clone())
            .collect();
        provided.extend(stored_secrets);
        let errors = render::validate_variables(m, &values, &provided);
        if let Some((k, e)) = errors.iter().next() {
            return Err(bad(format!("{k}: {e}")));
        }
        // The Photon block: the endpoint a host already uses is kept, a new host gets ours.
        let otlp = existing
            .as_ref()
            .and_then(|s| s.variables.get("OTEL_EXPORTER_OTLP_ENDPOINT").cloned())
            .or_else(|| {
                self.ingest.as_ref().map(|i| {
                    i.metrics_endpoint
                        .trim_end_matches("/v1/metrics")
                        .to_string()
                })
            });
        let environment = values
            .get("ENVIRONMENT")
            .cloned()
            .or_else(|| values.get("DEPLOYMENT_ENVIRONMENT").cloned());
        let rendered = render::render(RenderInput {
            manifest: m,
            compose_text: &version.compose_text,
            values: &values,
            otlp_endpoint: otlp.as_deref(),
            ingest_token: self.ingest.as_ref().map(|i| i.token.as_str()),
            environment: environment.as_deref(),
        });
        if let Some(o) = &otlp {
            if m.observability.otlp_endpoint {
                values.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), o.clone());
            }
        }
        let obs = self.observed(&input.host).await?;
        let containers = obs
            .as_ref()
            .map(|o| o.observed.containers.clone())
            .unwrap_or_default();
        let hv = self.host_view(&host);
        let images: Vec<String> = photon_deploy::compose::service_images(&rendered.compose)
            .into_values()
            .collect();
        let credentials = self.credentials_for_host(&host).await?;
        let target = preflight::Target {
            host: &hv,
            containers: &containers,
            total_ram_bytes: None,
            stack_id: existing.as_ref().map(|s| s.id.as_str()),
            stack_name: &name,
            now_ms: super::now_ms(),
            images: &images,
            credentials: &credentials,
        };
        let pull = match input.deploy_options.pull {
            PullPolicy::Missing => "missing",
            PullPolicy::Always => "always",
            PullPolicy::Never => "never",
        };
        let checks = preflight::run(m, &target, &values, pull);
        let warnings = lint_compose(&rendered.compose);
        Ok(Prepared {
            version,
            host,
            existing,
            values,
            rendered,
            checks,
            warnings,
        })
    }

    pub fn dry_run_result(&self, input: &TemplateDeployInput, p: &Prepared) -> DryRunResult {
        DryRunResult {
            ok: preflight::passed(&p.checks),
            host: input.host.clone(),
            checks: p.checks.clone(),
            compose: p.rendered.compose.clone(),
            env: p.rendered.env.clone(),
            previous_compose: p.existing.as_ref().and_then(|s| {
                s.files
                    .iter()
                    .find(|f| is_compose_file(&f.path))
                    .map(|f| f.content.clone())
            }),
            warnings: p.warnings.clone(),
            secrets: p.rendered.secret_names.clone(),
            generated: p.rendered.generated_names.clone(),
            error: None,
        }
    }

    /// Deploy (or upgrade in place): store the rendered generation, the encrypted secrets, and
    /// hand the host its new desired state. Shared by the single deploy and the rollout.
    pub async fn deploy_template(
        &self,
        me: &CurrentUser,
        input: &TemplateDeployInput,
        prepared: Prepared,
        prepull: bool,
        via_rollout: Option<&str>,
    ) -> Result<Stack, ApiErr> {
        let Prepared {
            version,
            host,
            existing,
            values,
            rendered,
            checks,
            warnings,
        } = prepared;
        if !preflight::passed(&checks) {
            let why = checks
                .iter()
                .filter(|c| c.status == CheckStatus::Fail)
                .map(|c| c.detail.clone())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(bad(format!("preflight failed: {why}")));
        }
        let mut acked = existing
            .as_ref()
            .map(|s| s.acknowledged_warnings.clone())
            .unwrap_or_default();
        acked.extend(input.acknowledged_warnings.iter().cloned());
        if let Some(msg) = unacknowledged(&warnings, &acked) {
            return Err(bad(msg));
        }
        if !host.capabilities().contains(&AgentCapability::Apply) {
            return Err(bad(
                "the agent on this host was installed without --docker-apply",
            ));
        }
        let now = super::now_ms();
        let m = &version.manifest;
        let name = input.name.trim().to_string();
        let files = vec![
            StackFile {
                path: "compose.yaml".into(),
                content: rendered.compose.clone(),
            },
            StackFile {
                path: ".env".into(),
                content: rendered.env.clone(),
            },
        ];
        let mut secret_names: Vec<String> = rendered.secret_names.clone();
        secret_names.extend(rendered.generated_names.iter().cloned());
        secret_names.sort();
        secret_names.dedup();

        let mut stack = match existing {
            Some(mut s) => {
                s.previous = Some(Box::new(StackSnapshot {
                    template_version_id: s.template_version_id.clone(),
                    template_version: s.template_version.clone(),
                    files: s.files.clone(),
                    variables: s.variables.clone(),
                    deploy_options: s.deploy_options.clone(),
                    generation: s.desired_generation,
                }));
                s.source = StackSource::Template;
                s.template_id = Some(version.template_id.clone());
                s.template_name = Some(m.name.clone());
                s.template_version = Some(version.version.clone());
                s.template_version_id = Some(version.id.clone());
                s.files = files;
                s.variables = values.clone();
                s.secrets = secret_names.clone();
                s.deploy_options = input.deploy_options.clone();
                s.apm_services = if m.observability.otlp_endpoint {
                    rendered.apm_services.clone()
                } else {
                    vec![]
                };
                s.acknowledged_warnings = acked;
                s
            }
            None => Stack {
                id: new_id("st"),
                name: name.clone(),
                host: input.host.clone(),
                site: host.site.clone(),
                source: StackSource::Template,
                template_id: Some(version.template_id.clone()),
                template_name: Some(m.name.clone()),
                template_version: Some(version.version.clone()),
                template_version_id: Some(version.id.clone()),
                status: StackStatus::Rendered,
                desired_generation: 0,
                applied_generation: None,
                files,
                variables: values.clone(),
                secrets: secret_names.clone(),
                deploy_options: input.deploy_options.clone(),
                services: vec![],
                drift: vec![],
                apm_services: if m.observability.otlp_endpoint {
                    rendered.apm_services.clone()
                } else {
                    vec![]
                },
                acknowledged_warnings: acked,
                last_error: None,
                created_by: me.username.clone(),
                created_ms: now,
                updated_ms: now,
                last_applied_ms: None,
                pending_op: None,
                baseline_config_hashes: BTreeMap::new(),
                previous: None,
                drift_since_ms: None,
            },
        };
        // Secrets: provided values replace, blanks keep, generated ones are minted once.
        let mut blob = self
            .store
            .get_secrets(&stack.id)
            .await
            .map_err(store_err)?
            .unwrap_or(SecretBlob {
                stack_id: stack.id.clone(),
                values: BTreeMap::new(),
            });
        for (k, v) in &input.secrets {
            if !v.is_empty() && rendered.secret_names.contains(k) {
                blob.values.insert(k.clone(), self.cipher.encrypt(v));
            }
        }
        for g in &rendered.generated_names {
            blob.values
                .entry(g.clone())
                .or_insert_with(|| self.cipher.encrypt(&render::generate_secret()));
        }
        // Only a `required` secret must have a value; an optional one (a RUM key, a licence
        // for a feature the site does not use) renders empty, exactly like an optional string.
        let missing: Vec<&String> = rendered
            .secret_names
            .iter()
            .filter(|n| !blob.values.contains_key(*n))
            .filter(|n| {
                version
                    .manifest
                    .variables
                    .iter()
                    .any(|v| v.name == **n && v.required)
            })
            .collect();
        if !missing.is_empty() {
            return Err(bad(format!(
                "secret value{} missing: {}",
                if missing.len() == 1 { "" } else { "s" },
                missing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        blob.values.retain(|k, _| secret_names.contains(k));
        self.store.put_secrets(&blob).await.map_err(store_err)?;
        // A rollout's pre-pull stage is the agent pulling before it applies; on the wire that is
        // the `Always` pull policy (`sync.rs` derives `prepull` from it), so a rollout asking for
        // it overrides the per-deploy `Missing` default — but never a deliberate `Never`.
        if prepull && stack.deploy_options.pull != PullPolicy::Never {
            stack.deploy_options.pull = PullPolicy::Always;
        }
        self.push_generation(&mut stack, StackOp::Up).await?;
        let payload = rendered.compose.as_bytes();
        let what = if stack.previous.is_some() {
            "stack.apply"
        } else {
            "stack.create"
        };
        let detail = format!(
            "{} {} → generation {}{}",
            m.name,
            version.version,
            stack.desired_generation,
            via_rollout
                .map(|r| format!(" (rollout {r})"))
                .unwrap_or_default()
        );
        self.audit(
            me,
            what,
            AuditTarget {
                host: Some(stack.host.clone()),
                stack: Some(stack.name.clone()),
                template: Some(version.template_id.clone()),
                version: Some(version.version.clone()),
                ..Default::default()
            },
            AuditResultKind::Pending,
            Some(detail),
            Some(payload),
        )
        .await;
        Ok(stack)
    }
}

pub(crate) async fn list(State(state): State<AppState>) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    let mut out = Vec::new();
    for t in a.store.list_templates().await.map_err(store_err)? {
        out.push(a.template_view(&t).await?);
    }
    out.sort_by_key(|x| x.name.to_lowercase());
    Ok(Json(json!({ "templates": out })))
}

pub(crate) async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Template>, ApiErr> {
    let a = api(&state)?;
    let t = a.template(&id).await?;
    Ok(Json(a.template_view(&t).await?))
}

pub(crate) async fn version(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TemplateVersion>, ApiErr> {
    let a = api(&state)?;
    let mut v = a.version(&id).await?;
    v.deployed_count = a
        .store
        .list_stacks()
        .await
        .map_err(store_err)?
        .iter()
        .filter(|s| {
            s.template_version_id.as_deref() == Some(v.id.as_str()) && s.status != StackStatus::Gone
        })
        .count() as u64;
    Ok(Json(v))
}

pub(crate) async fn publish(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(input): Json<TemplateVersionInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Templates)?;
    let (version, template) = a.publish(&me, &input, TemplateSource::Local, None).await?;
    Ok(Json(
        json!({ "ok": true, "version": version, "template": template }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct ImportBody {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    pub json: String,
}

pub(crate) async fn import(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(body): Json<ImportBody>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Templates)?;
    if body.format.as_deref().unwrap_or("portainer") != "portainer" {
        return Err(bad("only the portainer format is supported"));
    }
    let source = body.source.clone().unwrap_or_else(|| "portainer".into());
    let report = parse_portainer_templates(&body.json, &source).map_err(bad)?;
    let mut imported = 0usize;
    let mut skipped = report.skipped.len();
    for t in report.imported {
        let input = TemplateVersionInput {
            template_id: Some(t.manifest.id.clone()),
            manifest: t.manifest,
            compose_text: t.compose_text,
            notes: Some(format!("imported from {source}")),
        };
        match a
            .publish(&me, &input, TemplateSource::Portainer, None)
            .await
        {
            Ok(_) => imported += 1,
            Err(_) => skipped += 1,
        }
    }
    a.audit(
        &me,
        "template.import",
        AuditTarget {
            registry: Some(source.clone()),
            ..Default::default()
        },
        AuditResultKind::Ok,
        Some(format!(
            "{imported} imported, {skipped} skipped from {source} (portainer v{})",
            report.version
        )),
        None,
    )
    .await;
    Ok(Json(
        json!({ "ok": true, "imported": imported, "skipped": skipped }),
    ))
}

pub(crate) async fn dry_run(
    State(state): State<AppState>,
    Json(input): Json<TemplateDeployInput>,
) -> Result<Json<DryRunResult>, ApiErr> {
    let a = api(&state)?;
    match a.prepare(&input).await {
        Ok(p) => Ok(Json(a.dry_run_result(&input, &p))),
        Err((status, Json(body)))
            if status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND =>
        {
            let msg = body
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("dry run refused")
                .to_string();
            Ok(Json(DryRunResult::refused(&input.host, msg)))
        }
        Err(e) => Err(e),
    }
}

pub(crate) async fn deploy(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(input): Json<TemplateDeployInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Deploy)?;
    let prepared = a.prepare(&input).await?;
    let stack = a
        .deploy_template(&me, &input, prepared, false, None)
        .await?;
    Ok(Json(
        json!({ "ok": true, "stack": a.stack_view(&stack).await? }),
    ))
}

// --- registries (R5.7) ---

pub(crate) async fn registries(State(state): State<AppState>) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    let mut rs = a.store.list_registries().await.map_err(store_err)?;
    let templates = a.store.list_templates().await.map_err(store_err)?;
    for r in rs.iter_mut() {
        r.template_count = templates
            .iter()
            .filter(|t| t.registry_id.as_deref() == Some(r.id.as_str()))
            .count() as u64;
    }
    rs.sort_by(|x, y| x.name.cmp(&y.name));
    Ok(Json(json!({ "registries": rs })))
}

pub(crate) async fn add_registry(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(input): Json<RegistryInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Templates)?;
    let url = input.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(bad("registry url must start with http:// or https://"));
    }
    if input.name.trim().is_empty() {
        return Err(bad("a registry name is required"));
    }
    let r = Registry {
        id: new_id("rg"),
        name: input.name.trim().to_string(),
        url,
        kind: input.kind,
        last_sync_ms: None,
        template_count: 0,
        status: RegistryStatus::Syncing,
        error: None,
    };
    a.store.put_registry(&r).await.map_err(store_err)?;
    let r = sync_registry_now(a, &me, r).await;
    Ok(Json(
        json!({ "ok": r.status != RegistryStatus::Error, "registry": r, "error": r.error }),
    ))
}

pub(crate) async fn sync_registry(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Templates)?;
    let r = a
        .store
        .list_registries()
        .await
        .map_err(store_err)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| not_found("no such registry"))?;
    let r = sync_registry_now(a, &me, r).await;
    Ok(Json(
        json!({ "ok": r.status != RegistryStatus::Error, "registry": r, "error": r.error }),
    ))
}

/// Fetch a registry's JSON and import what it carries. Photon-kind registries are a JSON list
/// of `{manifest, compose_text}`; Portainer kinds are their template JSON.
async fn sync_registry_now(a: &DeployApi, me: &CurrentUser, mut r: Registry) -> Registry {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build();
    let fetched: Result<String, String> = match client {
        Ok(c) => match c.get(&r.url).send().await {
            Ok(resp) if resp.status().is_success() => resp.text().await.map_err(|e| e.to_string()),
            Ok(resp) => Err(format!("{} answered {}", r.url, resp.status())),
            Err(e) => Err(e.to_string()),
        },
        Err(e) => Err(e.to_string()),
    };
    let now = super::now_ms();
    match fetched {
        Err(e) => {
            r.status = RegistryStatus::Error;
            r.error = Some(e);
        }
        Ok(text) => {
            let result: Result<(usize, usize), String> = match r.kind {
                RegistryKind::Photon => {
                    #[derive(Deserialize)]
                    struct Entry {
                        manifest: TemplateManifest,
                        compose_text: String,
                    }
                    let parsed: Result<Vec<Entry>, String> = serde_json::from_str::<Vec<Entry>>(&text).or_else(|_| {
                        serde_json::from_str::<serde_json::Value>(&text)
                            .ok()
                            .and_then(|v| v.get("templates").cloned())
                            .ok_or_else(|| "not a Photon registry: expected a JSON array of {manifest, compose_text} or {templates: [...]}".to_string())
                            .and_then(|v| serde_json::from_value::<Vec<Entry>>(v).map_err(|e| format!("not a Photon registry: {e}")))
                    });
                    match parsed {
                        Ok(entries) => {
                            let (mut ok, mut skip) = (0, 0);
                            for e in entries {
                                let input = TemplateVersionInput {
                                    template_id: Some(e.manifest.id.clone()),
                                    manifest: e.manifest,
                                    compose_text: e.compose_text,
                                    notes: Some(format!("from registry {}", r.name)),
                                };
                                match a
                                    .publish(
                                        me,
                                        &input,
                                        TemplateSource::Registry,
                                        Some(r.id.clone()),
                                    )
                                    .await
                                {
                                    Ok(_) => ok += 1,
                                    Err(_) => skip += 1,
                                }
                            }
                            Ok((ok, skip))
                        }
                        Err(e) => Err(e),
                    }
                }
                RegistryKind::PortainerV2 | RegistryKind::PortainerV3 => {
                    match parse_portainer_templates(&text, &r.name) {
                        Ok(report) => {
                            let (mut ok, mut skip) = (0, report.skipped.len());
                            for t in report.imported {
                                let input = TemplateVersionInput {
                                    template_id: Some(t.manifest.id.clone()),
                                    manifest: t.manifest,
                                    compose_text: t.compose_text,
                                    notes: Some(format!("from registry {}", r.name)),
                                };
                                match a
                                    .publish(
                                        me,
                                        &input,
                                        TemplateSource::Registry,
                                        Some(r.id.clone()),
                                    )
                                    .await
                                {
                                    Ok(_) => ok += 1,
                                    Err(_) => skip += 1,
                                }
                            }
                            Ok((ok, skip))
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            match result {
                Ok((ok, skip)) => {
                    r.status = RegistryStatus::Ok;
                    r.error = None;
                    r.last_sync_ms = Some(now);
                    a.audit(
                        me,
                        "template.import",
                        AuditTarget {
                            registry: Some(r.name.clone()),
                            ..Default::default()
                        },
                        AuditResultKind::Ok,
                        Some(format!(
                            "{ok} new version{} from {}, {skip} already present or skipped",
                            if ok == 1 { "" } else { "s" },
                            r.url
                        )),
                        None,
                    )
                    .await;
                }
                Err(e) => {
                    r.status = RegistryStatus::Error;
                    r.error = Some(e);
                }
            }
        }
    }
    let templates = a.store.list_templates().await.unwrap_or_default();
    r.template_count = templates
        .iter()
        .filter(|t| t.registry_id.as_deref() == Some(r.id.as_str()))
        .count() as u64;
    let _ = a.store.put_registry(&r).await;
    r
}

pub(crate) async fn delete_registry(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Templates)?;
    if !a.store.delete_registry(&id).await.map_err(store_err)? {
        return Err(not_found("no such registry"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[allow(dead_code)]
fn _keep(_: fn(&TargetSelector, &[ManagedHost]) -> Vec<plan::ResolvedTarget>) {}

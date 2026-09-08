//! Image registry credentials (`/api/image-registries`): what a host presents when it pulls
//! from a private registry — a GitLab deploy token, a GHCR PAT, a Hub login. Admin-managed,
//! encrypted at rest through the same `SecretCipher` as stack secrets, write-only (the token
//! is never read back), and handed to an agent only over its own token channel
//! (`POST /api/agent/registry-auth`, in `sync.rs`) for the pull at hand. Distinct from
//! `/api/registries`, which are TEMPLATE catalogs.

use super::{api, bad, internal, not_found, require, store_err, ApiErr, DeployApi};
use crate::auth::CurrentUser;
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use photon_deploy::ids::new_id;
use photon_deploy::model::*;
use photon_deploy::proto::RegistryAuth;
use photon_deploy::registry::normalize_server;
use photon_deploy::store::RegistryCredentialRecord;
use serde_json::{json, Value};

impl DeployApi {
    /// The credentials a managed host is scoped to receive, public rows only.
    pub async fn credentials_for_host(
        &self,
        rec: &HostRecord,
    ) -> Result<Vec<RegistryCredential>, ApiErr> {
        Ok(self
            .credential_records_for_host(rec)
            .await?
            .into_iter()
            .map(|r| r.credential)
            .collect())
    }

    async fn credential_records_for_host(
        &self,
        rec: &HostRecord,
    ) -> Result<Vec<RegistryCredentialRecord>, ApiErr> {
        let mut rows = self
            .store
            .list_registry_credentials()
            .await
            .map_err(store_err)?;
        rows.retain(|r| match &r.credential.scope {
            None => true,
            Some(sel) => sel.matches(&rec.host, rec.site.as_deref(), &rec.labels),
        });
        rows.sort_by(|a, b| a.credential.server.cmp(&b.credential.server));
        Ok(rows)
    }

    /// Decrypted credentials for the agent: those scoped to the host, narrowed to `servers`
    /// when the agent names any. One row per registry — when two credentials claim the same
    /// registry the newest wins, so rotating by adding a new one behaves.
    pub async fn registry_auth_for(
        &self,
        rec: &HostRecord,
        servers: &[String],
    ) -> Result<Vec<RegistryAuth>, ApiErr> {
        let want: Vec<String> = servers.iter().map(|s| normalize_server(s)).collect();
        let mut rows = self.credential_records_for_host(rec).await?;
        rows.sort_by_key(|r| std::cmp::Reverse(r.credential.updated_ms));
        let mut out: Vec<RegistryAuth> = Vec::new();
        for r in rows {
            let server = normalize_server(&r.credential.server);
            if !want.is_empty() && !want.contains(&server) {
                continue;
            }
            if out.iter().any(|a| a.server == server) {
                continue;
            }
            let secret = self.cipher.decrypt(&r.secret_blob).map_err(|e| {
                internal(format!(
                    "registry credential {} cannot be decrypted: {e}",
                    r.credential.id
                ))
            })?;
            out.push(RegistryAuth {
                server,
                username: r.credential.username.clone(),
                secret,
            });
        }
        Ok(out)
    }
}

fn view(rows: Vec<RegistryCredentialRecord>) -> Vec<RegistryCredential> {
    let mut v: Vec<RegistryCredential> = rows.into_iter().map(|r| r.credential).collect();
    v.sort_by(|a, b| a.server.cmp(&b.server).then(a.username.cmp(&b.username)));
    v
}

fn validate_scope(scope: &Option<TargetSelector>) -> Result<(), ApiErr> {
    match scope {
        Some(TargetSelector::Hosts { hosts }) if hosts.iter().all(|h| h.trim().is_empty()) => {
            Err(bad("a hosts scope needs at least one host name"))
        }
        Some(TargetSelector::Site { sites }) if sites.iter().all(|s| s.trim().is_empty()) => {
            Err(bad("a site scope needs at least one site"))
        }
        Some(TargetSelector::Labels { labels }) if labels.is_empty() => {
            Err(bad("a labels scope needs at least one label"))
        }
        _ => Ok(()),
    }
}

fn scope_text(scope: &Option<TargetSelector>) -> String {
    match scope {
        None => "every host".into(),
        Some(TargetSelector::Hosts { hosts }) => format!("hosts {}", hosts.join(", ")),
        Some(TargetSelector::Site { sites }) => format!("site {}", sites.join(", ")),
        Some(TargetSelector::Labels { labels }) => format!(
            "labels {}",
            labels
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

pub(crate) async fn list(State(state): State<AppState>) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    let rows = a
        .store
        .list_registry_credentials()
        .await
        .map_err(store_err)?;
    Ok(Json(json!({ "credentials": view(rows) })))
}

pub(crate) async fn create(
    State(state): State<AppState>,
    me: CurrentUser,
    Json(input): Json<RegistryCredentialInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Templates)?;
    let server = normalize_server(input.server.as_deref().unwrap_or(""));
    if server.is_empty() {
        return Err(bad(
            "a registry is required — the host an image reference names, like registry.gitlab.com",
        ));
    }
    let username = input.username.unwrap_or_default().trim().to_string();
    if username.is_empty() {
        return Err(bad("a username is required"));
    }
    let secret = input.secret.unwrap_or_default();
    if secret.trim().is_empty() {
        return Err(bad("a token or password is required"));
    }
    let scope = input.scope.unwrap_or(None);
    validate_scope(&scope)?;
    let now = super::now_ms();
    let rec = RegistryCredentialRecord {
        credential: RegistryCredential {
            id: new_id("rc"),
            server: server.clone(),
            username: username.clone(),
            label: input
                .label
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty()),
            scope,
            created_ms: now,
            created_by: me.username.clone(),
            updated_ms: now,
        },
        secret_blob: a.cipher.encrypt(secret.trim()),
    };
    a.store
        .put_registry_credential(&rec)
        .await
        .map_err(store_err)?;
    a.audit(
        &me,
        "registry_credential.create",
        AuditTarget {
            registry: Some(server),
            user: Some(username),
            ..Default::default()
        },
        AuditResultKind::Ok,
        Some(format!("scope: {}", scope_text(&rec.credential.scope))),
        None,
    )
    .await;
    Ok(Json(json!({ "ok": true, "credential": rec.credential })))
}

pub(crate) async fn update(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(id): Path<String>,
    Json(input): Json<RegistryCredentialInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Templates)?;
    let mut rec = a
        .store
        .list_registry_credentials()
        .await
        .map_err(store_err)?
        .into_iter()
        .find(|r| r.credential.id == id)
        .ok_or_else(|| not_found("no such registry credential"))?;
    let mut changed: Vec<&str> = Vec::new();
    if let Some(s) = input.server {
        let server = normalize_server(&s);
        if server.is_empty() {
            return Err(bad("a registry is required"));
        }
        if server != rec.credential.server {
            rec.credential.server = server;
            changed.push("registry");
        }
    }
    if let Some(u) = input.username {
        let u = u.trim().to_string();
        if u.is_empty() {
            return Err(bad("a username is required"));
        }
        if u != rec.credential.username {
            rec.credential.username = u;
            changed.push("username");
        }
    }
    if let Some(sec) = input.secret.filter(|s| !s.trim().is_empty()) {
        rec.secret_blob = a.cipher.encrypt(sec.trim());
        changed.push("token");
    }
    if let Some(l) = input.label {
        rec.credential.label = Some(l.trim().to_string()).filter(|l| !l.is_empty());
        changed.push("label");
    }
    if let Some(scope) = input.scope {
        validate_scope(&scope)?;
        rec.credential.scope = scope;
        changed.push("scope");
    }
    rec.credential.updated_ms = super::now_ms();
    a.store
        .put_registry_credential(&rec)
        .await
        .map_err(store_err)?;
    a.audit(
        &me,
        "registry_credential.update",
        AuditTarget {
            registry: Some(rec.credential.server.clone()),
            user: Some(rec.credential.username.clone()),
            ..Default::default()
        },
        AuditResultKind::Ok,
        Some(if changed.is_empty() {
            "nothing changed".into()
        } else {
            format!("changed {}", changed.join(", "))
        }),
        None,
    )
    .await;
    Ok(Json(json!({ "ok": true, "credential": rec.credential })))
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Templates)?;
    let rec = a
        .store
        .list_registry_credentials()
        .await
        .map_err(store_err)?
        .into_iter()
        .find(|r| r.credential.id == id)
        .ok_or_else(|| not_found("no such registry credential"))?;
    a.store
        .delete_registry_credential(&id)
        .await
        .map_err(store_err)?;
    a.audit(
        &me,
        "registry_credential.delete",
        AuditTarget {
            registry: Some(rec.credential.server.clone()),
            user: Some(rec.credential.username.clone()),
            ..Default::default()
        },
        AuditResultKind::Ok,
        None,
        None,
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::{DeployHub, PlainCipher, SecretCipher};
    use photon_deploy::store::MemStore;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn deploy_api() -> DeployApi {
        DeployApi {
            store: Arc::new(MemStore::new()),
            cipher: Arc::new(PlainCipher),
            hub: Arc::new(DeployHub::new()),
            bootstrapper: None,
            agent_binaries_dir: None,
            ingest: None,
            latest_agent_version: None,
            server_origin: None,
            deployments: None,
        }
    }

    fn host(name: &str, site: Option<&str>, labels: &[(&str, &str)]) -> HostRecord {
        HostRecord {
            id: format!("mh_{name}"),
            host: name.into(),
            site: site.map(|s| s.into()),
            token_hash: "h".into(),
            requested_capabilities: vec![],
            reported_capabilities: vec![],
            agent_version: None,
            upgrade_to: None,
            poll_secs: 15,
            last_seen_ms: None,
            enrolled_ms: 0,
            docker: None,
            host_id: None,
            arch: None,
            os: None,
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            desired_generation: 0,
        }
    }

    fn rec(
        id: &str,
        server: &str,
        user: &str,
        scope: Option<TargetSelector>,
        updated_ms: i64,
    ) -> RegistryCredentialRecord {
        RegistryCredentialRecord {
            credential: RegistryCredential {
                id: id.into(),
                server: normalize_server(server),
                username: user.into(),
                label: None,
                scope,
                created_ms: 0,
                created_by: "admin".into(),
                updated_ms,
            },
            secret_blob: PlainCipher.encrypt(&format!("secret-{id}")),
        }
    }

    /// The agent gets exactly the credentials its host is scoped for, decrypted, one per
    /// registry with the newest winning, and narrowed to the registries it asked about.
    #[tokio::test]
    async fn auth_for_a_host_honours_scope_recency_and_the_asked_servers() {
        let a = deploy_api();
        let s = &a.store;
        s.put_registry_credential(&rec("old", "registry.gitlab.com", "old-user", None, 1))
            .await
            .unwrap();
        s.put_registry_credential(&rec(
            "new",
            "https://registry.gitlab.com/",
            "new-user",
            None,
            2,
        ))
        .await
        .unwrap();
        s.put_registry_credential(&rec(
            "site",
            "ghcr.io",
            "site-user",
            Some(TargetSelector::Site {
                sites: vec!["jakarta".into()],
            }),
            3,
        ))
        .await
        .unwrap();
        s.put_registry_credential(&rec(
            "lbl",
            "docker.io",
            "hub-user",
            Some(TargetSelector::Labels {
                labels: BTreeMap::from([("env".to_string(), "prod".to_string())]),
            }),
            4,
        ))
        .await
        .unwrap();

        let edge = host("edge-1", Some("jakarta"), &[("env", "dev")]);
        let auths = a.registry_auth_for(&edge, &[]).await.unwrap();
        let by: BTreeMap<&str, &RegistryAuth> =
            auths.iter().map(|x| (x.server.as_str(), x)).collect();
        assert_eq!(
            by.len(),
            2,
            "gitlab (newest of two) + ghcr (site) — not the prod-only hub one"
        );
        assert_eq!(by["registry.gitlab.com"].username, "new-user");
        assert_eq!(by["registry.gitlab.com"].secret, "secret-new");
        assert_eq!(by["ghcr.io"].username, "site-user");

        let narrowed = a
            .registry_auth_for(&edge, &["https://GHCR.io".to_string()])
            .await
            .unwrap();
        assert_eq!(narrowed.len(), 1);
        assert_eq!(narrowed[0].server, "ghcr.io");

        let prod = host("core-1", None, &[("env", "prod")]);
        let auths = a.registry_auth_for(&prod, &[]).await.unwrap();
        assert!(auths.iter().any(|x| x.server == "docker.io"));
        assert!(
            !auths.iter().any(|x| x.server == "ghcr.io"),
            "not in the site"
        );

        let public = a.credentials_for_host(&edge).await.unwrap();
        assert_eq!(public.len(), 3, "the view keeps both gitlab rows");
    }
}

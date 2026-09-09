//! One-shot lifecycle actions (R3): a command with a TTL, never desired state. Queued here,
//! picked up by the agent on its next poll (immediately while it is long-polling), executed at
//! most once, reported back on the following sync. An action nobody picked up within the lease
//! expires instead of firing on Monday. Every one is audited (R3.5).

use super::{api, bad, require, store_err, ApiErr, DeployApi};
use crate::auth::CurrentUser;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use photon_deploy::ids::new_id;
use photon_deploy::model::*;
use serde_json::{json, Value};

/// The lease: 15 minutes (a host offline for longer gets a fresh decision, not a stale one).
pub const ACTION_TTL_MS: i64 = 15 * 60_000;

impl DeployApi {
    /// Queue an action on a host. Validates the target against the last inventory so the
    /// operator gets "no such container" now rather than a failure minutes later.
    pub async fn create_action(
        &self,
        me: &CurrentUser,
        host: &str,
        input: &ActionInput,
    ) -> Result<ActionRecord, ApiErr> {
        let rec = self.host(host).await?;
        let caps = rec.capabilities();
        let target = AuditTarget {
            host: Some(host.to_string()),
            container: input.container_id.clone(),
            image: input.image.clone(),
            volume: input.volume.clone(),
            ..Default::default()
        };
        if !caps.contains(&AgentCapability::Lifecycle) {
            self.audit(
                me,
                &input.kind.audit_action(),
                target,
                AuditResultKind::Denied,
                Some("agent installed without --docker-lifecycle".into()),
                None,
            )
            .await;
            return Err(bad(
                "the agent on this host was installed without --docker-lifecycle",
            ));
        }
        let inv = self.inventory(host).await?;
        let mut container_name = None;
        let mut container_id = None;
        if input.kind.needs_container() {
            let id = input
                .container_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| bad("container_id is required"))?;
            let c = super::hub::find_container(&inv.containers, id)
                .ok_or_else(|| bad(format!("no container {id} on {host} in its last inventory")))?;
            container_name = Some(c.name.clone());
            container_id = Some(c.id.clone());
        }
        let mut image = input.image.clone().filter(|s| !s.is_empty());
        if matches!(input.kind, ActionKind::ImagePull | ActionKind::ImageRemove) {
            let want = image.clone().ok_or_else(|| bad("image is required"))?;
            let img = inv
                .images
                .iter()
                .find(|i| i.id == want || i.repo_tags.iter().any(|t| t == &want));
            match (input.kind, img) {
                (ActionKind::ImageRemove, None) => return Err(bad("no such image")),
                (ActionKind::ImageRemove, Some(i)) if i.used_by > 0 => {
                    return Err(bad(format!(
                        "{} is used by {} container{}",
                        i.repo_tags.first().cloned().unwrap_or(want),
                        i.used_by,
                        if i.used_by == 1 { "" } else { "s" }
                    )))
                }
                (ActionKind::ImagePull, Some(i))
                    if !i.repo_tags.is_empty() && !want.contains(':') && !want.contains('/') =>
                {
                    image = i.repo_tags.first().cloned();
                }
                _ => {}
            }
        }
        let mut volume = None;
        if input.kind == ActionKind::VolumeRemove {
            let want = input
                .volume
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| bad("volume is required"))?;
            let v =
                inv.volumes.iter().find(|v| v.name == want).ok_or_else(|| {
                    bad(format!("no volume {want} on {host} in its last inventory"))
                })?;
            // Docker refuses too, but "in use" now beats a failure on the next sync — and a
            // stopped container counts: its data is still that container's.
            if !v.used_by.is_empty() {
                return Err(bad(format!("{want} is used by {}", v.used_by.join(", "))));
            }
            volume = Some(v.name.clone());
        }
        // One open action per container at a time: a second restart while the first is queued
        // is a double-click, not a second decision.
        if let Some(cid) = &container_id {
            let open = self.store.open_actions(host).await.map_err(store_err)?;
            if let Some(a) = open
                .iter()
                .find(|a| a.container_id.as_deref() == Some(cid.as_str()))
            {
                return Err(super::err(
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "{} is already {} — wait for it to finish",
                        container_name.clone().unwrap_or_default(),
                        match a.status {
                            ActionStatus::Running => "running an action",
                            _ => "queued for an action",
                        }
                    ),
                ));
            }
        }
        let now = super::now_ms();
        let a = ActionRecord {
            id: new_id("ac"),
            host: host.to_string(),
            kind: input.kind,
            container_id,
            container_name: container_name.clone(),
            image: image.clone(),
            volume: volume.clone(),
            args: input.args.clone(),
            status: ActionStatus::Queued,
            actor: me.username.clone(),
            created_ms: now,
            started_ms: None,
            finished_ms: None,
            expires_ms: now + ACTION_TTL_MS,
            result: None,
        };
        self.store.put_action(&a).await.map_err(store_err)?;
        let online = rec.status(now) == AgentStatus::Online;
        self.audit(
            me,
            &input.kind.audit_action(),
            AuditTarget {
                host: Some(host.to_string()),
                container: container_name,
                image,
                volume,
                ..Default::default()
            },
            AuditResultKind::Pending,
            Some(if online {
                "queued; the agent picks it up on its next sync".into()
            } else {
                format!("queued while {host} is offline; expires in 15 minutes")
            }),
            Some(serde_json::to_vec(&a.args).unwrap_or_default().as_slice()),
        )
        .await;
        self.hub.wake(host);
        Ok(a)
    }
}

pub(crate) async fn create(
    State(state): State<AppState>,
    me: CurrentUser,
    Path(host): Path<String>,
    Json(input): Json<ActionInput>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Lifecycle)?;
    let rec = a.create_action(&me, &host, &input).await?;
    Ok(Json(json!({ "ok": true, "action": rec })))
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Path(host): Path<String>,
    Query(p): Query<super::inventory::LimitParams>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    let _ = a.host(&host).await?;
    let limit = p.limit.unwrap_or(20).clamp(1, 200);
    Ok(Json(
        json!({ "actions": a.store.list_actions(&host, limit).await.map_err(store_err)? }),
    ))
}

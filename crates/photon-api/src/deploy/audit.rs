//! `GET /api/audit` — every change Photon made to a host, newest first (R3.5 / R8.3). Readable by
//! every role: the question "who did what to customer X" must not need an admin.

use super::{api, require, store_err, ApiErr};
use crate::auth::CurrentUser;
use crate::AppState;
use axum::extract::{Query, State};
use axum::Json;
use photon_deploy::model::*;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Default, Deserialize)]
pub struct AuditParams {
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    me: CurrentUser,
    Query(p): Query<AuditParams>,
) -> Result<Json<Value>, ApiErr> {
    let a = api(&state)?;
    require(&me, Permission::Audit)?;
    let result = match p.result.as_deref() {
        None | Some("") => None,
        Some(r) => Some(
            AuditResultKind::parse(r)
                .ok_or_else(|| super::bad("result must be ok, failed, denied or pending"))?,
        ),
    };
    let f = AuditFilter {
        since_ms: p.since,
        actor: p.actor,
        action: p.action,
        host: p.host,
        result,
        q: p.q,
        limit: crate::query_params::clamp_limit(p.limit.unwrap_or(200)),
    };
    let (entries, total) = a.store.query_audit(&f).await.map_err(store_err)?;
    Ok(Json(json!({ "entries": entries, "total": total })))
}

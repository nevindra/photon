//! REST surface for the uptime vertical. All routes live behind the session-cookie
//! `protected` router. Handlers are thin: validate, call `UptimeStore`, and (on mutation)
//! emit a `SchedulerCommand` so the running scheduler reflects the change live.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use photon_uptime::model::*;
use photon_uptime::store::UptimeStore;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::AppState;

#[derive(Clone)]
pub struct UptimeApi {
    pub store: Arc<dyn UptimeStore>,
    pub cmd_tx: mpsc::Sender<SchedulerCommand>,
    pub retention_days: u32,
}

type ApiErr = (StatusCode, Json<serde_json::Value>);
fn err(code: StatusCode, msg: impl std::fmt::Display) -> ApiErr {
    (code, Json(json!({ "error": msg.to_string() })))
}
fn internal(e: impl std::fmt::Display) -> ApiErr {
    err(StatusCode::INTERNAL_SERVER_ERROR, e)
}

impl UptimeApi {
    pub async fn list(&self) -> Result<Vec<Monitor>, ApiErr> {
        self.store.list_monitors().await.map_err(internal)
    }
    pub async fn create(&self, input: MonitorInput) -> Result<Monitor, ApiErr> {
        let m = self.store.create_monitor(input).await.map_err(internal)?;
        let _ = self
            .cmd_tx
            .send(SchedulerCommand::Upsert(Box::new(m.clone())))
            .await;
        Ok(m)
    }
}

/// Pull the configured `UptimeApi` out of AppState or return 404 (subsystem disabled).
fn api(state: &AppState) -> Result<&UptimeApi, ApiErr> {
    state
        .uptime
        .as_ref()
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "uptime subsystem disabled"))
}

pub(crate) async fn list_monitors(State(s): State<AppState>) -> Result<Json<Vec<Monitor>>, ApiErr> {
    Ok(Json(api(&s)?.list().await?))
}
pub(crate) async fn create_monitor(
    State(s): State<AppState>,
    Json(input): Json<MonitorInput>,
) -> Result<Json<Monitor>, ApiErr> {
    Ok(Json(api(&s)?.create(input).await?))
}
pub(crate) async fn get_monitor(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Monitor>, ApiErr> {
    api(&s)?
        .store
        .get_monitor(&id)
        .await
        .map_err(internal)?
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such monitor"))
}
pub(crate) async fn update_monitor(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<MonitorInput>,
) -> Result<Json<Monitor>, ApiErr> {
    let a = api(&s)?;
    let m = a
        .store
        .update_monitor(&id, input)
        .await
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such monitor"))?;
    let _ = a
        .cmd_tx
        .send(SchedulerCommand::Upsert(Box::new(m.clone())))
        .await;
    Ok(Json(m))
}
pub(crate) async fn delete_monitor(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    let a = api(&s)?;
    if a.store.delete_monitor(&id).await.map_err(internal)? {
        let _ = a.cmd_tx.send(SchedulerCommand::Remove(id)).await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(err(StatusCode::NOT_FOUND, "no such monitor"))
    }
}
async fn set_enabled(s: &AppState, id: &str, enabled: bool) -> Result<Json<Monitor>, ApiErr> {
    let a = api(s)?;
    let m = a
        .store
        .set_enabled(id, enabled)
        .await
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such monitor"))?;
    let _ = a
        .cmd_tx
        .send(SchedulerCommand::Upsert(Box::new(m.clone())))
        .await;
    Ok(Json(m))
}
pub(crate) async fn pause_monitor(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Monitor>, ApiErr> {
    set_enabled(&s, &id, false).await
}
pub(crate) async fn resume_monitor(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Monitor>, ApiErr> {
    set_enabled(&s, &id, true).await
}

#[derive(Deserialize)]
pub struct WindowQuery {
    #[serde(default)]
    window: Option<String>,
}
fn window_since(window: &Option<String>) -> i64 {
    let ms = match window.as_deref() {
        Some("7d") => 7 * 86_400_000,
        Some("30d") => 30 * 86_400_000,
        _ => 86_400_000,
    };
    now_ms() - ms
}

pub(crate) async fn get_heartbeats(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<WindowQuery>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let a = api(&s)?;
    let since = window_since(&q.window);
    let hbs = a.store.heartbeats(&id, since).await.map_err(internal)?;
    let uptime = a.store.uptime_pct(&id, since).await.map_err(internal)?;
    Ok(Json(json!({ "heartbeats": hbs, "uptime_pct": uptime })))
}
pub(crate) async fn get_incidents(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Incident>>, ApiErr> {
    Ok(Json(
        api(&s)?.store.incidents(&id, 100).await.map_err(internal)?,
    ))
}

#[cfg(test)]
mod tests {
    use photon_uptime::model::SchedulerCommand;
    use photon_uptime::store::MemStore;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    // Smoke test: the UptimeApi wrapper wires a store + command channel and create→list works.
    #[tokio::test]
    async fn create_then_list_via_api_layer() {
        let store = Arc::new(MemStore::new());
        let (tx, mut rx) = mpsc::channel::<SchedulerCommand>(8);
        let api = super::UptimeApi {
            store: store.clone(),
            cmd_tx: tx,
            retention_days: 30,
        };

        let input = serde_json::from_str(r#"{"name":"api","type":"http","target":"https://x.test","interval_secs":30,"timeout_secs":5,"retries":2}"#).unwrap();
        let m = api.create(input).await.unwrap();
        assert_eq!(api.list().await.unwrap().len(), 1);
        // creating a monitor emits an Upsert command to the scheduler
        assert!(matches!(rx.try_recv().unwrap(), SchedulerCommand::Upsert(mm) if mm.id == m.id));
    }
}

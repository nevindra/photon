//! REST surface for the alerts vertical (the system-wide webhook alert & notification engine).
//! All routes live behind the session-cookie `protected` router. Handlers are thin: validate,
//! call `AlertStore`, and (on rule mutation) emit a `SchedulerCommand` so the running evaluation
//! loop reflects the change live — mirrors `uptime.rs`.
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use photon_alerts::model::*;
use photon_alerts::notify::{Notifier, WebhookNotifier};
use photon_alerts::source::ConditionSource;
use photon_alerts::store::AlertStore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::query_params::clamp_limit;
use crate::AppState;

#[derive(Clone)]
pub struct AlertsApi {
    pub store: Arc<dyn AlertStore>,
    pub cmd_tx: mpsc::Sender<SchedulerCommand>,
    pub source: Arc<dyn ConditionSource>,
}

type ApiErr = (StatusCode, Json<serde_json::Value>);
fn err(code: StatusCode, msg: impl std::fmt::Display) -> ApiErr {
    (code, Json(json!({ "error": msg.to_string() })))
}
fn internal(e: impl std::fmt::Display) -> ApiErr {
    err(StatusCode::INTERNAL_SERVER_ERROR, e)
}

impl AlertsApi {
    pub async fn list_rules(&self) -> Result<Vec<Rule>, ApiErr> {
        self.store.list_rules().await.map_err(internal)
    }
    pub async fn create_rule(&self, input: RuleInput) -> Result<Rule, ApiErr> {
        let r = self.store.create_rule(input).await.map_err(internal)?;
        let _ = self
            .cmd_tx
            .send(SchedulerCommand::Upsert(Box::new(r.clone())))
            .await;
        Ok(r)
    }
}

/// Pull the configured `AlertsApi` out of AppState or return 404 (subsystem disabled).
fn api(state: &AppState) -> Result<&AlertsApi, ApiErr> {
    state
        .alerts
        .as_ref()
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "alerts subsystem disabled"))
}

// ---------------------------------------------------------------------------
// rules
// ---------------------------------------------------------------------------

pub(crate) async fn list_rules(State(s): State<AppState>) -> Result<Json<Vec<Rule>>, ApiErr> {
    Ok(Json(api(&s)?.list_rules().await?))
}
pub(crate) async fn create_rule(
    State(s): State<AppState>,
    Json(input): Json<RuleInput>,
) -> Result<Json<Rule>, ApiErr> {
    Ok(Json(api(&s)?.create_rule(input).await?))
}
pub(crate) async fn get_rule(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Rule>, ApiErr> {
    api(&s)?
        .store
        .get_rule(&id)
        .await
        .map_err(internal)?
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such rule"))
}

/// Partial update body for `PATCH /alerts/rules/:id`: every field optional so the same route
/// covers both a full edit-dialog submit and the rules-table enable/pause toggle (`{"enabled":
/// false}` alone) — there's no dedicated pause/resume route like `/monitors/:id/pause`.
#[derive(Debug, Default, Deserialize)]
pub struct RulePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub for_secs: Option<i64>,
    #[serde(default)]
    pub interval_secs: Option<i64>,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub channel_ids: Option<Vec<ChannelId>>,
}

pub(crate) async fn update_rule(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<RulePatch>,
) -> Result<Json<Rule>, ApiErr> {
    let a = api(&s)?;
    let existing = a
        .store
        .get_rule(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such rule"))?;
    let input = RuleInput {
        name: patch.name.unwrap_or(existing.name),
        description: patch.description.or(existing.description),
        enabled: patch.enabled.unwrap_or(existing.enabled),
        condition: patch.condition.unwrap_or(existing.condition),
        for_secs: patch.for_secs.unwrap_or(existing.for_secs),
        interval_secs: patch.interval_secs.unwrap_or(existing.interval_secs),
        severity: patch.severity.unwrap_or(existing.severity),
        channel_ids: patch.channel_ids.unwrap_or(existing.channel_ids),
    };
    let r = a
        .store
        .update_rule(&id, input)
        .await
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such rule"))?;
    let _ = a
        .cmd_tx
        .send(SchedulerCommand::Upsert(Box::new(r.clone())))
        .await;
    Ok(Json(r))
}

pub(crate) async fn delete_rule(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    let a = api(&s)?;
    if a.store.delete_rule(&id).await.map_err(internal)? {
        let _ = a.cmd_tx.send(SchedulerCommand::Remove(id)).await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(err(StatusCode::NOT_FOUND, "no such rule"))
    }
}

// ---------------------------------------------------------------------------
// preview / test — dry-run a condition against live data
// ---------------------------------------------------------------------------

/// One evaluated series in a preview/test response: its label key, current value, and whether it
/// currently breaches the condition's `cmp`/`threshold`.
#[derive(Serialize)]
pub struct PreviewSeries {
    series_key: serde_json::Value,
    value: f64,
    breaching: bool,
}
#[derive(Serialize)]
pub struct PreviewResult {
    series: Vec<PreviewSeries>,
}

async fn sample_condition(a: &AlertsApi, cond: &Condition) -> Result<Json<PreviewResult>, ApiErr> {
    let samples = a.source.sample(cond, now_ms()).await.map_err(internal)?;
    let cmp = cond.cmp();
    let threshold = cond.threshold();
    let series = samples
        .into_iter()
        .map(|s| PreviewSeries {
            series_key: s.labels_json(),
            breaching: cmp.test(s.value, threshold),
            value: s.value,
        })
        .collect();
    Ok(Json(PreviewResult { series }))
}

/// `POST /alerts/preview` — dry-run a draft `Condition` body (not yet a saved rule) against live
/// data, powering the "will trigger on N series now" preview in the create/edit dialog.
pub(crate) async fn preview(
    State(s): State<AppState>,
    Json(cond): Json<Condition>,
) -> Result<Json<PreviewResult>, ApiErr> {
    sample_condition(api(&s)?, &cond).await
}

/// `POST /alerts/rules/:id/test` — evaluate an already-saved rule's condition right now.
pub(crate) async fn test_rule(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PreviewResult>, ApiErr> {
    let a = api(&s)?;
    let rule = a
        .store
        .get_rule(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such rule"))?;
    sample_condition(a, &rule.condition).await
}

// ---------------------------------------------------------------------------
// channels
// ---------------------------------------------------------------------------

pub(crate) async fn list_channels(State(s): State<AppState>) -> Result<Json<Vec<Channel>>, ApiErr> {
    Ok(Json(
        api(&s)?.store.list_channels().await.map_err(internal)?,
    ))
}
pub(crate) async fn create_channel(
    State(s): State<AppState>,
    Json(input): Json<ChannelInput>,
) -> Result<Json<Channel>, ApiErr> {
    Ok(Json(
        api(&s)?
            .store
            .create_channel(input)
            .await
            .map_err(internal)?,
    ))
}
pub(crate) async fn get_channel(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Channel>, ApiErr> {
    api(&s)?
        .store
        .get_channel(&id)
        .await
        .map_err(internal)?
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such channel"))
}

/// Partial update body for `PATCH /alerts/channels/:id` — same rationale as [`RulePatch`].
#[derive(Debug, Default, Deserialize)]
pub struct ChannelPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<ChannelKind>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
}

pub(crate) async fn update_channel(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<ChannelPatch>,
) -> Result<Json<Channel>, ApiErr> {
    let a = api(&s)?;
    let existing = a
        .store
        .get_channel(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such channel"))?;
    let input = ChannelInput {
        name: patch.name.unwrap_or(existing.name),
        kind: patch.kind.unwrap_or(existing.kind),
        url: patch.url.unwrap_or(existing.url),
        secret: patch.secret.or(existing.secret),
        headers: patch.headers.or(existing.headers),
    };
    let ch = a
        .store
        .update_channel(&id, input)
        .await
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such channel"))?;
    Ok(Json(ch))
}

pub(crate) async fn delete_channel(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    if api(&s)?.store.delete_channel(&id).await.map_err(internal)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(err(StatusCode::NOT_FOUND, "no such channel"))
    }
}

/// `POST /alerts/channels/:id/test` — send a sample webhook to this channel via a throwaway
/// `WebhookNotifier` (fire-and-forget, exactly like a real triggered/resolved delivery).
pub(crate) async fn test_channel(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let a = api(&s)?;
    let channel = a
        .store
        .get_channel(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such channel"))?;
    let now = now_ms();
    let payload = json!({
        "status": "triggered",
        "rule": { "id": "test", "name": "Test notification", "severity": "info", "signal": "test" },
        "series": {},
        "condition": "This is a test notification from Photon",
        "value": 1.0, "threshold": 1.0,
        "started_at": now, "at": now, "incident_id": 0,
    });
    WebhookNotifier::new().deliver(&channel, payload).await;
    Ok(Json(json!({ "delivered": true })))
}

// ---------------------------------------------------------------------------
// incidents
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct IncidentsParams {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    rule_id: Option<String>,
    #[serde(default = "default_incidents_limit")]
    limit: usize,
}
fn default_incidents_limit() -> usize {
    100
}

pub(crate) async fn list_incidents(
    State(s): State<AppState>,
    Query(q): Query<IncidentsParams>,
) -> Result<Json<Vec<Incident>>, ApiErr> {
    let a = api(&s)?;
    let limit = clamp_limit(q.limit) as u32;
    Ok(Json(
        a.store
            .list_incidents(q.status.as_deref(), q.rule_id.as_deref(), limit)
            .await
            .map_err(internal)?,
    ))
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use photon_alerts::model::{Condition, SchedulerCommand, SeriesSample};
    use photon_alerts::source::ConditionSource;
    use photon_alerts::store::mem::MemStore;
    use photon_core::PhotonError;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tower::ServiceExt; // for `oneshot`

    /// A `ConditionSource` fake that never actually samples anything — this test only exercises
    /// the CRUD path, so `sample` is never called.
    struct FakeSource;
    #[async_trait]
    impl ConditionSource for FakeSource {
        async fn sample(
            &self,
            _cond: &Condition,
            _now_ms: i64,
        ) -> Result<Vec<SeriesSample>, PhotonError> {
            Ok(vec![])
        }
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    // Smoke test: the AlertsApi wrapper wires a store + command channel into the router and a
    // create→list roundtrip works end to end through the HTTP layer (session-authed, like every
    // other `/api/alerts/*` route).
    #[tokio::test]
    async fn create_then_list_via_api_layer() {
        let store = Arc::new(MemStore::new());
        let (tx, mut rx) = mpsc::channel::<SchedulerCommand>(8);
        let alerts = super::AlertsApi {
            store: store.clone(),
            cmd_tx: tx,
            source: Arc::new(FakeSource),
        };
        let app = crate::test_server().with_alerts(Some(alerts)).into_router();
        let cookie = crate::session_cookie(&app).await;

        let body = serde_json::json!({
            "name": "high cpu",
            "condition": {
                "signal": "metrics", "metric_name": "system.cpu.utilization",
                "agg": "avg", "window_secs": 300, "cmp": "gt", "threshold": 0.9
            },
        })
        .to_string();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/alerts/rules")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let created = body_json(resp).await;
        assert_eq!(created["name"], "high cpu");
        // creating a rule emits an Upsert command to the scheduler
        let created_id = created["id"].as_str().unwrap().to_string();
        assert!(
            matches!(rx.try_recv().unwrap(), SchedulerCommand::Upsert(r) if r.id == created_id)
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/alerts/rules")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let rules = body_json(resp).await;
        assert_eq!(rules.as_array().unwrap().len(), 1);
    }
}

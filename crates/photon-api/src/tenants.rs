//! Tenant registry: the customer Photon installs allowed to federate into this central node
//! (name + minted bearer token + optional UI link-out), persisted in the shared control-plane
//! SQLite database (`[storage].db_path`). Mirrors the `rum_apps.rs` store pattern exactly — a
//! single `Mutex<Connection>` (low-volume OLTP), WAL mode, `CREATE TABLE IF NOT EXISTS` on open,
//! and an in-memory variant for tests. Errors map to `PhotonError::Io` (SQLite access is I/O) —
//! `PhotonError` is never edited, per project convention.

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use photon_core::metric_agg::Agg;
use photon_core::query::{
    MetricFieldResolver, MetricResolvedKind, MetricResolvedQuery, MetricResolvedTerm,
};
use photon_core::{PhotonError, TenantTokenMap};
use photon_query::{MetricSeriesRequest, MetricsQueryEngine, SeriesPoint};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Mutex;

use crate::AppState;

/// One registered tenant. `name` is the tenant's stable identity (immutable, `[a-z0-9-]{1,64}`);
/// `token` is the bearer token minted for its federation pusher/tee.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tenant {
    pub name: String,
    pub token: String,
    pub ui_url: Option<String>,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// Pure field validation, shared by the create API handler (surfaced as `400`). Uniqueness is
/// enforced separately by the SQLite `PRIMARY KEY`/`UNIQUE` constraints.
pub fn validate_tenant_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("name must be 1-64 characters".into());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("name must match [a-z0-9-]{1,64}".into());
    }
    // `/api/tenants/summary` (GET) shadows `/api/tenants/:name` in the router, so a tenant
    // literally named `summary` could never be PATCHed or DELETEd.
    if name == "summary" {
        return Err("\"summary\" is a reserved name".into());
    }
    Ok(())
}

/// Mint a new bearer token for a tenant.
pub fn mint_tenant_token() -> String {
    format!("tk_tenant_{}", uuid::Uuid::new_v4().simple())
}

/// Persistence boundary for tenants. Async so handlers can `.await` it uniformly; the SQLite impl
/// is synchronous under the `Mutex`.
#[async_trait]
pub trait TenantStore: Send + Sync {
    /// All tenants, sorted by name ascending.
    async fn list(&self) -> Result<Vec<Tenant>, PhotonError>;
    /// Insert a tenant. Errors on a duplicate name (PRIMARY KEY) or token (UNIQUE).
    async fn create(&self, t: &Tenant) -> Result<(), PhotonError>;
    /// Overwrite a tenant's `ui_url`, keyed by name. `false` if name absent.
    async fn update(&self, name: &str, ui_url: Option<&str>) -> Result<bool, PhotonError>;
    /// Replace a tenant's token, keyed by name. `false` if name absent.
    async fn rotate_token(&self, name: &str, new_token: &str) -> Result<bool, PhotonError>;
    /// Delete a tenant by name. `true` if a row was removed.
    async fn delete(&self, name: &str) -> Result<bool, PhotonError>;
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tenants (
    name       TEXT PRIMARY KEY,
    token      TEXT NOT NULL UNIQUE,
    ui_url     TEXT,
    created_at INTEGER NOT NULL
);
"#;

pub struct SqliteTenantStore {
    conn: Mutex<Connection>,
}

impl SqliteTenantStore {
    /// Open (creating parent dirs + file if needed) the shared control-plane DB and ensure the
    /// `tenants` table exists. Safe to call alongside the user/rum_app/uptime stores opening the
    /// same file: WAL mode allows concurrent readers + a single writer, and `CREATE TABLE IF NOT
    /// EXISTS` is idempotent.
    pub fn open(path: &str) -> Result<Self, PhotonError> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
        }
        let conn = Connection::open(path).map_err(err)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(err)?;
        Self::from_conn(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, PhotonError> {
        Self::from_conn(Connection::open_in_memory().map_err(err)?)
    }

    fn from_conn(conn: Connection) -> Result<Self, PhotonError> {
        conn.execute_batch(SCHEMA).map_err(err)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

fn row_to_tenant(r: &rusqlite::Row) -> rusqlite::Result<Tenant> {
    Ok(Tenant {
        name: r.get(0)?,
        token: r.get(1)?,
        ui_url: r.get(2)?,
        created_at: r.get(3)?,
    })
}

#[async_trait]
impl TenantStore for SqliteTenantStore {
    async fn list(&self) -> Result<Vec<Tenant>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT name,token,ui_url,created_at FROM tenants ORDER BY name")
            .map_err(err)?;
        let rows = stmt.query_map([], row_to_tenant).map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }

    async fn create(&self, t: &Tenant) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO tenants (name,token,ui_url,created_at) VALUES (?1,?2,?3,?4)",
            params![t.name, t.token, t.ui_url, t.created_at],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn update(&self, name: &str, ui_url: Option<&str>) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute(
                "UPDATE tenants SET ui_url=?2 WHERE name=?1",
                params![name, ui_url],
            )
            .map_err(err)?;
        Ok(n > 0)
    }

    async fn rotate_token(&self, name: &str, new_token: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute(
                "UPDATE tenants SET token=?2 WHERE name=?1",
                params![name, new_token],
            )
            .map_err(err)?;
        Ok(n > 0)
    }

    async fn delete(&self, name: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute("DELETE FROM tenants WHERE name=?1", params![name])
            .map_err(err)?;
        Ok(n > 0)
    }
}

// ---------------------------------------------------------------------------
// TenantApi: the `AppState`-facing handle over a `TenantStore` plus the live, ingest-shared
// `TenantTokenMap`. Mirrors `RumApi`'s store + live-cache-rebuilt-on-mutation shape (`rum.rs`).
// ---------------------------------------------------------------------------

/// The tenant registry attached to `AppState`: a store-backed registry of federated tenants,
/// plus the `token -> tenant name` map shared live with `photon-ingest`'s auth resolution
/// (`Task 2`'s `TenantTokenMap`). `None` on `AppState` disables `/api/tenants*` (404).
#[derive(Clone)]
pub struct TenantApi {
    store: std::sync::Arc<dyn TenantStore>,
    tokens: TenantTokenMap,
}

impl TenantApi {
    /// Build the registry over a durable store, loading the initial token map from it. The
    /// initial load's failure is propagated (a central node can't safely start ingest auth with
    /// an unknown token set); subsequent reloads (`reload_tokens`) are fail-safe instead.
    pub async fn new(
        store: std::sync::Arc<dyn TenantStore>,
        tokens: TenantTokenMap,
    ) -> Result<Self, PhotonError> {
        let api = TenantApi { store, tokens };
        api.load_tokens().await?;
        Ok(api)
    }

    async fn load_tokens(&self) -> Result<(), PhotonError> {
        let list = self.store.list().await?;
        let map = list.into_iter().map(|t| (t.token, t.name)).collect();
        *self.tokens.write().unwrap_or_else(|e| e.into_inner()) = map;
        Ok(())
    }

    /// Re-read every tenant from the store and replace the live token map. Called after each
    /// mutation. A store read failure leaves the previous map intact (fail-safe), exactly
    /// `RumApi::reload_cache`.
    async fn reload_tokens(&self) {
        let _ = self.load_tokens().await;
    }

    pub async fn list(&self) -> Result<Vec<Tenant>, PhotonError> {
        self.store.list().await
    }

    pub async fn create(&self, t: &Tenant) -> Result<(), PhotonError> {
        self.store.create(t).await?;
        self.reload_tokens().await;
        Ok(())
    }

    pub async fn update(&self, name: &str, ui_url: Option<&str>) -> Result<bool, PhotonError> {
        self.store.update(name, ui_url).await
    }

    pub async fn rotate_token(&self, name: &str, new_token: &str) -> Result<bool, PhotonError> {
        let ok = self.store.rotate_token(name, new_token).await?;
        if ok {
            self.reload_tokens().await;
        }
        Ok(ok)
    }

    pub async fn delete(&self, name: &str) -> Result<bool, PhotonError> {
        let ok = self.store.delete(name).await?;
        if ok {
            self.reload_tokens().await;
        }
        Ok(ok)
    }
}

// ---------------------------------------------------------------------------
// Session-authed handlers (`GET/POST /api/tenants`, `PATCH/DELETE /api/tenants/:name`,
// `POST /api/tenants/:name/rotate-token`). Registered in `lib.rs`'s `protected` router. The
// token is a secret: `list` redacts it to its last 4 characters; the full value is only ever
// returned once, in the `create`/`rotate-token` response bodies (mirrors the RUM minted-key flow
// in `rum.rs`).
// ---------------------------------------------------------------------------

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn bad_request(msg: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": msg.into() })),
    )
        .into_response()
}

fn err_500(e: PhotonError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

/// `ui_url` renders as a raw `href`/`window.open` target in the UI (Vue does not sanitize bound
/// `:href`), so a stored `javascript:` URL would execute in the admin's session — only http(s)
/// passes. Empty/whitespace normalizes to `None` (clears the link).
fn normalize_ui_url(raw: Option<&str>) -> Result<Option<String>, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(u) if u.starts_with("http://") || u.starts_with("https://") => Ok(Some(u.to_string())),
        Some(_) => Err("ui_url must start with http:// or https://".to_string()),
    }
}

/// `"…abcd"` — the last 4 characters of a secret token, for listing without exposing it.
fn redact_token(token: &str) -> String {
    let tail = if token.len() > 4 {
        &token[token.len() - 4..]
    } else {
        token
    };
    format!("…{tail}")
}

fn tenant_json(t: &Tenant) -> Value {
    json!({
        "name": t.name,
        "token": t.token,
        "ui_url": t.ui_url,
        "created_at": t.created_at,
    })
}

fn tenant_json_redacted(t: &Tenant) -> Value {
    json!({
        "name": t.name,
        "token": redact_token(&t.token),
        "ui_url": t.ui_url,
        "created_at": t.created_at,
    })
}

/// `GET /api/tenants` — registered tenants, token redacted.
pub(crate) async fn list_tenants(State(st): State<AppState>) -> Response {
    let Some(api) = st.tenants.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match api.list().await {
        Ok(list) => Json(json!({
            "tenants": list.iter().map(tenant_json_redacted).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => err_500(e),
    }
}

#[derive(Deserialize)]
pub(crate) struct CreateTenantBody {
    name: String,
    #[serde(default)]
    ui_url: Option<String>,
}

/// `POST /api/tenants` — register a new tenant. Server mints the token. 201 with the full
/// record (the only time the token is returned unredacted besides rotate); 400 invalid name;
/// 409 duplicate name.
pub(crate) async fn create_tenant(
    State(st): State<AppState>,
    Json(body): Json<CreateTenantBody>,
) -> Response {
    let Some(api) = st.tenants.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let name = body.name.trim().to_string();
    if let Err(e) = validate_tenant_name(&name) {
        return bad_request(e);
    }
    let ui_url = match normalize_ui_url(body.ui_url.as_deref()) {
        Ok(u) => u,
        Err(e) => return bad_request(e),
    };
    let tenant = Tenant {
        name,
        token: mint_tenant_token(),
        ui_url,
        created_at: now_ms(),
    };
    match api.create(&tenant).await {
        Ok(()) => (StatusCode::CREATED, Json(tenant_json(&tenant))).into_response(),
        Err(e) => {
            // A racing create (or the PK/UNIQUE constraint) can fail the insert after any
            // pre-check — surface a name collision as 409, not 500.
            match api.list().await {
                Ok(list) if list.iter().any(|t| t.name == tenant.name) => (
                    StatusCode::CONFLICT,
                    Json(json!({ "error": "a tenant with that name already exists" })),
                )
                    .into_response(),
                _ => err_500(e),
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct UpdateTenantBody {
    ui_url: Option<String>,
}

/// `PATCH /api/tenants/:name` — update the link-out `ui_url` (name + token unchanged). 200 with
/// the redacted record; 404 unknown.
pub(crate) async fn update_tenant(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<UpdateTenantBody>,
) -> Response {
    let Some(api) = st.tenants.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ui_url = match normalize_ui_url(body.ui_url.as_deref()) {
        Ok(u) => u,
        Err(e) => return bad_request(e),
    };
    match api.update(&name, ui_url.as_deref()).await {
        Ok(true) => match api.list().await {
            Ok(list) => match list.into_iter().find(|t| t.name == name) {
                Some(t) => (StatusCode::OK, Json(tenant_json_redacted(&t))).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            },
            Err(e) => err_500(e),
        },
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err_500(e),
    }
}

/// `POST /api/tenants/:name/rotate-token` — mint a fresh token (old token stops working
/// immediately, live in the shared `TenantTokenMap`). 200 `{token}`; 404 unknown.
pub(crate) async fn rotate_tenant_token(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let Some(api) = st.tenants.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let new_token = mint_tenant_token();
    match api.rotate_token(&name, &new_token).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "token": new_token }))).into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err_500(e),
    }
}

/// `DELETE /api/tenants/:name` — unregister a tenant (its token is removed from the shared map).
/// 204; 404 unknown.
pub(crate) async fn delete_tenant(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let Some(api) = st.tenants.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match api.delete(&name).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err_500(e),
    }
}

// ---------------------------------------------------------------------------
// `GET /api/tenants/summary` — the curated Home-board endpoint: one round trip over the four
// `photon.federation.*` metrics per tenant instead of N `/api/metrics/query` calls from the UI.
// ---------------------------------------------------------------------------

/// Curated per-tenant health snapshot for the Home tenant board.
#[derive(Debug, Clone, Serialize)]
pub struct TenantSummary {
    pub name: String,
    /// From the `mode` attr of `photon.federation.up`; `None` = never reported (in this window).
    pub mode: Option<String>,
    /// `"up" | "stale" | "down"`.
    pub status: String,
    /// Unix milliseconds; `0` = never.
    pub last_seen_ms: i64,
    /// Differenced from `photon.federation.ingest.rows` (summed across signals) over the window.
    pub ingest_rows_per_sec: f64,
    /// Latest `photon.federation.incidents.open`.
    pub open_incidents: f64,
    /// Sum of the latest `photon.federation.disk.hot_bytes` across signals.
    pub hot_bytes: f64,
    pub ui_url: Option<String>,
    /// `(ms, rows/sec)` points for the card sparkline.
    pub spark: Vec<(i64, f64)>,
}

/// Central doesn't know each tenant's configured `interval_secs`, so staleness is a fixed
/// threshold rather than a multiple of it — revisit if per-tenant configurable intervals land.
const SUMMARY_FRESH_AFTER_SECS: i64 = 120;
const SUMMARY_STALE_AFTER_SECS: i64 = 600;
const SUMMARY_WINDOW_SECS: i64 = 15 * 60;
const SUMMARY_BUCKETS: usize = 30;

/// `tenant:"{name}"` as a resolved metrics filter, built directly (no grammar round trip needed —
/// tenant names are always `[a-z0-9-]{1,64}`, so resolution can't fail).
pub(crate) fn tenant_metric_filter(name: &str, promoted: &[String]) -> MetricResolvedQuery {
    let field = MetricFieldResolver::new(promoted)
        .resolve_field_name("tenant")
        .expect("resolve_field_name never errors on a plain label name");
    MetricResolvedQuery {
        terms: vec![MetricResolvedTerm {
            negated: false,
            kind: MetricResolvedKind::Match {
                field,
                values: vec![name.to_string()],
            },
        }],
    }
}

/// The latest non-null value of a metric over the window (group_by=[] so at most one series).
async fn latest_value(
    engine: &MetricsQueryEngine,
    metric: &str,
    agg: Agg,
    filter: &MetricResolvedQuery,
    start: i64,
    end: i64,
) -> f64 {
    let req = MetricSeriesRequest {
        metric: metric.to_string(),
        agg: Some(agg),
        group_by: vec![],
        filter: Some(filter.clone()),
        start_ts_nanos: start,
        end_ts_nanos: end,
        buckets: SUMMARY_BUCKETS,
    };
    let series = engine
        .query_series(req)
        .await
        .map(|r| r.series)
        .unwrap_or_default();
    series
        .first()
        .and_then(|s| s.points.iter().rev().find_map(|p| p.v))
        .unwrap_or(0.0)
}

/// Headline rows/sec (last-minus-first of the cumulative sum, divided by the ACTUAL elapsed
/// time between those points — a tenant registered 2 minutes ago must not be diluted by the full
/// 15-minute window — clamped at 0 so a counter reset never reads as negative throughput) + a
/// sparkline of the bucket-to-bucket rate.
fn rate_and_spark(points: &[SeriesPoint]) -> (f64, Vec<(i64, f64)>) {
    let present: Vec<&SeriesPoint> = points.iter().filter(|p| p.v.is_some()).collect();
    let ingest_rows_per_sec = match (present.first(), present.last()) {
        (Some(a), Some(b)) if b.t > a.t => {
            ((b.v.unwrap() - a.v.unwrap()) / ((b.t - a.t) as f64 / 1e9)).max(0.0)
        }
        _ => 0.0,
    };
    let spark = present
        .windows(2)
        .filter_map(|w| {
            let dt_secs = (w[1].t - w[0].t) as f64 / 1e9;
            if dt_secs <= 0.0 {
                return None;
            }
            let rate = ((w[1].v.unwrap() - w[0].v.unwrap()) / dt_secs).max(0.0);
            Some((w[1].t / 1_000_000, rate))
        })
        .collect();
    (ingest_rows_per_sec, spark)
}

async fn build_tenant_summary(
    engine: &MetricsQueryEngine,
    tenant: &Tenant,
    promoted: &[String],
    now_ms: i64,
) -> TenantSummary {
    let end = now_ms * 1_000_000;
    let start = end - SUMMARY_WINDOW_SECS * 1_000_000_000;
    let filter = tenant_metric_filter(&tenant.name, promoted);

    let up_req = MetricSeriesRequest {
        metric: "photon.federation.up".to_string(),
        agg: None,
        group_by: vec!["mode".to_string()],
        filter: Some(filter.clone()),
        start_ts_nanos: start,
        end_ts_nanos: end,
        buckets: SUMMARY_BUCKETS,
    };
    let up_series = engine
        .query_series(up_req)
        .await
        .map(|r| r.series)
        .unwrap_or_default();
    let mut last_seen_ns = 0i64;
    let mut mode = None;
    for s in &up_series {
        for p in &s.points {
            if p.v.is_some() && p.t > last_seen_ns {
                last_seen_ns = p.t;
                mode = s.labels.get("mode").cloned();
            }
        }
    }
    let last_seen_ms = if last_seen_ns > 0 {
        last_seen_ns / 1_000_000
    } else {
        0
    };
    let age_secs = if last_seen_ms > 0 {
        (now_ms - last_seen_ms) / 1000
    } else {
        i64::MAX
    };
    let status = if age_secs <= SUMMARY_FRESH_AFTER_SECS {
        "up"
    } else if age_secs <= SUMMARY_STALE_AFTER_SECS {
        "stale"
    } else {
        "down"
    };

    let rows_req = MetricSeriesRequest {
        metric: "photon.federation.ingest.rows".to_string(),
        agg: Some(Agg::Sum),
        group_by: vec![],
        filter: Some(filter.clone()),
        start_ts_nanos: start,
        end_ts_nanos: end,
        buckets: SUMMARY_BUCKETS,
    };
    let rows_series = engine
        .query_series(rows_req)
        .await
        .map(|r| r.series)
        .unwrap_or_default();
    let (ingest_rows_per_sec, spark) = rows_series
        .first()
        .map(|s| rate_and_spark(&s.points))
        .unwrap_or_default();

    let open_incidents = latest_value(
        engine,
        "photon.federation.incidents.open",
        Agg::Last,
        &filter,
        start,
        end,
    )
    .await;
    let hot_bytes = latest_value(
        engine,
        "photon.federation.disk.hot_bytes",
        Agg::Sum,
        &filter,
        start,
        end,
    )
    .await;

    TenantSummary {
        name: tenant.name.clone(),
        mode,
        status: status.to_string(),
        last_seen_ms,
        ingest_rows_per_sec,
        open_incidents,
        hot_bytes,
        ui_url: tenant.ui_url.clone(),
        spark,
    }
}

/// `GET /api/tenants/summary` — curated per-tenant health for the Home board.
pub(crate) async fn summary(State(st): State<AppState>) -> Response {
    let Some(api) = st.tenants.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let list = match api.list().await {
        Ok(l) => l,
        Err(e) => return err_500(e),
    };
    let promoted = st.metrics_query.promoted_attributes().to_vec();
    let now = now_ms();
    // 4 engine queries per tenant — run tenants concurrently instead of 4N serial awaits.
    // ponytail: still 4 queries/tenant; fold into one group_by=["tenant"] pass if N grows.
    let out = futures::future::join_all(
        list.iter()
            .map(|t| build_tenant_summary(&st.metrics_query, t, &promoted, now)),
    )
    .await;
    Json(out).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(name: &str, token: &str) -> Tenant {
        Tenant {
            name: name.into(),
            token: token.into(),
            ui_url: None,
            created_at: 0,
        }
    }

    #[test]
    fn rate_uses_actual_elapsed_time_not_the_fixed_window() {
        // 1200 rows over 120s = 10/s; dividing by the fixed 900s window would report 1.33/s.
        let points = vec![
            SeriesPoint {
                t: 0,
                v: Some(100.0),
            },
            SeriesPoint {
                t: 120_000_000_000,
                v: Some(1300.0),
            },
        ];
        let (rate, _) = rate_and_spark(&points);
        assert!((rate - 10.0).abs() < 1e-9, "got {rate}");
    }

    #[test]
    fn ui_url_only_accepts_http_schemes() {
        assert_eq!(
            normalize_ui_url(Some("https://t.example"))
                .unwrap()
                .as_deref(),
            Some("https://t.example")
        );
        assert_eq!(normalize_ui_url(Some("   ")).unwrap(), None);
        assert_eq!(normalize_ui_url(None).unwrap(), None);
        assert!(normalize_ui_url(Some("javascript:alert(1)")).is_err());
        assert!(normalize_ui_url(Some("ftp://x")).is_err());
    }

    #[tokio::test]
    async fn crud_round_trip_in_memory() {
        let store = SqliteTenantStore::open_in_memory().unwrap();
        assert!(store.list().await.unwrap().is_empty());

        store.create(&tenant("acme", "tk_1")).await.unwrap();
        store.create(&tenant("initech", "tk_2")).await.unwrap();

        // list() is sorted by name.
        let names: Vec<String> = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["acme".to_string(), "initech".to_string()]);

        assert!(store
            .update("acme", Some("https://acme.example.com"))
            .await
            .unwrap());
        let acme = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.name == "acme")
            .unwrap();
        assert_eq!(acme.ui_url, Some("https://acme.example.com".to_string()));
        assert_eq!(acme.token, "tk_1"); // update does not touch the token

        assert!(store.rotate_token("acme", "tk_1_new").await.unwrap());
        let acme = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.name == "acme")
            .unwrap();
        assert_eq!(acme.token, "tk_1_new");
        assert!(!store.rotate_token("nope", "tk_x").await.unwrap()); // unknown name -> false

        assert!(store.delete("acme").await.unwrap());
        assert!(!store.delete("acme").await.unwrap()); // already gone
        assert_eq!(store.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn create_rejects_duplicate_name_and_token() {
        let store = SqliteTenantStore::open_in_memory().unwrap();
        store.create(&tenant("acme", "tk_1")).await.unwrap();
        assert!(
            store.create(&tenant("acme", "tk_2")).await.is_err(),
            "duplicate name (PRIMARY KEY)"
        );
        assert!(
            store.create(&tenant("other", "tk_1")).await.is_err(),
            "duplicate token (UNIQUE)"
        );
    }

    #[tokio::test]
    async fn open_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("photon.db");
        let p = path.to_str().unwrap();
        {
            let store = SqliteTenantStore::open(p).unwrap();
            store.create(&tenant("acme", "tk_1")).await.unwrap();
        }
        let store = SqliteTenantStore::open(p).unwrap();
        let tenants = store.list().await.unwrap();
        assert_eq!(tenants.len(), 1);
        assert_eq!(tenants[0].token, "tk_1");
    }

    #[test]
    fn validate_rejects_bad_names() {
        assert!(validate_tenant_name("acme").is_ok());
        assert!(validate_tenant_name("acme-01").is_ok());
        assert!(validate_tenant_name("").is_err(), "empty");
        assert!(validate_tenant_name("Acme").is_err(), "uppercase");
        assert!(validate_tenant_name("ac me").is_err(), "spaces");
        assert!(validate_tenant_name(&"a".repeat(65)).is_err(), "too long");
        assert!(
            validate_tenant_name("summary").is_err(),
            "reserved: shadowed by GET /api/tenants/summary"
        );
    }

    #[test]
    fn mint_tenant_token_is_prefixed_and_unique() {
        let a = mint_tenant_token();
        let b = mint_tenant_token();
        assert!(a.starts_with("tk_tenant_"));
        assert!(b.starts_with("tk_tenant_"));
        assert_ne!(a, b);
    }

    // ---- TenantApi handlers (`/api/tenants*`) ---------------------------------------------

    /// A `TenantApi` over an in-memory store, sharing its token map with the caller so tests
    /// can assert on live ingest-auth resolution the same way `photon-server` wires it.
    async fn api_with(tokens: TenantTokenMap) -> TenantApi {
        let store = SqliteTenantStore::open_in_memory().unwrap();
        TenantApi::new(std::sync::Arc::new(store), tokens)
            .await
            .unwrap()
    }

    fn create_body(v: Value) -> axum::extract::Json<CreateTenantBody> {
        axum::extract::Json(serde_json::from_value(v).unwrap())
    }

    async fn resp_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn create_returns_201_with_full_token() {
        let tokens = TenantTokenMap::default();
        let api = api_with(tokens.clone()).await;
        let state = crate::test_state_with_tenants(Some(api));
        let resp = create_tenant(
            axum::extract::State(state),
            create_body(json!({ "name": "acme" })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp_json(resp).await;
        assert_eq!(body["name"], "acme");
        let token = body["token"].as_str().unwrap();
        assert!(token.starts_with("tk_tenant_"));
        // The shared token map is rebuilt on create.
        assert_eq!(tokens.read().unwrap().get(token), Some(&"acme".to_string()));
    }

    #[tokio::test]
    async fn create_rejects_invalid_name() {
        let api = api_with(TenantTokenMap::default()).await;
        let state = crate::test_state_with_tenants(Some(api));
        let resp = create_tenant(
            axum::extract::State(state),
            create_body(json!({ "name": "Not Valid" })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_duplicate_name_is_409() {
        let api = api_with(TenantTokenMap::default()).await;
        let state = crate::test_state_with_tenants(Some(api));
        let first = create_tenant(
            axum::extract::State(state.clone()),
            create_body(json!({ "name": "acme" })),
        )
        .await;
        assert_eq!(first.status(), StatusCode::CREATED);
        let second = create_tenant(
            axum::extract::State(state),
            create_body(json!({ "name": "acme" })),
        )
        .await;
        assert_eq!(second.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn list_redacts_the_token() {
        let api = api_with(TenantTokenMap::default()).await;
        let state = crate::test_state_with_tenants(Some(api));
        let create = create_tenant(
            axum::extract::State(state.clone()),
            create_body(json!({ "name": "acme" })),
        )
        .await;
        let full_token = resp_json(create).await["token"]
            .as_str()
            .unwrap()
            .to_string();

        let resp = list_tenants(axum::extract::State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp_json(resp).await;
        let listed = body["tenants"][0]["token"].as_str().unwrap();
        assert_ne!(listed, full_token);
        assert!(listed.starts_with('…'));
        assert!(full_token.ends_with(&listed[3..])); // last 4 chars survive redaction
    }

    #[tokio::test]
    async fn rotate_replaces_token_in_shared_map() {
        let tokens = TenantTokenMap::default();
        let api = api_with(tokens.clone()).await;
        let state = crate::test_state_with_tenants(Some(api));
        let create = create_tenant(
            axum::extract::State(state.clone()),
            create_body(json!({ "name": "acme" })),
        )
        .await;
        let old_token = resp_json(create).await["token"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(tokens.read().unwrap().contains_key(&old_token));

        let resp = rotate_tenant_token(
            axum::extract::State(state),
            axum::extract::Path("acme".to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let new_token = resp_json(resp).await["token"].as_str().unwrap().to_string();
        assert_ne!(new_token, old_token);
        assert!(!tokens.read().unwrap().contains_key(&old_token));
        assert_eq!(
            tokens.read().unwrap().get(&new_token),
            Some(&"acme".to_string())
        );
    }

    #[tokio::test]
    async fn rotate_unknown_name_is_404() {
        let api = api_with(TenantTokenMap::default()).await;
        let state = crate::test_state_with_tenants(Some(api));
        let resp = rotate_tenant_token(
            axum::extract::State(state),
            axum::extract::Path("nope".to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_removes_token_from_shared_map() {
        let tokens = TenantTokenMap::default();
        let api = api_with(tokens.clone()).await;
        let state = crate::test_state_with_tenants(Some(api));
        let create = create_tenant(
            axum::extract::State(state.clone()),
            create_body(json!({ "name": "acme" })),
        )
        .await;
        let token = resp_json(create).await["token"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(tokens.read().unwrap().contains_key(&token));

        let resp = delete_tenant(
            axum::extract::State(state.clone()),
            axum::extract::Path("acme".to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!tokens.read().unwrap().contains_key(&token));

        let again = delete_tenant(
            axum::extract::State(state),
            axum::extract::Path("acme".to_string()),
        )
        .await;
        assert_eq!(again.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn disabled_subsystem_404s() {
        let state = crate::test_state_with_tenants(None);
        let resp = list_tenants(axum::extract::State(state)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ---- GET /api/tenants/summary ---------------------------------------------------------

    fn tenant_attrs(
        tenant: &str,
        extra: &[(&str, &str)],
    ) -> std::collections::BTreeMap<String, String> {
        let mut a = std::collections::BTreeMap::new();
        a.insert("tenant".to_string(), tenant.to_string());
        for (k, v) in extra {
            a.insert((*k).to_string(), (*v).to_string());
        }
        a
    }

    fn up_point(tenant: &str, mode: &str, ts: i64) -> photon_core::metric_record::MetricPoint {
        photon_core::metric_record::MetricPoint {
            metric_name: "photon.federation.up".to_string(),
            metric_type: photon_core::metric_schema::metric_type::GAUGE,
            timestamp_nanos: ts,
            value: Some(1.0),
            attributes: tenant_attrs(tenant, &[("mode", mode)]),
            ..Default::default()
        }
    }

    fn rows_point(tenant: &str, ts: i64, value: f64) -> photon_core::metric_record::MetricPoint {
        photon_core::metric_record::MetricPoint {
            metric_name: "photon.federation.ingest.rows".to_string(),
            metric_type: photon_core::metric_schema::metric_type::SUM,
            temporality: Some(2), // cumulative
            is_monotonic: Some(true),
            timestamp_nanos: ts,
            start_timestamp_nanos: Some(0),
            value: Some(value),
            attributes: tenant_attrs(tenant, &[("signal", "logs")]),
            ..Default::default()
        }
    }

    fn incidents_point(
        tenant: &str,
        ts: i64,
        value: f64,
    ) -> photon_core::metric_record::MetricPoint {
        photon_core::metric_record::MetricPoint {
            metric_name: "photon.federation.incidents.open".to_string(),
            metric_type: photon_core::metric_schema::metric_type::GAUGE,
            timestamp_nanos: ts,
            value: Some(value),
            attributes: tenant_attrs(tenant, &[]),
            ..Default::default()
        }
    }

    fn hot_bytes_point(
        tenant: &str,
        ts: i64,
        value: f64,
    ) -> photon_core::metric_record::MetricPoint {
        photon_core::metric_record::MetricPoint {
            metric_name: "photon.federation.disk.hot_bytes".to_string(),
            metric_type: photon_core::metric_schema::metric_type::GAUGE,
            timestamp_nanos: ts,
            value: Some(value),
            attributes: tenant_attrs(tenant, &[("signal", "logs")]),
            ..Default::default()
        }
    }

    /// Ingest -> compact -> engine, mirroring `photon-query/tests/metric_query.rs`'s
    /// `engine_with`. Promoted attributes are just `tenant` (this endpoint's only filter/group
    /// column). The `tempfile::TempDir` is leaked into the return so the hot dir outlives the
    /// test.
    async fn engine_with(
        points: Vec<photon_core::metric_record::MetricPoint>,
    ) -> (MetricsQueryEngine, tempfile::TempDir) {
        use photon_core::config::WalConfig;
        use photon_core::metric_record::MetricBatchBuilder;
        use photon_core::metric_schema::{metric_type, MetricSchema};

        let tmp = tempfile::tempdir().unwrap();
        let hot_dir = tmp.path().to_path_buf();
        // Sort keys `service.name`/`host.name` must exist as columns even though this test never
        // populates them (metrics_compactor.rs's `sort_metrics` requires both unconditionally,
        // mirroring how `photon-server` always injects `host.name` into the real schema).
        let schema = MetricSchema::new(&[
            "service.name".to_string(),
            "host.name".to_string(),
            "tenant".to_string(),
        ]);
        let wal_cfg = WalConfig {
            segment_max_bytes: 1,
            segment_max_age_secs: 0,
            group_commit_max_delay_ms: 0,
        };
        let wal = std::sync::Arc::new(
            photon_wal::DiskWal::open_arrow(
                hot_dir.join("wal-metrics"),
                schema.arrow.clone(),
                wal_cfg,
            )
            .await
            .unwrap(),
        );
        for p in &points {
            let mut b = MetricBatchBuilder::new(&schema);
            b.append(p);
            wal.append(b.finish().unwrap()).await.unwrap();
            wal.sync().await.unwrap();
        }
        // Trailing append (far before any test window) to close the last real data segment.
        let seal = photon_core::metric_record::MetricPoint {
            metric_name: "__seal__".to_string(),
            metric_type: metric_type::GAUGE,
            timestamp_nanos: 1,
            value: Some(0.0),
            attributes: tenant_attrs("__seal__", &[]),
            ..Default::default()
        };
        let mut tail = MetricBatchBuilder::new(&schema);
        tail.append(&seal);
        wal.append(tail.finish().unwrap()).await.unwrap();
        wal.sync().await.unwrap();

        let storage = photon_storage::Storage {
            hot: std::sync::Arc::new(
                object_store::local::LocalFileSystem::new_with_prefix(&hot_dir).unwrap(),
            ),
            durable: None,
            hot_dir: Some(hot_dir.clone()),
        };
        let replicator = std::sync::Arc::new(photon_storage::Replicator::new(storage.clone()));
        let compactor =
            photon_compact::MetricsCompactor::new(wal.clone(), storage, replicator, schema.clone());
        while compactor.run_once().await.unwrap().is_some() {}

        (MetricsQueryEngine::new(hot_dir, schema).unwrap(), tmp)
    }

    fn now_nanos() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64
    }

    #[tokio::test]
    async fn summary_reports_fresh_and_never_reported_tenants() {
        let now = now_nanos();
        let points = vec![
            up_point("acme", "summary", now - 5_000_000_000),
            rows_point("acme", now - 600_000_000_000, 0.0),
            rows_point("acme", now - 5_000_000_000, 300.0),
            incidents_point("acme", now - 5_000_000_000, 2.0),
            hot_bytes_point("acme", now - 5_000_000_000, 4096.0),
        ];
        let (engine, _tmp) = engine_with(points).await;

        let store = SqliteTenantStore::open_in_memory().unwrap();
        store.create(&tenant("acme", "tk_acme")).await.unwrap();
        store
            .create(&tenant("initech", "tk_initech"))
            .await
            .unwrap();
        let api = TenantApi::new(std::sync::Arc::new(store), TenantTokenMap::default())
            .await
            .unwrap();

        let state = crate::test_state_with_tenants_and_metrics(Some(api), engine);
        let resp = summary(axum::extract::State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Vec<Value> = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();

        let acme = body.iter().find(|t| t["name"] == "acme").unwrap();
        assert_eq!(acme["status"], "up");
        assert_eq!(acme["mode"], "summary");
        assert!(acme["last_seen_ms"].as_i64().unwrap() > 0);
        assert!(
            acme["ingest_rows_per_sec"].as_f64().unwrap() > 0.0,
            "acme: {acme}"
        );
        assert_eq!(acme["open_incidents"], 2.0);
        assert_eq!(acme["hot_bytes"], 4096.0);

        let initech = body.iter().find(|t| t["name"] == "initech").unwrap();
        assert_eq!(initech["status"], "down");
        assert_eq!(initech["mode"], Value::Null);
        assert_eq!(initech["last_seen_ms"], 0);
        assert_eq!(initech["ingest_rows_per_sec"], 0.0);
    }

    #[tokio::test]
    async fn summary_disabled_subsystem_404s() {
        let state = crate::test_state_with_tenants(None);
        let resp = summary(axum::extract::State(state)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

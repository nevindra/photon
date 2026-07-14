//! RUM ingest (`POST /api/rum`, public). This module also owns the session-authed app-management
//! handlers (create/update/rotate/delete) and the `GET /api/rum/*` read routes.
//!
//! The beacon is authed per-app: a public `key` selects the registered app, and the request
//! `Origin` must be in that app's allowlist. Vitals/errors are mapped (in photon-core) and handed
//! to a `RumSink` the server implements over the existing metrics + logs WALs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::rum_apps::{RumApp, RumAppStore};
use photon_core::metric_record::MetricPoint;
use photon_core::record::LogRecord;
use photon_core::rum::{self, Beacon, ATTR_ROUTE};
use photon_core::PhotonError;
use photon_query::{
    BreakdownRow, CountBucket, ErrorDetail, ErrorEvent, ErrorIssue, LcpAttribution, QueryRequest,
    TagBreakdown, VitalSummary,
};

use crate::AppState;

/// Server-supplied write path for RUM rows. Implemented in `photon-server` over the existing
/// metrics + logs `BroadcastingWal`s (photon-api cannot depend on photon-wal).
#[async_trait]
pub trait RumSink: Send + Sync {
    async fn ingest_vitals(&self, points: Vec<MetricPoint>) -> Result<(), PhotonError>;
    async fn ingest_errors(&self, records: Vec<LogRecord>) -> Result<(), PhotonError>;
}

/// The RUM subsystem handle attached to `AppState`: a store-backed registry of the browser apps
/// allowed to POST beacons (fronted by a live in-memory cache) plus the write sink. `None` on
/// `AppState` disables all `/api/rum*` routes.
#[derive(Clone)]
pub struct RumApi {
    store: Arc<dyn RumAppStore>,
    /// Live in-memory view keyed by app `key`, read by the beacon hot-path and the CORS predicate.
    /// Rebuilt from the store after every mutation (low volume — a full reload is simplest/correct).
    cache: Arc<RwLock<HashMap<String, RumApp>>>,
    sink: Arc<dyn RumSink>,
    /// Monotonic draw counter mixed with wall-clock nanos for a cheap, dependency-free
    /// pseudo-random source (see `should_sample`). Shared across clones (`RumApi` is cloned per
    /// request by axum's `State` extractor).
    sample_draws: Arc<AtomicU64>,
    /// Per-app fixed-window rate limiter: `key -> (window_start_secs, count_in_window)`.
    /// Best-effort, not a precise limiter — see `check_rate_limit`.
    rate_windows: Arc<Mutex<HashMap<String, (u64, u32)>>>,
}

impl RumApi {
    /// Build the registry over a durable store, loading the initial cache from it.
    pub async fn new(store: Arc<dyn RumAppStore>, sink: Arc<dyn RumSink>) -> RumApi {
        let api = RumApi {
            store,
            cache: Arc::new(RwLock::new(HashMap::new())),
            sink,
            sample_draws: Arc::new(AtomicU64::new(0)),
            rate_windows: Arc::new(Mutex::new(HashMap::new())),
        };
        api.reload_cache().await;
        api
    }

    /// Re-read every app from the store and replace the cache. Called at startup and after each
    /// mutation. A store read failure leaves the previous cache intact (fail-safe).
    async fn reload_cache(&self) {
        if let Ok(apps) = self.store.list().await {
            let map = apps.into_iter().map(|a| (a.key.clone(), a)).collect();
            *self.cache.write().unwrap() = map;
        }
    }

    pub async fn create_app(&self, app: RumApp) -> Result<(), PhotonError> {
        self.store.create(&app).await?;
        self.reload_cache().await;
        Ok(())
    }

    pub async fn update_app(
        &self,
        name: &str,
        allowed_origins: Vec<String>,
        sample_rate: f64,
        rate_limit: u32,
    ) -> Result<bool, PhotonError> {
        let ok = self
            .store
            .update(name, &allowed_origins, sample_rate, rate_limit)
            .await?;
        if ok {
            self.reload_cache().await;
        }
        Ok(ok)
    }

    pub async fn rotate_key(&self, name: &str, new_key: &str) -> Result<bool, PhotonError> {
        let ok = self.store.rotate_key(name, new_key).await?;
        if ok {
            self.reload_cache().await;
        }
        Ok(ok)
    }

    pub async fn delete_app(&self, name: &str) -> Result<bool, PhotonError> {
        let ok = self.store.delete(name).await?;
        if ok {
            self.reload_cache().await;
        }
        Ok(ok)
    }

    /// True if any registered app allows `origin` — read by the beacon router's CORS predicate.
    pub fn origin_allowed(&self, origin: &str) -> bool {
        self.cache
            .read()
            .unwrap()
            .values()
            .any(|a| a.allowed_origins.iter().any(|o| o == origin))
    }

    /// All registered apps, sorted by name — drives `GET /api/rum/apps`.
    pub fn list_apps(&self) -> Vec<RumApp> {
        let mut v: Vec<RumApp> = self.cache.read().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// A coin-flip drop decision for `sample_rate` (probability `1 - sample_rate` of dropping).
    /// `>= 1.0` never drops; `<= 0.0` always drops. In between, mixes a monotonic draw counter
    /// with wall-clock nanos (no `rand` dependency) into a `[0, 1)` fraction.
    fn should_sample(&self, sample_rate: f64) -> bool {
        if sample_rate >= 1.0 {
            return true;
        }
        if sample_rate <= 0.0 {
            return false;
        }
        let draw = self.sample_draws.fetch_add(1, Ordering::Relaxed);
        let nanos = now_nanos() as u64;
        // A splitmix64-style mix so back-to-back calls (same wall-clock nanosecond, e.g. in
        // tests or bursts) still disperse across the range instead of aliasing.
        let mut x = draw.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ nanos;
        x ^= x >> 33;
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        x ^= x >> 33;
        let frac = (x % 1_000_000) as f64 / 1_000_000.0;
        frac < sample_rate
    }

    /// A simple per-app fixed-window limiter: at most `rate_limit` beacons per app per wall-clock
    /// second. Best-effort protection, not a precise limiter (a burst can straddle a window
    /// boundary). Returns `true` if the beacon is admitted, `false` if the window is exhausted.
    fn check_rate_limit(&self, app_key: &str, rate_limit: u32) -> bool {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut windows = self.rate_windows.lock().unwrap();
        let entry = windows.entry(app_key.to_string()).or_insert((now_secs, 0));
        if entry.0 != now_secs {
            *entry = (now_secs, 0);
        }
        if entry.1 >= rate_limit {
            false
        } else {
            entry.1 += 1;
            true
        }
    }

    /// Every configured origin across all apps (sorted, deduped) — the union used to build the CORS
    /// layer, and retained for tests. Reads the live cache.
    pub fn all_origins(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .cache
            .read()
            .unwrap()
            .values()
            .flat_map(|a| a.allowed_origins.iter().cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

fn now_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// `POST /api/rum` — the public browser beacon. Body is `text/plain` JSON (so it stays a CORS
/// simple request). Returns 204 on success; 400 malformed; 403 on a bad key/disallowed Origin (an
/// unregistered app is a 403, not a 404 — the beacon route is always mounted).
pub(crate) async fn beacon(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(rum) = st.rum.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let beacon: Beacon = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "malformed beacon").into_response(),
    };
    // Clone the matched app out of the read guard and drop the guard *before* any `.await` on the
    // sink below — never hold the cache `RwLock` across an await point.
    let app = {
        let cache = rum.cache.read().unwrap();
        match cache.get(&beacon.key) {
            Some(a) => a.clone(),
            None => return StatusCode::FORBIDDEN.into_response(),
        }
    };
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !app.allowed_origins.iter().any(|o| o == origin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Per-app operational controls, applied before mapping/ingesting (in that order: the rate
    // limiter protects the endpoint regardless of sampling outcome, then sampling trims volume
    // of what actually gets ingested).
    if !rum.check_rate_limit(&app.key, app.rate_limit) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if !rum.should_sample(app.sample_rate) {
        // Accepted-but-sampled-out: the client shouldn't retry.
        return StatusCode::NO_CONTENT.into_response();
    }
    let now = now_nanos();
    let points = rum::beacon_to_metric_points(&beacon, &app.name, now);
    let records = rum::beacon_to_log_records(&beacon, &app.name, now);
    if !points.is_empty() && rum.sink.ingest_vitals(points).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if !records.is_empty() && rum.sink.ingest_errors(records).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// App management handlers (`POST/PATCH/DELETE /api/rum/apps*`). Registered in `lib.rs`'s
// `protected` router (a later task), so they inherit the signed-session gate. They mutate the
// registry via `RumApi`'s store-backed methods; the server mints the public `key`.
// ---------------------------------------------------------------------------

/// Server-mint a public app key. `pk_live_` + a v4 UUID's 32 hex chars — unguessable enough for a
/// public identifier, and collision-free against the `UNIQUE` key constraint in practice.
fn mint_key() -> String {
    format!("pk_live_{}", uuid::Uuid::new_v4().simple())
}

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

#[derive(Deserialize)]
pub(crate) struct CreateAppBody {
    name: String,
    allowed_origins: Vec<String>,
    sample_rate: Option<f64>,
    rate_limit: Option<u32>,
}

/// `POST /api/rum/apps` — register a new app. Server mints the key. 201 with the record; 400 on
/// invalid fields; 409 on a duplicate name.
pub(crate) async fn create_app(
    State(st): State<AppState>,
    Json(body): Json<CreateAppBody>,
) -> Response {
    let Some(rum) = st.rum.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let name = body.name.trim().to_string();
    let sample_rate = body.sample_rate.unwrap_or(1.0);
    let rate_limit = body.rate_limit.unwrap_or(5000);
    if let Err(e) =
        crate::rum_apps::validate_app_fields(&name, &body.allowed_origins, sample_rate, rate_limit)
    {
        return bad_request(e);
    }
    if rum.list_apps().iter().any(|a| a.name == name) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "an app with that name already exists" })),
        )
            .into_response();
    }
    let app = crate::rum_apps::RumApp {
        name,
        key: mint_key(),
        allowed_origins: body.allowed_origins,
        sample_rate,
        rate_limit,
        created_at: now_ms(),
    };
    match rum.create_app(app.clone()).await {
        Ok(()) => (StatusCode::CREATED, Json(rum_app_json(&app))).into_response(),
        Err(e) => {
            // A racing create (or the UNIQUE/PK constraint) can fail the insert after the
            // pre-check passed — surface that as a 409, not a 500.
            if rum.list_apps().iter().any(|a| a.name == app.name) {
                (
                    StatusCode::CONFLICT,
                    Json(json!({ "error": "an app with that name already exists" })),
                )
                    .into_response()
            } else {
                err_500(e)
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct UpdateAppBody {
    allowed_origins: Option<Vec<String>>,
    sample_rate: Option<f64>,
    rate_limit: Option<u32>,
}

/// `PATCH /api/rum/apps/:name` — update origins / sampling / rate limit (name + key unchanged).
/// Omitted fields keep their current value. 200 with the record; 404 unknown; 400 invalid.
pub(crate) async fn update_app(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<UpdateAppBody>,
) -> Response {
    let Some(rum) = st.rum.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(current) = rum.list_apps().into_iter().find(|a| a.name == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let allowed_origins = body.allowed_origins.unwrap_or(current.allowed_origins);
    let sample_rate = body.sample_rate.unwrap_or(current.sample_rate);
    let rate_limit = body.rate_limit.unwrap_or(current.rate_limit);
    if let Err(e) =
        crate::rum_apps::validate_app_fields(&name, &allowed_origins, sample_rate, rate_limit)
    {
        return bad_request(e);
    }
    match rum
        .update_app(&name, allowed_origins.clone(), sample_rate, rate_limit)
        .await
    {
        Ok(true) => {
            let updated = crate::rum_apps::RumApp {
                name,
                key: current.key,
                allowed_origins,
                sample_rate,
                rate_limit,
                created_at: current.created_at,
            };
            (StatusCode::OK, Json(rum_app_json(&updated))).into_response()
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err_500(e),
    }
}

/// `POST /api/rum/apps/:name/rotate-key` — mint a fresh key (old key stops working). 200 `{key}`;
/// 404 unknown.
pub(crate) async fn rotate_app_key(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let Some(rum) = st.rum.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let new_key = mint_key();
    match rum.rotate_key(&name, &new_key).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "key": new_key }))).into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err_500(e),
    }
}

/// `DELETE /api/rum/apps/:name` — unregister an app. 204; 404 unknown.
pub(crate) async fn delete_app(State(st): State<AppState>, Path(name): Path<String>) -> Response {
    let Some(rum) = st.rum.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match rum.delete_app(&name).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err_500(e),
    }
}

// ---------------------------------------------------------------------------
// Session-authed read routes (`GET /api/rum/*`). Registered in `lib.rs`'s `protected`
// router, so they inherit the signed-session gate. They query the existing metrics/logs
// stores directly (the RUM app `name` is the row's `service.name`), so they work whether or
// not the RUM *ingest* subsystem is enabled — only `apps` reads the registry (`state.rum`).
// ---------------------------------------------------------------------------

/// Max error issues returned by `/rum/errors` and the page-detail error list.
const ERROR_LIMIT: usize = 100;

/// The facet fields surfaced by `/rum/errors/facets` — the dimensions the error-search UI lets
/// users filter/browse by.
const ERROR_FACET_FIELDS: [&str; 6] = [
    "exception.type",
    "error.kind",
    "browser.route",
    "browser.name",
    "device.type",
    "network.connection",
];
/// Top-N values returned per facet field.
const ERROR_FACET_LIMIT: usize = 20;

#[derive(Deserialize)]
pub(crate) struct VitalsParams {
    app: String,
    start: i64,
    end: i64,
}

#[derive(Deserialize)]
pub(crate) struct BreakdownParams {
    app: String,
    dimension: String,
    start: i64,
    end: i64,
}

#[derive(Deserialize)]
pub(crate) struct PagesParams {
    app: String,
    start: i64,
    end: i64,
}

#[derive(Deserialize)]
pub(crate) struct PageDetailParams {
    app: String,
    route: String,
    start: i64,
    end: i64,
}

#[derive(Deserialize)]
pub(crate) struct ErrorsParams {
    app: String,
    start: i64,
    end: i64,
    /// Optional log-query-grammar filter (same syntax as Logs search / `/api/facet`). Blank or
    /// absent → no filter.
    #[serde(default)]
    q: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ErrorDetailParams {
    app: String,
    start: i64,
    end: i64,
}

#[derive(Deserialize)]
pub(crate) struct ErrorFacetsParams {
    app: String,
    start: i64,
    end: i64,
    /// Optional log-query-grammar filter, same as `/rum/errors` — scopes each field's facet to the
    /// matching slice rather than the whole error dataset.
    #[serde(default)]
    q: Option<String>,
}

/// A query-engine failure → 500 with a JSON `{ "error": … }` body.
fn err_500(e: PhotonError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

/// One Core-Web-Vital scorecard: p75 + Google rating (`good`/`needs`/`poor`) + the thresholds and
/// the rating-count distribution the UI draws the bar from.
fn vital_summary_json(v: &VitalSummary) -> Value {
    let (good, poor) = rum::thresholds(&v.metric).unwrap_or((0.0, 0.0));
    let rating = if v.p75 <= good {
        "good"
    } else if v.p75 <= poor {
        "needs"
    } else {
        "poor"
    };
    json!({
        "metric": v.metric,
        "p75": v.p75,
        "rating": rating,
        "good_max": good,
        "poor_min": poor,
        "dist": { "good": v.good, "needs": v.needs, "poor": v.poor, "total": v.count },
    })
}

/// One breakdown row keyed by the grouping dimension value (device/browser/…). `*_p75` is `null`
/// for a vital with no samples in the group.
fn breakdown_row_json(r: &BreakdownRow) -> Value {
    json!({
        "key": r.key,
        "pageviews": r.pageviews,
        "lcp_p75": r.lcp_p75,
        "inp_p75": r.inp_p75,
        "cls_p75": r.cls_p75,
    })
}

/// A page row — a route-dimension breakdown row surfaced under the `route` key.
fn page_row_json(r: &BreakdownRow) -> Value {
    json!({
        "route": r.key,
        "pageviews": r.pageviews,
        "lcp_p75": r.lcp_p75,
        "inp_p75": r.inp_p75,
        "cls_p75": r.cls_p75,
    })
}

/// LCP attribution for a page: the four sub-part averages (ms) + the top LCP element. Every field
/// is `null` when its gauge/attribute had no samples in the window (SDK ran without attribution).
fn lcp_attribution_json(a: &LcpAttribution) -> Value {
    json!({
        "ttfb": a.ttfb,
        "resource_load_delay": a.resource_load_delay,
        "resource_load_time": a.resource_load_time,
        "element_render_delay": a.element_render_delay,
        "element": a.top_element,
    })
}

/// One error issue: fingerprint + a sample exception type/message + occurrence and distinct-session
/// counts.
fn error_issue_json(e: &ErrorIssue) -> Value {
    json!({
        "fingerprint": e.fingerprint,
        "exception_type": e.exception_type,
        "message": e.message,
        "count": e.count,
        "sessions": e.sessions,
        "trace_id": e.trace_id,
    })
}

/// `GET /api/rum/apps` — the registered RUM apps (full records; the public `key` is safe to
/// expose). Empty when nothing is registered yet.
pub(crate) async fn apps(State(st): State<AppState>) -> Response {
    let apps: Vec<Value> = st
        .rum
        .as_ref()
        .map(|r| r.list_apps().iter().map(rum_app_json).collect())
        .unwrap_or_default();
    Json(json!({ "apps": apps })).into_response()
}

fn rum_app_json(a: &crate::rum_apps::RumApp) -> Value {
    json!({
        "name": a.name,
        "key": a.key,
        "allowed_origins": a.allowed_origins,
        "sample_rate": a.sample_rate,
        "rate_limit": a.rate_limit,
        "created_at": a.created_at,
    })
}

/// `GET /api/rum/vitals?app&start&end` — the five Core-Web-Vitals scorecards for an app over the
/// window (vitals with no samples are omitted by the engine).
pub(crate) async fn vitals(State(st): State<AppState>, Query(p): Query<VitalsParams>) -> Response {
    let summaries = match st.metrics_query.rum_vitals(&p.app, p.start, p.end).await {
        Ok(v) => v,
        Err(e) => return err_500(e),
    };
    let vitals: Vec<Value> = summaries.iter().map(vital_summary_json).collect();
    Json(json!({ "app": p.app, "vitals": vitals })).into_response()
}

/// `GET /api/rum/vitals/breakdown?app&dimension&start&end` — LCP/INP/CLS p75 grouped by one
/// dimension (device / browser / route / …).
pub(crate) async fn breakdown(
    State(st): State<AppState>,
    Query(p): Query<BreakdownParams>,
) -> Response {
    let rows = match st
        .metrics_query
        .rum_breakdown(&p.app, &p.dimension, p.start, p.end, None)
        .await
    {
        Ok(v) => v,
        Err(e) => return err_500(e),
    };
    let rows: Vec<Value> = rows.iter().map(breakdown_row_json).collect();
    Json(json!({ "app": p.app, "dimension": p.dimension, "rows": rows })).into_response()
}

/// `GET /api/rum/pages?app&start&end` — one row per `browser.route`, with that page's LCP/INP/CLS
/// p75 and a pageview proxy. A route-dimension breakdown surfaced as the pages list.
pub(crate) async fn pages(State(st): State<AppState>, Query(p): Query<PagesParams>) -> Response {
    let rows = match st
        .metrics_query
        .rum_breakdown(&p.app, ATTR_ROUTE, p.start, p.end, None)
        .await
    {
        Ok(v) => v,
        Err(e) => return err_500(e),
    };
    let pages: Vec<Value> = rows.iter().map(page_row_json).collect();
    Json(json!({ "app": p.app, "pages": pages })).into_response()
}

/// `GET /api/rum/pages/detail?app&route&start&end` — one page's vitals (from the app's route
/// breakdown), its device breakdown, and its error issues — the last two route-scoped.
pub(crate) async fn page_detail(
    State(st): State<AppState>,
    Query(p): Query<PageDetailParams>,
) -> Response {
    // Page vitals: the matching row from the app-wide `browser.route` breakdown.
    let page_rows = match st
        .metrics_query
        .rum_breakdown(&p.app, ATTR_ROUTE, p.start, p.end, None)
        .await
    {
        Ok(v) => v,
        Err(e) => return err_500(e),
    };
    let vitals: Option<Value> = page_rows.iter().find(|r| r.key == p.route).map(|r| {
        json!({
            "pageviews": r.pageviews,
            "lcp_p75": r.lcp_p75,
            "inp_p75": r.inp_p75,
            "cls_p75": r.cls_p75,
        })
    });

    // Device breakdown scoped to this route.
    let device_rows = match st
        .metrics_query
        .rum_breakdown(&p.app, "device.type", p.start, p.end, Some(&p.route))
        .await
    {
        Ok(v) => v,
        Err(e) => return err_500(e),
    };

    // Error issues scoped to this route.
    let errors = match st
        .query
        .rum_errors(&p.app, p.start, p.end, ERROR_LIMIT, Some(&p.route), None)
        .await
    {
        Ok(v) => v,
        Err(e) => return err_500(e),
    };

    // LCP attribution (the four sub-part averages + top element) scoped to this route.
    let attribution = match st
        .metrics_query
        .rum_lcp_attribution(&p.app, Some(&p.route), p.start, p.end)
        .await
    {
        Ok(v) => v,
        Err(e) => return err_500(e),
    };

    Json(json!({
        "app": p.app,
        "route": p.route,
        "vitals": vitals,
        "breakdown": device_rows.iter().map(breakdown_row_json).collect::<Vec<_>>(),
        "errors": errors.iter().map(error_issue_json).collect::<Vec<_>>(),
        "attribution": { "lcp": lcp_attribution_json(&attribution) },
    }))
    .into_response()
}

/// `GET /api/rum/errors?app&start&end&q` — the top error issues for an app, grouped by
/// fingerprint, optionally filtered by the log query grammar (`q`, same syntax as Logs search /
/// `/api/facet`). A malformed `q` yields a 400 with a byte `offset` (see `QueryParamError`).
pub(crate) async fn errors(State(st): State<AppState>, Query(p): Query<ErrorsParams>) -> Response {
    let promoted = st.query.promoted_attributes();
    let rq = match crate::query_params::resolve_query(p.q.as_deref().unwrap_or(""), promoted) {
        Ok(rq) => rq,
        Err(e) => return e.into_response(),
    };
    let issues = match st
        .query
        .rum_errors(&p.app, p.start, p.end, ERROR_LIMIT, None, rq)
        .await
    {
        Ok(v) => v,
        Err(e) => return err_500(e),
    };
    let errors: Vec<Value> = issues.iter().map(error_issue_json).collect();
    Json(json!({ "app": p.app, "errors": errors })).into_response()
}

/// `GET /api/rum/errors/:fingerprint?app&start&end` — full detail for one error issue: header
/// stats, an occurrence series, tag breakdowns, a sample stack, and recent sample events. 200
/// with all-empty sections when the fingerprint has no rows in the window.
pub(crate) async fn error_detail(
    State(st): State<AppState>,
    Path(fingerprint): Path<String>,
    Query(p): Query<ErrorDetailParams>,
) -> Response {
    let detail = match st
        .query
        .rum_error_detail(&p.app, &fingerprint, p.start, p.end)
        .await
    {
        Ok(d) => d,
        Err(e) => return err_500(e),
    };
    Json(error_detail_json(&p.app, &fingerprint, &detail)).into_response()
}

/// `GET /api/rum/errors/facets?app&start&end&q` — top values (+counts) for each of the 6 error
/// facet fields, scoped to the app's error dataset (severity 17-24, the `LogRecord` error range)
/// and optionally narrowed by the log query grammar (`q`, same syntax/parity as `/rum/errors`). A
/// malformed `q` yields a 400 with a byte `offset` (see `QueryParamError`). Registered ahead of
/// `/rum/errors/:fingerprint` in `lib.rs` so axum matches this static segment first.
pub(crate) async fn error_facets(
    State(st): State<AppState>,
    Query(p): Query<ErrorFacetsParams>,
) -> Response {
    let promoted = st.query.promoted_attributes();
    let rq = match crate::query_params::resolve_query(p.q.as_deref().unwrap_or(""), promoted) {
        Ok(rq) => rq,
        Err(e) => return e.into_response(),
    };
    let mut facets = serde_json::Map::new();
    for field in ERROR_FACET_FIELDS {
        let req = QueryRequest {
            start_ts_nanos: p.start,
            end_ts_nanos: p.end,
            services: vec![p.app.clone()],
            severities: vec![(17, 24)],
            text: None,
            query: rq.clone(),
            limit: 0,
        };
        let fr = match st.query.facet(field, req, ERROR_FACET_LIMIT).await {
            Ok(f) => f,
            Err(e) => return err_500(e),
        };
        facets.insert(
            field.to_string(),
            json!({
                "values": fr.values.iter().map(|v| json!({ "value": v.value, "count": v.count })).collect::<Vec<_>>(),
                "capped": fr.capped,
            }),
        );
    }
    Json(json!({ "app": p.app, "facets": facets })).into_response()
}

fn error_detail_json(app: &str, fingerprint: &str, d: &ErrorDetail) -> Value {
    json!({
        "app": app,
        "fingerprint": fingerprint,
        "exception_type": d.exception_type,
        "message": d.message,
        "error_kind": d.error_kind,
        "first_seen": d.first_seen,
        "last_seen": d.last_seen,
        "occurrences": d.occurrences,
        "sessions": d.sessions,
        "series": d.series.iter().map(count_bucket_json).collect::<Vec<_>>(),
        "tags": d.tags.iter().map(tag_breakdown_json).collect::<Vec<_>>(),
        "sample_stack": d.sample_stack,
        "events": d.events.iter().map(error_event_json).collect::<Vec<_>>(),
    })
}

fn count_bucket_json(c: &CountBucket) -> Value {
    json!({ "t": c.t, "count": c.count })
}

fn tag_breakdown_json(t: &TagBreakdown) -> Value {
    json!({
        "field": t.field,
        "values": t.values.iter().map(|v| json!({ "value": v.value, "count": v.count })).collect::<Vec<_>>(),
    })
}

fn error_event_json(e: &ErrorEvent) -> Value {
    json!({
        "timestamp": e.timestamp,
        "route": e.route,
        "browser": e.browser,
        "device": e.device,
        "session": e.session,
        "trace_id": e.trace_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::sync::Mutex;

    /// Records how many batches (calls) and how many total points/records the handler forwarded,
    /// so tests can assert both "a batch arrived" and "it carried the mapped rows". Reused by the
    /// server E2E test.
    #[derive(Default)]
    struct FakeSink {
        vital_batches: Mutex<usize>,
        vital_points: Mutex<usize>,
        error_batches: Mutex<usize>,
        error_records: Mutex<usize>,
    }
    #[async_trait]
    impl RumSink for FakeSink {
        async fn ingest_vitals(&self, p: Vec<MetricPoint>) -> Result<(), PhotonError> {
            *self.vital_batches.lock().unwrap() += 1;
            *self.vital_points.lock().unwrap() += p.len();
            Ok(())
        }
        async fn ingest_errors(&self, r: Vec<LogRecord>) -> Result<(), PhotonError> {
            *self.error_batches.lock().unwrap() += 1;
            *self.error_records.lock().unwrap() += r.len();
            Ok(())
        }
    }

    fn app() -> RumApp {
        RumApp {
            name: "web".into(),
            key: "pk_1".into(),
            allowed_origins: vec!["https://ok.example".into()],
            sample_rate: 1.0,
            rate_limit: 5000,
            created_at: 0,
        }
    }

    const BODY: &str = r#"{"app":"web","key":"pk_1","session":"s","view":{"id":"v","route":"/c","path":"/c"},"ctx":{"ua":"iPhone Safari","conn":"4g"},"vitals":[{"n":"LCP","v":4300}],"errors":[{"kind":"exception","type":"TypeError","msg":"x","src":"a.js","line":1}]}"#;

    use crate::rum_apps::SqliteRumAppStore;

    /// Build a `RumApi` over an in-memory store seeded with `apps` (mirrors the server wiring:
    /// register apps in the store, then construct the registry so its cache loads from the store).
    async fn rum_api_with(apps: Vec<RumApp>, sink: Arc<dyn RumSink>) -> RumApi {
        let store = SqliteRumAppStore::open_in_memory().unwrap();
        for a in &apps {
            store.create(a).await.unwrap();
        }
        RumApi::new(Arc::new(store), sink).await
    }

    /// Build an `AppState` whose `rum` is a `RumApi` over the given `FakeSink`, and return both so
    /// the caller can drive `beacon` and then assert on what the sink saw.
    async fn state_with_sink(sink: Arc<FakeSink>) -> AppState {
        let rum = rum_api_with(vec![app()], sink).await;
        crate::test_state_with_rum(Some(rum))
    }

    /// Like `state_with_sink`, but registers a caller-supplied app (e.g. a non-default
    /// `sample_rate`/`rate_limit`) instead of the default `app()`.
    async fn state_with_app(app: RumApp, sink: Arc<FakeSink>) -> AppState {
        let rum = rum_api_with(vec![app], sink).await;
        crate::test_state_with_rum(Some(rum))
    }

    fn headers_with_origin(origin: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("origin", HeaderValue::from_str(origin).unwrap());
        h
    }

    #[tokio::test]
    async fn valid_key_and_origin_ingests_and_returns_204() {
        let sink = Arc::new(FakeSink::default());
        let state = state_with_sink(sink.clone()).await;
        let resp = beacon(
            State(state),
            headers_with_origin("https://ok.example"),
            Bytes::from_static(BODY.as_bytes()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        // One vital batch (the LCP point) and one error batch (the TypeError record).
        assert_eq!(*sink.vital_batches.lock().unwrap(), 1);
        assert_eq!(*sink.vital_points.lock().unwrap(), 1);
        assert_eq!(*sink.error_batches.lock().unwrap(), 1);
        assert_eq!(*sink.error_records.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn zero_sample_rate_drops_the_beacon_but_still_returns_204() {
        let sink = Arc::new(FakeSink::default());
        let mut a = app();
        a.sample_rate = 0.0;
        let state = state_with_app(a, sink.clone()).await;
        let resp = beacon(
            State(state),
            headers_with_origin("https://ok.example"),
            Bytes::from_static(BODY.as_bytes()),
        )
        .await;
        // Accepted-but-sampled-out: still 204 (the client shouldn't retry), but the sink never
        // sees the vitals/errors.
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(*sink.vital_batches.lock().unwrap(), 0);
        assert_eq!(*sink.error_batches.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn rate_limit_of_one_rejects_the_second_beacon_in_the_window() {
        let sink = Arc::new(FakeSink::default());
        let mut a = app();
        a.rate_limit = 1;
        let state = state_with_app(a, sink.clone()).await;
        let first = beacon(
            State(state.clone()),
            headers_with_origin("https://ok.example"),
            Bytes::from_static(BODY.as_bytes()),
        )
        .await;
        assert_eq!(first.status(), StatusCode::NO_CONTENT);
        let second = beacon(
            State(state),
            headers_with_origin("https://ok.example"),
            Bytes::from_static(BODY.as_bytes()),
        )
        .await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        // Only the first beacon made it to the sink.
        assert_eq!(*sink.vital_batches.lock().unwrap(), 1);
        assert_eq!(*sink.error_batches.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn unknown_key_is_forbidden() {
        let sink = Arc::new(FakeSink::default());
        let state = state_with_sink(sink.clone()).await;
        let body = BODY.replace("pk_1", "pk_unregistered");
        let resp = beacon(
            State(state),
            headers_with_origin("https://ok.example"),
            Bytes::from(body),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(*sink.vital_batches.lock().unwrap(), 0);
        assert_eq!(*sink.error_batches.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn disallowed_origin_is_forbidden() {
        let sink = Arc::new(FakeSink::default());
        let state = state_with_sink(sink.clone()).await;
        let resp = beacon(
            State(state),
            headers_with_origin("https://evil.example"),
            Bytes::from_static(BODY.as_bytes()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(*sink.vital_batches.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn missing_origin_is_forbidden() {
        let sink = Arc::new(FakeSink::default());
        let state = state_with_sink(sink.clone()).await;
        // No `Origin` header at all -> treated as "" -> not in the allowlist.
        let resp = beacon(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(BODY.as_bytes()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(*sink.error_batches.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn malformed_body_is_bad_request() {
        let sink = Arc::new(FakeSink::default());
        let state = state_with_sink(sink.clone()).await;
        let resp = beacon(
            State(state),
            headers_with_origin("https://ok.example"),
            Bytes::from_static(b"{ this is not json"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(*sink.vital_batches.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn disabled_subsystem_returns_404() {
        // `rum: None` on the state -> route body reports the subsystem is off.
        let state = crate::test_state_with_rum(None);
        let resp = beacon(
            State(state),
            headers_with_origin("https://ok.example"),
            Bytes::from_static(BODY.as_bytes()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ---- session-authed read routes (`GET /api/rum/*`) ------------------------------------

    /// A routed test server whose RUM subsystem registers the single `web` app (over a no-op
    /// [`FakeSink`]), so the authed GET routes can resolve `apps` and query the empty stores.
    async fn router_with_rum() -> axum::Router {
        let rum = rum_api_with(vec![app()], Arc::new(FakeSink::default())).await;
        crate::test_server().with_rum(Some(rum)).into_router()
    }

    /// Log in, then GET `uri` with the session cookie; decode the JSON body.
    async fn authed_get(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        use tower::ServiceExt;
        let cookie = crate::session_cookie(router).await;
        let resp = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn apps_requires_session() {
        use tower::ServiceExt;
        let router = router_with_rum().await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/rum/apps")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn apps_lists_registered_names() {
        let router = router_with_rum().await;
        let (status, v) = authed_get(&router, "/api/rum/apps").await;
        assert_eq!(status, StatusCode::OK);
        // `apps` now returns full records (not just names); the public `key` is intentionally
        // exposed, alongside the origins/sampling/rate-limit/created_at fields.
        assert_eq!(v["apps"].as_array().unwrap().len(), 1);
        assert_eq!(v["apps"][0]["name"], "web");
        assert_eq!(v["apps"][0]["key"], "pk_1");
        assert_eq!(v["apps"][0]["sample_rate"], 1.0);
        assert_eq!(v["apps"][0]["rate_limit"], 5000);
    }

    #[tokio::test]
    async fn apps_with_subsystem_disabled_is_authed_and_empty() {
        // `rum: None` (the default `test_router`) -> route still requires the session, then
        // returns an empty app list rather than 404.
        let router = crate::test_router();
        let (status, v) = authed_get(&router, "/api/rum/apps").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["apps"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn vitals_returns_array_on_empty_store() {
        let router = router_with_rum().await;
        let (status, v) = authed_get(&router, "/api/rum/vitals?app=web&start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["app"], "web");
        assert_eq!(v["vitals"], serde_json::json!([])); // no data -> empty scorecard list
    }

    #[tokio::test]
    async fn breakdown_returns_rows_array() {
        let router = router_with_rum().await;
        let (status, v) = authed_get(
            &router,
            "/api/rum/vitals/breakdown?app=web&dimension=device.type&start=0&end=100",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["dimension"], "device.type");
        assert!(v["rows"].is_array());
    }

    #[tokio::test]
    async fn pages_returns_pages_array() {
        let router = router_with_rum().await;
        let (status, v) = authed_get(&router, "/api/rum/pages?app=web&start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert!(v["pages"].is_array());
    }

    #[tokio::test]
    async fn page_detail_has_vitals_breakdown_errors() {
        let router = router_with_rum().await;
        let (status, v) = authed_get(
            &router,
            "/api/rum/pages/detail?app=web&route=%2Fcheckout&start=0&end=100",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["route"], "/checkout");
        assert!(v["vitals"].is_null()); // no data for this route -> null
        assert!(v["breakdown"].is_array());
        assert!(v["errors"].is_array());
        // LCP attribution object is always present; with no data every sub-part + element is null.
        assert!(v["attribution"]["lcp"].is_object());
        assert!(v["attribution"]["lcp"]["ttfb"].is_null());
        assert!(v["attribution"]["lcp"]["element"].is_null());
    }

    #[tokio::test]
    async fn errors_returns_errors_array() {
        let router = router_with_rum().await;
        let (status, v) = authed_get(&router, "/api/rum/errors?app=web&start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert!(v["errors"].is_array());
    }

    #[tokio::test]
    async fn errors_accepts_query_grammar() {
        let router = router_with_rum().await;
        let (status, v) = authed_get(
            &router,
            "/api/rum/errors?app=web&start=0&end=100&q=exception.type:TypeError",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(v["errors"].is_array());
    }

    #[tokio::test]
    async fn errors_rejects_bad_grammar_with_offset() {
        use tower::ServiceExt;
        let router = router_with_rum().await;
        let cookie = crate::session_cookie(&router).await;
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/rum/errors?app=web&start=0&end=100&q=%3A%3Abad")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn error_facets_returns_all_fields() {
        let router = router_with_rum().await;
        let (status, v) =
            authed_get(&router, "/api/rum/errors/facets?app=web&start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert!(v["facets"]["exception.type"]["values"].is_array());
        assert!(v["facets"]["browser.name"].is_object());
        assert!(v["facets"]["device.type"].is_object());
    }

    #[tokio::test]
    async fn error_facets_rejects_bad_grammar_with_400() {
        use tower::ServiceExt;
        let router = router_with_rum().await;
        let cookie = crate::session_cookie(&router).await;
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/rum/errors/facets?app=web&start=0&end=100&q=%3A%3Abad")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_validates_and_mints_key() {
        assert!(mint_key().starts_with("pk_live_"));
        assert_eq!(mint_key().len(), "pk_live_".len() + 32);

        // validation is shared with the store helper:
        assert!(
            crate::rum_apps::validate_app_fields("web", &["https://a".into()], 1.0, 5000).is_ok()
        );
        assert!(crate::rum_apps::validate_app_fields("web", &[], 1.0, 5000).is_err());
    }

    #[tokio::test]
    async fn error_detail_returns_detail_shape() {
        let router = router_with_rum().await;
        let (status, v) = authed_get(
            &router,
            "/api/rum/errors/deadbeefdeadbeef?app=web&start=0&end=100",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(v["fingerprint"].is_string());
        assert!(v["series"].is_array());
        assert!(v["tags"].is_array());
        assert!(v["events"].is_array());
        assert_eq!(v["occurrences"], 0); // FakeSink has no rows -> empty-but-200
    }
}

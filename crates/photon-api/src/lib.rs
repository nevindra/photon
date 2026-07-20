//! photon-api: REST API (session auth + log search) that also serves the embedded Vue UI.
//!
//! Implemented per the `photon-api` section of
//! `docs/superpowers/plans/2026-07-01-photon-interface-contracts.md`.
//!
//! An axum [`Router`] over shared [`AppState`] (the query engine, the user table, and the
//! cookie signing key). JSON endpoints live under `/api`; every other GET is served from the
//! embedded frontend bundle (with an SPA fallback to `index.html`).

pub mod alerts;
mod assets;
mod auth;
mod data;
mod facet;
mod fields;
mod histogram;
mod infra;
mod metrics;
mod query_params;
mod red;
mod rum;
pub mod rum_apps;
mod search;
mod services;
pub mod settings;
mod stream;
mod traces;
mod traces_agg;
mod traces_search;
pub mod uptime;
mod usage;
pub mod users;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::FromRef;
use axum::routing::{get, post};
use axum::{middleware, Router};
use axum_extra::extract::cookie::Key;

use crate::users::UserStore;
use photon_core::PhotonError;
use photon_query::{MetricsQueryEngine, QueryEngine, SpanQueryEngine};

pub use data::{DataAdmin, PurgeCommand, PurgeSender, RetentionAtomics};
pub use rum::{RumApi, RumSink};
pub use stream::LiveHub;
pub use usage::{
    signal_from_path, ReplicationStatus, SqliteUsageStore, UsageSampleRow, UsageStore,
};

/// The Photon HTTP server: a query engine, the human user table, and a cookie signing key.
pub struct ApiServer {
    query: QueryEngine,
    span_query: SpanQueryEngine,
    metrics_query: MetricsQueryEngine,
    /// The human UI user store (SQLite-backed in production).
    users: Arc<dyn UserStore>,
    key: Key,
    /// The uptime-vertical REST layer, if enabled. `None` disables the `/api/monitors*` routes
    /// (they 404) — set via [`ApiServer::with_uptime`], which `photon-server` (Task 10) calls.
    uptime: Option<uptime::UptimeApi>,
    /// The data-admin handle (purge channels + retention atomics + settings store). `None`
    /// keeps `/api/retention` + `/api/data/purge` returning 404 — set via
    /// [`ApiServer::with_data_admin`].
    data: Option<data::DataAdmin>,
    /// The live-tail SSE fan-out (broadcast senders + config + connection semaphore). `None`
    /// keeps `/api/stream/logs` + `/api/stream/spans` returning 404 — set via
    /// [`ApiServer::with_live_hub`].
    live: Option<Arc<stream::LiveHub>>,
    /// The usage/storage time-series store (footprint + ingest-counter samples + durable
    /// accounting). `None` makes `/api/usage/series` return an empty series and `/api/storage`
    /// report `durable_bytes=0` — set via [`ApiServer::with_usage`].
    usage: Option<Arc<dyn usage::UsageStore>>,
    /// A live view of the durable replicator (configured + pending queue depth). `None` reports
    /// `durable.configured=false` — set via [`ApiServer::with_usage`].
    replication: Option<Arc<dyn usage::ReplicationStatus>>,
    /// The RUM subsystem (the SQLite-backed app registry + the metrics/logs write sink). `None`
    /// keeps `POST /api/rum` returning 404 — set via [`ApiServer::with_rum`].
    rum: Option<rum::RumApi>,
    /// The alerts vertical (rule/channel store + the scheduler's live-reload command sender +
    /// the `ConditionSource` seam). `None` keeps `/api/alerts/*` returning 404 — set via
    /// [`ApiServer::with_alerts`].
    alerts: Option<alerts::AlertsApi>,
}

/// Shared, immutable application state: a cheap-to-clone handle over an `Arc`. Handlers pull
/// the query engine and the user table from here (via `Deref`); [`Key`] is exposed to the
/// cookie extractors via [`FromRef`].
///
/// A local newtype (rather than a bare `Arc<AppStateInner>`) is required so we can implement
/// the foreign `FromRef` trait for the foreign [`Key`] type without tripping the orphan rule.
#[derive(Clone)]
pub(crate) struct AppState(Arc<AppStateInner>);

pub(crate) struct AppStateInner {
    pub(crate) query: QueryEngine,
    pub(crate) span_query: SpanQueryEngine,
    pub(crate) metrics_query: MetricsQueryEngine,
    pub(crate) users: Arc<dyn UserStore>,
    pub(crate) key: Key,
    pub(crate) uptime: Option<uptime::UptimeApi>,
    pub(crate) data: Option<data::DataAdmin>,
    pub(crate) live: Option<Arc<stream::LiveHub>>,
    pub(crate) usage: Option<Arc<dyn usage::UsageStore>>,
    // Read by the reshaped `GET /api/storage` handler (Task 6, in `data.rs`); only written here.
    #[allow(dead_code)]
    pub(crate) replication: Option<Arc<dyn usage::ReplicationStatus>>,
    /// The RUM subsystem; read by the public `beacon` handler (`rum.rs`). `None` disables `/rum`.
    pub(crate) rum: Option<rum::RumApi>,
    /// The alerts vertical; read by `alerts.rs`. `None` disables `/api/alerts/*`.
    pub(crate) alerts: Option<alerts::AlertsApi>,
}

impl std::ops::Deref for AppState {
    type Target = AppStateInner;
    fn deref(&self) -> &AppStateInner {
        &self.0
    }
}

// Lets `SignedCookieJar` (and the auth middleware) recover the signing key from the state.
impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.key.clone()
    }
}

impl ApiServer {
    /// Build a server from a ready query engine, the user store, and the cookie-signing secret.
    pub fn new(
        query: QueryEngine,
        span_query: SpanQueryEngine,
        metrics_query: MetricsQueryEngine,
        users: Arc<dyn UserStore>,
        session_secret: &str,
    ) -> ApiServer {
        let key = Key::derive_from(session_secret.as_bytes());
        ApiServer {
            query,
            span_query,
            metrics_query,
            users,
            key,
            uptime: None,
            data: None,
            live: None,
            usage: None,
            replication: None,
            rum: None,
            alerts: None,
        }
    }

    /// Attach the uptime-vertical REST layer (built by `photon-server`). Leaving this unset
    /// (`None`) keeps the `/api/monitors*` routes returning 404 — the subsystem is optional.
    pub fn with_uptime(mut self, uptime: Option<uptime::UptimeApi>) -> Self {
        self.uptime = uptime;
        self
    }

    /// Attach the data-admin handle (purge channels + retention atomics + settings store). Leaving
    /// it unset keeps `/api/retention` + `/api/data/purge` returning 404.
    pub fn with_data_admin(mut self, data: Option<data::DataAdmin>) -> Self {
        self.data = data;
        self
    }

    /// Attach the live-tail SSE fan-out (built by `photon-server` from the `BroadcastingWal`
    /// senders + `[live]` config). Leaving it unset keeps `/api/stream/logs` +
    /// `/api/stream/spans` returning 404 — the subsystem is optional.
    pub fn with_live_hub(mut self, hub: LiveHub) -> Self {
        self.live = Some(Arc::new(hub));
        self
    }

    /// Attach the usage/storage time-series store and a live view of the durable replicator
    /// (built by `photon-server`). Leaving it unset keeps `/api/usage/series` empty and
    /// `/api/storage`'s `durable`/`durable_bytes` reporting the not-configured defaults.
    pub fn with_usage(
        mut self,
        usage: Arc<dyn usage::UsageStore>,
        replication: Arc<dyn usage::ReplicationStatus>,
    ) -> Self {
        self.usage = Some(usage);
        self.replication = Some(replication);
        self
    }

    /// Attach the RUM subsystem (the app store-backed registry + write sink), built by
    /// `photon-server`. Leaving it unset keeps `POST /api/rum` returning 404.
    pub fn with_rum(mut self, rum: Option<rum::RumApi>) -> Self {
        self.rum = rum;
        self
    }

    /// Attach the alerts vertical (rule/channel store + the scheduler's live-reload command
    /// sender + the `ConditionSource` seam), built by `photon-server`. Leaving it unset keeps
    /// `/api/alerts/*` returning 404 — the subsystem is optional.
    pub fn with_alerts(mut self, alerts: Option<alerts::AlertsApi>) -> Self {
        self.alerts = alerts;
        self
    }

    /// Bind `addr` and serve until the process is terminated.
    pub async fn serve(self, addr: SocketAddr) -> Result<(), PhotonError> {
        let app = self.into_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| PhotonError::Io(format!("failed to bind {addr}: {e}")))?;
        axum::serve(listener, app)
            .await
            .map_err(|e| PhotonError::Io(format!("server error: {e}")))
    }

    /// Assemble the router: `/api/login` is open; every other `/api/*` route sits behind the
    /// signed-session gate; all remaining GETs fall through to the embedded UI.
    fn into_router(self) -> Router {
        let state = AppState(Arc::new(AppStateInner {
            query: self.query,
            span_query: self.span_query,
            metrics_query: self.metrics_query,
            users: self.users,
            key: self.key,
            uptime: self.uptime,
            data: self.data,
            live: self.live,
            usage: self.usage,
            replication: self.replication,
            rum: self.rum,
            alerts: self.alerts,
        }));

        // Everything except `/api/login` requires a valid signed `photon_session` cookie.
        let protected = Router::new()
            .route("/services", get(search::services))
            .route("/search", post(search::search))
            .route("/fields", get(fields::fields))
            .route("/facet", get(facet::facet))
            .route("/stream/logs", get(stream::stream_logs))
            .route("/stream/spans", get(stream::stream_spans))
            .route("/histogram", get(histogram::histogram))
            .route("/storage", get(data::storage))
            .route("/usage/series", get(usage::usage_series))
            .route(
                "/retention",
                get(data::get_retention).put(data::put_retention),
            )
            .route("/data/purge", post(data::purge))
            .route("/traces/:trace_id", get(traces::get_trace))
            .route("/traces/search", post(traces_search::traces_search))
            .route("/spans/search", post(traces_search::spans_search))
            .route("/traces/fields", get(traces_agg::traces_fields))
            .route("/traces/facet", get(traces_agg::traces_facet))
            .route("/traces/histogram", get(traces_agg::traces_histogram))
            .route("/traces/latency", get(traces_agg::traces_latency))
            .route("/red", get(red::red))
            .route("/services/:service/timeseries", get(services::timeseries))
            .route(
                "/services/:service/dependencies",
                get(services::dependencies),
            )
            .route(
                "/services/:service/settings",
                get(services::get_settings)
                    .put(services::put_settings)
                    .delete(services::delete_settings),
            )
            .route("/metrics/query", post(metrics::query))
            .route("/metrics/catalog", get(metrics::catalog))
            .route("/metrics/metadata/:name", get(metrics::metadata))
            .route("/metrics/labels", get(metrics::labels))
            .route("/infra/hosts", get(infra::hosts))
            .route("/infra/hosts/:host", get(infra::host_detail))
            .route("/infra/hosts/:host/timeseries", get(infra::host_timeseries))
            .route("/rum/apps", get(rum::apps).post(rum::create_app))
            .route(
                "/rum/apps/:name",
                axum::routing::patch(rum::update_app).delete(rum::delete_app),
            )
            .route("/rum/apps/:name/rotate-key", post(rum::rotate_app_key))
            .route("/rum/vitals", get(rum::vitals))
            .route("/rum/vitals/breakdown", get(rum::breakdown))
            .route("/rum/pages", get(rum::pages))
            .route("/rum/pages/detail", get(rum::page_detail))
            .route("/rum/errors", get(rum::errors))
            .route("/rum/errors/facets", get(rum::error_facets))
            .route("/rum/errors/:fingerprint", get(rum::error_detail))
            .route(
                "/monitors",
                get(uptime::list_monitors).post(uptime::create_monitor),
            )
            .route(
                "/monitors/:id",
                get(uptime::get_monitor)
                    .patch(uptime::update_monitor)
                    .delete(uptime::delete_monitor),
            )
            .route("/monitors/:id/pause", post(uptime::pause_monitor))
            .route("/monitors/:id/resume", post(uptime::resume_monitor))
            .route("/monitors/:id/heartbeats", get(uptime::get_heartbeats))
            .route("/monitors/:id/incidents", get(uptime::get_incidents))
            .route(
                "/alerts/rules",
                get(alerts::list_rules).post(alerts::create_rule),
            )
            .route(
                "/alerts/rules/:id",
                get(alerts::get_rule)
                    .patch(alerts::update_rule)
                    .delete(alerts::delete_rule),
            )
            .route("/alerts/rules/:id/test", post(alerts::test_rule))
            .route("/alerts/preview", post(alerts::preview))
            .route(
                "/alerts/channels",
                get(alerts::list_channels).post(alerts::create_channel),
            )
            .route(
                "/alerts/channels/:id",
                get(alerts::get_channel)
                    .patch(alerts::update_channel)
                    .delete(alerts::delete_channel),
            )
            .route("/alerts/channels/:id/test", post(alerts::test_channel))
            .route("/alerts/channels/test", post(alerts::test_channel_draft))
            .route("/alerts/incidents", get(alerts::list_incidents))
            .route("/logout", post(auth::logout))
            .route("/users", get(auth::list_users).post(auth::create_user))
            .route("/users/:username", axum::routing::delete(auth::delete_user))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                auth::require_auth,
            ));

        // Public RUM beacon: no session cookie (browsers can't hold one), per-app key + Origin
        // auth inside the handler. CORS is scoped to just this route — never the authed ones.
        let rum_router = {
            use axum::http::{header, HeaderValue, Method};
            use tower_http::cors::{AllowOrigin, CorsLayer};
            let reg = state.rum.clone();
            let cors = CorsLayer::new()
                .allow_methods([Method::POST, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE])
                .allow_origin(AllowOrigin::predicate(
                    move |origin: &HeaderValue, _parts: &axum::http::request::Parts| match (
                        reg.as_ref(),
                        origin.to_str(),
                    ) {
                        (Some(r), Ok(o)) => r.origin_allowed(o),
                        _ => false,
                    },
                ));
            Router::new().route("/rum", post(rum::beacon)).layer(cors)
        };

        let api = Router::new()
            .route("/login", post(auth::login))
            .route("/setup", post(auth::setup))
            .route("/session", get(auth::session))
            .merge(rum_router)
            .merge(protected);

        Router::new()
            .nest("/api", api)
            .fallback(assets::static_handler)
            // Content-negotiated response compression (gzip/br) over the whole surface: the JSON
            // API responses (a 500-row /api/search shrinks ~15x) and the embedded UI bundle. The
            // default predicate skips SSE (`/api/stream/*`), gRPC, images, and sub-32-byte bodies,
            // so live-tail streaming is untouched. Transparent to clients — no frontend change.
            .layer(tower_http::compression::CompressionLayer::new())
            .with_state(state)
    }
}

// ---------------------------------------------------------------------------
// In-crate test helpers. Exposed at the crate root as `#[cfg(test)] pub(crate)`
// so the per-endpoint handler modules (`fields`, `facet`, `histogram`, `search`)
// can each drive an authenticated request against the same seeded server via
// `crate::test_router()` + `crate::session_cookie(&router)`.
// ---------------------------------------------------------------------------

/// Argon2 PHC hash of `password` with a deterministic (test-only) salt — no OS RNG needed.
#[cfg(test)]
fn hash_password(password: &str) -> String {
    use argon2::password_hash::SaltString;
    use argon2::{Argon2, PasswordHasher};
    let salt = SaltString::encode_b64(b"photon-test-salt").unwrap();
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

/// A ready [`ApiServer`] over an empty temp data dir and the given user store.
#[cfg(test)]
pub(crate) fn test_server_with_users(users: Arc<dyn UserStore>) -> ApiServer {
    use photon_core::schema::LogSchema;
    use photon_core::span_schema::SpanSchema;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.keep();
    let schema = LogSchema::new(&["service.name".to_string()]);
    let query = QueryEngine::new(path.clone(), schema).unwrap();
    let span_schema = SpanSchema::new(&["service.name".to_string()]);
    let span_query = SpanQueryEngine::new(path.clone(), span_schema).unwrap();
    let metric_schema =
        photon_core::metric_schema::MetricSchema::new(&["service.name".to_string()]);
    let metrics_query = MetricsQueryEngine::new(path, metric_schema).unwrap();
    ApiServer::new(
        query,
        span_query,
        metrics_query,
        users,
        "a-long-random-session-signing-secret-value",
    )
}

/// A ready [`ApiServer`] seeded with a single `admin`/`hunter2` user (the default for handler
/// tests in the per-endpoint modules).
#[cfg(test)]
pub(crate) fn test_server() -> ApiServer {
    let store = crate::users::SqliteUserStore::open_in_memory().unwrap();
    store.seed("admin", &hash_password("hunter2"));
    test_server_with_users(Arc::new(store))
}

/// The seeded test server as a routed axum [`Router`], ready for `oneshot` requests.
#[cfg(test)]
pub(crate) fn test_router() -> axum::Router {
    test_server().into_router()
}

/// An [`AppState`] over the seeded test server, carrying the given RUM subsystem. Lets `rum.rs`
/// drive the `beacon` handler directly (it takes `State<AppState>`) with a `FakeSink`-backed
/// [`RumApi`] — or `None` to exercise the disabled (404) path.
#[cfg(test)]
pub(crate) fn test_state_with_rum(rum: Option<rum::RumApi>) -> AppState {
    let s = test_server();
    AppState(Arc::new(AppStateInner {
        query: s.query,
        span_query: s.span_query,
        metrics_query: s.metrics_query,
        users: s.users,
        key: s.key,
        uptime: s.uptime,
        data: s.data,
        live: s.live,
        usage: s.usage,
        replication: s.replication,
        rum,
        alerts: s.alerts,
    }))
}

/// Log in as the seeded admin and return the `photon_session=…` cookie pair for authed requests.
#[cfg(test)]
pub(crate) async fn session_cookie(router: &axum::Router) -> String {
    use tower::ServiceExt;
    let body = serde_json::json!({ "username": "admin", "password": "hunter2" }).to_string();
    let resp = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/login")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let raw = resp
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("login should set a session cookie")
        .to_str()
        .unwrap();
    raw.split(';').next().unwrap().to_string() // just `photon_session=…`
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for `oneshot`

    fn login_body(username: &str, password: &str) -> Body {
        Body::from(format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        ))
    }

    #[tokio::test]
    async fn login_rejects_bad_password() {
        let app = test_server().into_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("content-type", "application/json")
                    .body(login_body("admin", "wrong"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_accepts_correct_password_and_sets_cookie() {
        let app = test_server().into_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("content-type", "application/json")
                    .body(login_body("admin", "hunter2"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cookie = resp
            .headers()
            .get("set-cookie")
            .expect("login should set a session cookie")
            .to_str()
            .unwrap();
        assert!(cookie.contains("photon_session="));
    }

    #[tokio::test]
    async fn protected_route_requires_session() {
        let app = test_server().into_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/services")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// `GET /api/session` on the open route returns a >32-byte JSON body, so the CompressionLayer
    /// engages. Drive it with each `Accept-Encoding` to prove the layer is wired and negotiates.
    async fn session_content_encoding(accept_encoding: Option<&str>) -> Option<String> {
        let app = test_server().into_router();
        let mut req = Request::builder().method("GET").uri("/api/session");
        if let Some(ae) = accept_encoding {
            req = req.header(axum::http::header::ACCEPT_ENCODING, ae);
        }
        let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        resp.headers()
            .get(axum::http::header::CONTENT_ENCODING)
            .map(|v| v.to_str().unwrap().to_string())
    }

    #[tokio::test]
    async fn compresses_json_response_with_gzip() {
        assert_eq!(
            session_content_encoding(Some("gzip")).await.as_deref(),
            Some("gzip")
        );
    }

    #[tokio::test]
    async fn negotiates_brotli_when_offered() {
        assert_eq!(
            session_content_encoding(Some("br")).await.as_deref(),
            Some("br")
        );
    }

    #[tokio::test]
    async fn stays_uncompressed_without_accept_encoding() {
        // Transparent by default: a client that doesn't opt in gets plain JSON, not gzip.
        assert_eq!(session_content_encoding(None).await, None);
    }
}

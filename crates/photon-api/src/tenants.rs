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
use photon_core::{PhotonError, TenantTokenMap};
use rusqlite::{params, Connection};
use serde::Deserialize;
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
        *self.tokens.write().unwrap() = map;
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
    let tenant = Tenant {
        name,
        token: mint_tenant_token(),
        ui_url: body.ui_url,
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
    match api.update(&name, body.ui_url.as_deref()).await {
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
        assert!(
            validate_tenant_name(&"a".repeat(65)).is_err(),
            "too long"
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
}

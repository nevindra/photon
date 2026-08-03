//! Tenant registry: the customer Photon installs allowed to federate into this central node
//! (name + minted bearer token + optional UI link-out), persisted in the shared control-plane
//! SQLite database (`[storage].db_path`). Mirrors the `rum_apps.rs` store pattern exactly — a
//! single `Mutex<Connection>` (low-volume OLTP), WAL mode, `CREATE TABLE IF NOT EXISTS` on open,
//! and an in-memory variant for tests. Errors map to `PhotonError::Io` (SQLite access is I/O) —
//! `PhotonError` is never edited, per project convention.

use async_trait::async_trait;
use photon_core::PhotonError;
use rusqlite::{params, Connection};
use std::sync::Mutex;

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
}

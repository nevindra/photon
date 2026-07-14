//! RUM app registry: the browser apps allowed to POST beacons (`service.name` + public key +
//! Origin allowlist + per-app sampling/rate-limit), persisted in the shared control-plane SQLite
//! database (`[storage].db_path`). Mirrors the `users.rs` store pattern exactly — a single
//! `Mutex<Connection>` (low-volume OLTP), WAL mode, `CREATE TABLE IF NOT EXISTS` on open, and an
//! in-memory variant for tests. Errors map to `PhotonError::Io` (SQLite access is I/O) —
//! `PhotonError` is never edited, per project convention.

use async_trait::async_trait;
use photon_core::PhotonError;
use rusqlite::{params, Connection};
use std::sync::Mutex;

/// One registered browser app. `name` is `service.name` (immutable identity); `key` is the PUBLIC
/// `pk_live_…` client identifier; `allowed_origins` is the CORS + anti-spoof allowlist.
#[derive(Debug, Clone, PartialEq)]
pub struct RumApp {
    pub name: String,
    pub key: String,
    pub allowed_origins: Vec<String>,
    pub sample_rate: f64,
    pub rate_limit: u32,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// Pure field validation, shared by the create/update API handlers (surfaced as `400`). Uniqueness
/// is enforced separately by the SQLite `PRIMARY KEY`/`UNIQUE` constraints.
pub fn validate_app_fields(
    name: &str,
    allowed_origins: &[String],
    sample_rate: f64,
    rate_limit: u32,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("name must not be empty".into());
    }
    if allowed_origins.is_empty() {
        return Err("at least one allowed origin is required".into());
    }
    if allowed_origins.iter().any(|o| o.trim().is_empty()) {
        return Err("allowed origins must not be blank".into());
    }
    if !(0.0..=1.0).contains(&sample_rate) {
        return Err("sample_rate must be within 0.0..=1.0".into());
    }
    if rate_limit == 0 {
        return Err("rate_limit must be greater than 0".into());
    }
    Ok(())
}

/// Persistence boundary for RUM apps. Async so handlers can `.await` it uniformly; the SQLite impl
/// is synchronous under the `Mutex`.
#[async_trait]
pub trait RumAppStore: Send + Sync {
    /// All apps, sorted by name ascending.
    async fn list(&self) -> Result<Vec<RumApp>, PhotonError>;
    /// Insert an app. Errors on a duplicate name (PRIMARY KEY) or key (UNIQUE).
    async fn create(&self, app: &RumApp) -> Result<(), PhotonError>;
    /// Overwrite an app's origins + sampling + rate limit, keyed by name. `false` if name absent.
    async fn update(
        &self,
        name: &str,
        allowed_origins: &[String],
        sample_rate: f64,
        rate_limit: u32,
    ) -> Result<bool, PhotonError>;
    /// Replace an app's key, keyed by name. `false` if name absent.
    async fn rotate_key(&self, name: &str, new_key: &str) -> Result<bool, PhotonError>;
    /// Delete an app by name. `true` if a row was removed.
    async fn delete(&self, name: &str) -> Result<bool, PhotonError>;
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS rum_apps (
    name            TEXT PRIMARY KEY,
    key             TEXT NOT NULL UNIQUE,
    allowed_origins TEXT NOT NULL,
    sample_rate     REAL NOT NULL,
    rate_limit      INTEGER NOT NULL,
    created_at      INTEGER NOT NULL
);
"#;

pub struct SqliteRumAppStore {
    conn: Mutex<Connection>,
}

impl SqliteRumAppStore {
    /// Open (creating parent dirs + file if needed) the shared control-plane DB and ensure the
    /// `rum_apps` table exists. Safe to call alongside the user/uptime stores opening the same
    /// file: WAL mode allows concurrent readers + a single writer, and `CREATE TABLE IF NOT
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
    pub(crate) fn open_in_memory() -> Result<Self, PhotonError> {
        Self::from_conn(Connection::open_in_memory().map_err(err)?)
    }

    fn from_conn(conn: Connection) -> Result<Self, PhotonError> {
        conn.execute_batch(SCHEMA).map_err(err)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

fn row_to_app(r: &rusqlite::Row) -> rusqlite::Result<RumApp> {
    let origins_json: String = r.get(2)?;
    let allowed_origins: Vec<String> = serde_json::from_str(&origins_json).unwrap_or_default();
    Ok(RumApp {
        name: r.get(0)?,
        key: r.get(1)?,
        allowed_origins,
        sample_rate: r.get(3)?,
        rate_limit: r.get::<_, i64>(4)? as u32,
        created_at: r.get(5)?,
    })
}

#[async_trait]
impl RumAppStore for SqliteRumAppStore {
    async fn list(&self) -> Result<Vec<RumApp>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT name,key,allowed_origins,sample_rate,rate_limit,created_at FROM rum_apps ORDER BY name")
            .map_err(err)?;
        let rows = stmt.query_map([], row_to_app).map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }

    async fn create(&self, app: &RumApp) -> Result<(), PhotonError> {
        let origins = serde_json::to_string(&app.allowed_origins).map_err(err)?;
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO rum_apps (name,key,allowed_origins,sample_rate,rate_limit,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![app.name, app.key, origins, app.sample_rate, app.rate_limit as i64, app.created_at],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn update(
        &self,
        name: &str,
        allowed_origins: &[String],
        sample_rate: f64,
        rate_limit: u32,
    ) -> Result<bool, PhotonError> {
        let origins = serde_json::to_string(allowed_origins).map_err(err)?;
        let c = self.conn.lock().unwrap();
        let n = c
            .execute(
                "UPDATE rum_apps SET allowed_origins=?2, sample_rate=?3, rate_limit=?4 WHERE name=?1",
                params![name, origins, sample_rate, rate_limit as i64],
            )
            .map_err(err)?;
        Ok(n > 0)
    }

    async fn rotate_key(&self, name: &str, new_key: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute(
                "UPDATE rum_apps SET key=?2 WHERE name=?1",
                params![name, new_key],
            )
            .map_err(err)?;
        Ok(n > 0)
    }

    async fn delete(&self, name: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute("DELETE FROM rum_apps WHERE name=?1", params![name])
            .map_err(err)?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, key: &str) -> RumApp {
        RumApp {
            name: name.into(),
            key: key.into(),
            allowed_origins: vec!["https://a.example.com".into()],
            sample_rate: 1.0,
            rate_limit: 5000,
            created_at: 0,
        }
    }

    #[tokio::test]
    async fn crud_round_trip_in_memory() {
        let store = SqliteRumAppStore::open_in_memory().unwrap();
        assert!(store.list().await.unwrap().is_empty());

        store.create(&app("web", "pk_web")).await.unwrap();
        store.create(&app("admin", "pk_admin")).await.unwrap();

        // list() is sorted by name.
        let names: Vec<String> = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, vec!["admin".to_string(), "web".to_string()]);

        // update overwrites origins + limits, keyed by name.
        assert!(store
            .update("web", &["https://x".into(), "https://y".into()], 0.5, 100)
            .await
            .unwrap());
        let web = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.name == "web")
            .unwrap();
        assert_eq!(
            web.allowed_origins,
            vec!["https://x".to_string(), "https://y".to_string()]
        );
        assert_eq!(web.sample_rate, 0.5);
        assert_eq!(web.rate_limit, 100);
        assert_eq!(web.key, "pk_web"); // update does not touch the key

        // rotate_key swaps only the key.
        assert!(store.rotate_key("web", "pk_web_2").await.unwrap());
        let web = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.name == "web")
            .unwrap();
        assert_eq!(web.key, "pk_web_2");

        assert!(store.delete("web").await.unwrap());
        assert!(!store.delete("web").await.unwrap()); // already gone
        assert!(!store
            .update("web", &["https://z".into()], 1.0, 1)
            .await
            .unwrap()); // absent -> false
        assert!(!store.rotate_key("web", "pk_x").await.unwrap()); // absent -> false
        assert_eq!(store.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn create_rejects_duplicate_name_and_key() {
        let store = SqliteRumAppStore::open_in_memory().unwrap();
        store.create(&app("web", "pk_1")).await.unwrap();
        assert!(
            store.create(&app("web", "pk_2")).await.is_err(),
            "duplicate name (PRIMARY KEY)"
        );
        assert!(
            store.create(&app("other", "pk_1")).await.is_err(),
            "duplicate key (UNIQUE)"
        );
    }

    #[tokio::test]
    async fn open_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("photon.db");
        let p = path.to_str().unwrap();
        {
            let store = SqliteRumAppStore::open(p).unwrap();
            store.create(&app("web", "pk_web")).await.unwrap();
        }
        let store = SqliteRumAppStore::open(p).unwrap();
        let apps = store.list().await.unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(
            apps[0].allowed_origins,
            vec!["https://a.example.com".to_string()]
        );
    }

    #[test]
    fn validate_rejects_bad_fields() {
        let ok = vec!["https://a".to_string()];
        assert!(validate_app_fields("web", &ok, 1.0, 5000).is_ok());
        assert!(
            validate_app_fields("  ", &ok, 1.0, 5000).is_err(),
            "blank name"
        );
        assert!(
            validate_app_fields("web", &[], 1.0, 5000).is_err(),
            "no origins"
        );
        assert!(
            validate_app_fields("web", &["".to_string()], 1.0, 5000).is_err(),
            "blank origin entry"
        );
        assert!(
            validate_app_fields("web", &ok, 1.5, 5000).is_err(),
            "sample_rate > 1"
        );
        assert!(
            validate_app_fields("web", &ok, -0.1, 5000).is_err(),
            "sample_rate < 0"
        );
        assert!(
            validate_app_fields("web", &ok, 1.0, 0).is_err(),
            "rate_limit 0"
        );
    }
}

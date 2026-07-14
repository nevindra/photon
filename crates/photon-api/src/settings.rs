//! Runtime settings persisted in the shared control-plane SQLite DB (`[storage].db_path`).
//! Currently: per-signal retention days. Mirrors the `users.rs` store pattern (Mutex<Connection>,
//! WAL, CREATE TABLE IF NOT EXISTS, in-memory variant for tests).

use async_trait::async_trait;
use photon_core::PhotonError;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::Mutex;

/// Persistence boundary for runtime settings. Retention is stored per signal
/// ("logs" | "traces" | "metrics" | "uptime") under the key `retention_days.<signal>`.
/// Per-service Apdex latency thresholds (milliseconds) live in a dedicated `service_settings`
/// table keyed by `service.name`.
#[async_trait]
pub trait SettingsStore: Send + Sync {
    async fn get_retention(&self, signal: &str) -> Result<Option<u32>, PhotonError>;
    async fn set_retention(&self, signal: &str, days: u32) -> Result<(), PhotonError>;

    async fn service_apdex_threshold(&self, service: &str) -> Result<Option<u32>, PhotonError>;
    async fn all_apdex_thresholds(&self) -> Result<HashMap<String, u32>, PhotonError>;
    async fn set_apdex_threshold(&self, service: &str, ms: u32) -> Result<(), PhotonError>;
    async fn clear_apdex_threshold(&self, service: &str) -> Result<(), PhotonError>;
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS service_settings (
    service            TEXT PRIMARY KEY,
    apdex_threshold_ms INTEGER NOT NULL
);
"#;

fn retention_key(signal: &str) -> String {
    format!("retention_days.{signal}")
}

pub struct SqliteSettingsStore {
    conn: Mutex<Connection>,
}

impl SqliteSettingsStore {
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

#[async_trait]
impl SettingsStore for SqliteSettingsStore {
    async fn get_retention(&self, signal: &str) -> Result<Option<u32>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT value FROM settings WHERE key=?1")
            .map_err(err)?;
        let mut rows = stmt
            .query_map(params![retention_key(signal)], |r| r.get::<_, String>(0))
            .map_err(err)?;
        match rows.next() {
            Some(v) => Ok(v.map_err(err)?.parse::<u32>().ok()),
            None => Ok(None),
        }
    }
    async fn set_retention(&self, signal: &str, days: u32) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO settings (key,value) VALUES (?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![retention_key(signal), days.to_string()],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn service_apdex_threshold(&self, service: &str) -> Result<Option<u32>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT apdex_threshold_ms FROM service_settings WHERE service=?1")
            .map_err(err)?;
        let mut rows = stmt
            .query_map(params![service], |r| r.get::<_, i64>(0))
            .map_err(err)?;
        match rows.next() {
            Some(v) => Ok(Some(v.map_err(err)? as u32)),
            None => Ok(None),
        }
    }

    async fn all_apdex_thresholds(&self) -> Result<HashMap<String, u32>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT service, apdex_threshold_ms FROM service_settings")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
            })
            .map_err(err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (svc, ms) = row.map_err(err)?;
            out.insert(svc, ms);
        }
        Ok(out)
    }

    async fn set_apdex_threshold(&self, service: &str, ms: u32) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO service_settings (service, apdex_threshold_ms) VALUES (?1, ?2)
             ON CONFLICT(service) DO UPDATE SET apdex_threshold_ms=excluded.apdex_threshold_ms",
            params![service, ms as i64],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn clear_apdex_threshold(&self, service: &str) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "DELETE FROM service_settings WHERE service=?1",
            params![service],
        )
        .map_err(err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retention_round_trips_and_defaults_to_none() {
        let s = SqliteSettingsStore::open_in_memory().unwrap();
        assert_eq!(s.get_retention("logs").await.unwrap(), None);
        s.set_retention("logs", 30).await.unwrap();
        s.set_retention("logs", 45).await.unwrap(); // upsert
        assert_eq!(s.get_retention("logs").await.unwrap(), Some(45));
        assert_eq!(s.get_retention("traces").await.unwrap(), None);
    }

    #[tokio::test]
    async fn apdex_threshold_round_trips_upserts_and_clears() {
        let s = SqliteSettingsStore::open_in_memory().unwrap();
        assert_eq!(s.service_apdex_threshold("checkout").await.unwrap(), None);

        s.set_apdex_threshold("checkout", 500).await.unwrap();
        s.set_apdex_threshold("checkout", 750).await.unwrap(); // upsert
        assert_eq!(
            s.service_apdex_threshold("checkout").await.unwrap(),
            Some(750)
        );

        s.set_apdex_threshold("web", 250).await.unwrap();
        let all = s.all_apdex_thresholds().await.unwrap();
        assert_eq!(all.get("checkout"), Some(&750));
        assert_eq!(all.get("web"), Some(&250));
        assert_eq!(all.len(), 2);

        s.clear_apdex_threshold("checkout").await.unwrap();
        assert_eq!(s.service_apdex_threshold("checkout").await.unwrap(), None);
        assert_eq!(s.all_apdex_thresholds().await.unwrap().len(), 1);
    }
}

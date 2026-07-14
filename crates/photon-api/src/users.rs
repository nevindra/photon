//! User store: the human UI accounts (username + argon2 hash), persisted in the shared
//! control-plane SQLite database (`[storage].db_path`). Mirrors the `photon-uptime` store
//! pattern — a single `Mutex<Connection>` (low-volume OLTP), WAL mode, `CREATE TABLE IF NOT
//! EXISTS` on open, and an in-memory variant for tests. Errors map to `PhotonError::Io`
//! (SQLite access is I/O) — `PhotonError` is never edited, per project convention.

use async_trait::async_trait;
use photon_core::PhotonError;
use rusqlite::{params, Connection};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// One stored user row.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredUser {
    pub username: String,
    pub password_hash: String,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// Persistence boundary for UI users. Async so handlers can `.await` it uniformly; the SQLite
/// impl is synchronous under the `Mutex`.
#[async_trait]
pub trait UserStore: Send + Sync {
    /// Number of users. `0` ⇒ first-run onboarding is required.
    async fn count(&self) -> Result<u64, PhotonError>;
    /// Fetch one user by exact username.
    async fn get(&self, username: &str) -> Result<Option<StoredUser>, PhotonError>;
    /// All users, sorted by username ascending.
    async fn list(&self) -> Result<Vec<StoredUser>, PhotonError>;
    /// Insert a user. Returns an error if the username already exists (PRIMARY KEY conflict).
    async fn create(&self, username: &str, password_hash: &str) -> Result<(), PhotonError>;
    /// Delete a user. Returns `true` if a row was removed.
    async fn delete(&self, username: &str) -> Result<bool, PhotonError>;
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    username      TEXT PRIMARY KEY,
    password_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL
);
"#;

pub struct SqliteUserStore {
    conn: Mutex<Connection>,
}

impl SqliteUserStore {
    /// Open (creating parent dirs + file if needed) the shared control-plane DB and ensure the
    /// `users` table exists. Safe to call alongside the uptime store opening the same file:
    /// WAL mode allows concurrent readers + a single writer, and `CREATE TABLE IF NOT EXISTS`
    /// is idempotent.
    pub fn open(path: &str) -> Result<Self, PhotonError> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
        }
        let conn = Connection::open(path).map_err(err)?;
        // `busy_timeout` matters because auth shares this file with the uptime store (a second
        // writer connection). Without it, a writer that loses the WAL single-writer race gets an
        // immediate `SQLITE_BUSY` (rusqlite's default timeout is 0); 5s makes it wait-and-retry.
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

    /// Test-only synchronous seed (used by `crate::test_server`, which is not async).
    #[cfg(test)]
    pub(crate) fn seed(&self, username: &str, password_hash: &str) {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO users (username,password_hash,created_at) VALUES (?1,?2,0)",
            params![username, password_hash],
        )
        .unwrap();
    }
}

fn row_to_user(r: &rusqlite::Row) -> rusqlite::Result<StoredUser> {
    Ok(StoredUser {
        username: r.get(0)?,
        password_hash: r.get(1)?,
        created_at: r.get(2)?,
    })
}

#[async_trait]
impl UserStore for SqliteUserStore {
    async fn count(&self) -> Result<u64, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .map_err(err)?;
        Ok(n as u64)
    }

    async fn get(&self, username: &str) -> Result<Option<StoredUser>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT username,password_hash,created_at FROM users WHERE username=?1")
            .map_err(err)?;
        let mut rows = stmt
            .query_map(params![username], row_to_user)
            .map_err(err)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(err)?)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<StoredUser>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT username,password_hash,created_at FROM users ORDER BY username")
            .map_err(err)?;
        let rows = stmt.query_map([], row_to_user).map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }

    async fn create(&self, username: &str, password_hash: &str) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO users (username,password_hash,created_at) VALUES (?1,?2,?3)",
            params![username, password_hash, now_ms()],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn delete(&self, username: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute("DELETE FROM users WHERE username=?1", params![username])
            .map_err(err)?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn crud_round_trip_in_memory() {
        let store = SqliteUserStore::open_in_memory().unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
        assert!(store.get("alice").await.unwrap().is_none());

        store.create("alice", "hash-a").await.unwrap();
        store.create("bob", "hash-b").await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);

        let alice = store.get("alice").await.unwrap().unwrap();
        assert_eq!(alice.username, "alice");
        assert_eq!(alice.password_hash, "hash-a");

        // list() is sorted by username.
        let names: Vec<String> = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.username)
            .collect();
        assert_eq!(names, vec!["alice".to_string(), "bob".to_string()]);

        assert!(store.delete("alice").await.unwrap());
        assert!(!store.delete("alice").await.unwrap()); // already gone
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn create_rejects_duplicate_username() {
        let store = SqliteUserStore::open_in_memory().unwrap();
        store.create("alice", "hash-a").await.unwrap();
        assert!(store.create("alice", "hash-a2").await.is_err());
    }

    #[tokio::test]
    async fn open_creates_file_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("photon.db");
        let p = path.to_str().unwrap();
        {
            let store = SqliteUserStore::open(p).unwrap();
            store.create("alice", "hash-a").await.unwrap();
        }
        // Re-open the same file: the user survives.
        let store = SqliteUserStore::open(p).unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
        assert_eq!(
            store.get("alice").await.unwrap().unwrap().password_hash,
            "hash-a"
        );
    }
}

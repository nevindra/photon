//! SQLite-backed `UptimeStore` (rusqlite, bundled). Low-volume OLTP; a single guarded
//! connection is sufficient. All timestamps are Unix ms.

use crate::model::*;
use crate::store::{monitor_from_input, UptimeStats, UptimeStore};
use photon_core::PhotonError;
use rusqlite::{params, Connection, Row};
use std::sync::Mutex;

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Uptime(e.to_string())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS monitors (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, type TEXT NOT NULL, target TEXT NOT NULL,
    interval_secs INTEGER NOT NULL, timeout_secs INTEGER NOT NULL, retries INTEGER NOT NULL,
    http_method TEXT, expect_status TEXT, keyword TEXT,
    ignore_tls INTEGER NOT NULL, follow_redirects INTEGER NOT NULL, webhook_url TEXT,
    enabled INTEGER NOT NULL, last_state TEXT NOT NULL,
    last_check_at INTEGER, last_latency_ms INTEGER,
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS heartbeats (
    id INTEGER PRIMARY KEY AUTOINCREMENT, monitor_id TEXT NOT NULL, ts INTEGER NOT NULL,
    ok INTEGER NOT NULL, latency_ms INTEGER NOT NULL, status_code INTEGER, error TEXT
);
CREATE INDEX IF NOT EXISTS idx_hb_monitor_ts ON heartbeats(monitor_id, ts DESC);
CREATE TABLE IF NOT EXISTS incidents (
    id INTEGER PRIMARY KEY AUTOINCREMENT, monitor_id TEXT NOT NULL,
    started_at INTEGER NOT NULL, ended_at INTEGER, cause TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_inc_monitor ON incidents(monitor_id, started_at DESC);
"#;

fn state_to_str(s: MonitorState) -> &'static str {
    match s {
        MonitorState::Pending => "pending",
        MonitorState::Up => "up",
        MonitorState::Down => "down",
    }
}
fn state_from_str(s: &str) -> MonitorState {
    match s {
        "up" => MonitorState::Up,
        "down" => MonitorState::Down,
        _ => MonitorState::Pending,
    }
}
fn type_to_str(t: CheckType) -> &'static str {
    match t {
        CheckType::Http => "http",
        CheckType::Tcp => "tcp",
        CheckType::Icmp => "icmp",
    }
}
fn type_from_str(s: &str) -> CheckType {
    match s {
        "tcp" => CheckType::Tcp,
        "icmp" => CheckType::Icmp,
        _ => CheckType::Http,
    }
}

impl SqliteStore {
    pub fn open(path: &str) -> Result<Self, PhotonError> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
        }
        let conn = Connection::open(path).map_err(err)?;
        // `busy_timeout`: this DB file is now shared with the auth user store (a second writer
        // connection), so make a writer that loses the WAL single-writer race wait-and-retry
        // rather than fail immediately with `SQLITE_BUSY` (rusqlite's default timeout is 0).
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )
        .map_err(err)?;
        conn.execute_batch(SCHEMA).map_err(err)?;
        // Additive migration: monitors gained `channel_ids` (JSON array) for the alerts bridge.
        // The `let _ =` swallows the "duplicate column" error on already-migrated DBs — the
        // standard rusqlite additive-migration idiom.
        let _ = conn.execute("ALTER TABLE monitors ADD COLUMN channel_ids TEXT", []);
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn insert_monitor(conn: &Connection, m: &Monitor) -> Result<(), PhotonError> {
        let channel_ids = serde_json::to_string(&m.channel_ids).map_err(err)?;
        conn.execute(
            "INSERT OR REPLACE INTO monitors (id,name,type,target,interval_secs,timeout_secs,retries,
                http_method,expect_status,keyword,ignore_tls,follow_redirects,webhook_url,channel_ids,enabled,
                last_state,last_check_at,last_latency_ms,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![m.id, m.name, type_to_str(m.check_type), m.target, m.interval_secs, m.timeout_secs, m.retries,
                m.http_method, m.expect_status, m.keyword, m.ignore_tls as i64, m.follow_redirects as i64, m.webhook_url,
                channel_ids, m.enabled as i64, state_to_str(m.last_state), m.last_check_at, m.last_latency_ms, m.created_at, m.updated_at],
        ).map_err(err)?;
        Ok(())
    }
}

fn row_to_monitor(r: &Row) -> rusqlite::Result<Monitor> {
    Ok(Monitor {
        id: r.get("id")?,
        name: r.get("name")?,
        check_type: type_from_str(&r.get::<_, String>("type")?),
        target: r.get("target")?,
        interval_secs: r.get("interval_secs")?,
        timeout_secs: r.get("timeout_secs")?,
        retries: r.get("retries")?,
        http_method: r.get("http_method")?,
        expect_status: r.get("expect_status")?,
        keyword: r.get("keyword")?,
        ignore_tls: r.get::<_, i64>("ignore_tls")? != 0,
        follow_redirects: r.get::<_, i64>("follow_redirects")? != 0,
        webhook_url: r.get("webhook_url")?,
        // NULL (pre-migration rows) or invalid JSON → empty list, conservatively.
        channel_ids: r
            .get::<_, Option<String>>("channel_ids")?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        enabled: r.get::<_, i64>("enabled")? != 0,
        last_state: state_from_str(&r.get::<_, String>("last_state")?),
        last_check_at: r.get("last_check_at")?,
        last_latency_ms: r.get("last_latency_ms")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

#[async_trait::async_trait]
impl UptimeStore for SqliteStore {
    async fn list_monitors(&self) -> Result<Vec<Monitor>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT * FROM monitors ORDER BY name")
            .map_err(err)?;
        let rows = stmt.query_map([], row_to_monitor).map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }
    async fn get_monitor(&self, id: &str) -> Result<Option<Monitor>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT * FROM monitors WHERE id=?1")
            .map_err(err)?;
        let mut rows = stmt.query_map(params![id], row_to_monitor).map_err(err)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(err)?)),
            None => Ok(None),
        }
    }
    async fn create_monitor(&self, input: MonitorInput) -> Result<Monitor, PhotonError> {
        let m = monitor_from_input(uuid::Uuid::new_v4().to_string(), input, now_ms());
        let c = self.conn.lock().unwrap();
        Self::insert_monitor(&c, &m)?;
        Ok(m)
    }
    async fn update_monitor(
        &self,
        id: &str,
        input: MonitorInput,
    ) -> Result<Option<Monitor>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let existing = {
            let mut stmt = c
                .prepare("SELECT * FROM monitors WHERE id=?1")
                .map_err(err)?;
            let mut rows = stmt.query_map(params![id], row_to_monitor).map_err(err)?;
            match rows.next() {
                Some(r) => r.map_err(err)?,
                None => return Ok(None),
            }
        };
        let mut m = monitor_from_input(id.to_string(), input, now_ms());
        m.created_at = existing.created_at;
        m.last_state = existing.last_state;
        m.last_check_at = existing.last_check_at;
        m.last_latency_ms = existing.last_latency_ms;
        m.enabled = existing.enabled;
        Self::insert_monitor(&c, &m)?;
        Ok(Some(m))
    }
    async fn delete_monitor(&self, id: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM heartbeats WHERE monitor_id=?1", params![id])
            .map_err(err)?;
        c.execute("DELETE FROM incidents WHERE monitor_id=?1", params![id])
            .map_err(err)?;
        let n = c
            .execute("DELETE FROM monitors WHERE id=?1", params![id])
            .map_err(err)?;
        Ok(n > 0)
    }
    async fn set_enabled(&self, id: &str, enabled: bool) -> Result<Option<Monitor>, PhotonError> {
        {
            let c = self.conn.lock().unwrap();
            c.execute(
                "UPDATE monitors SET enabled=?1, updated_at=?2 WHERE id=?3",
                params![enabled as i64, now_ms(), id],
            )
            .map_err(err)?;
        }
        self.get_monitor(id).await
    }
    async fn append_heartbeat(&self, hb: Heartbeat) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute("INSERT INTO heartbeats (monitor_id,ts,ok,latency_ms,status_code,error) VALUES (?1,?2,?3,?4,?5,?6)",
            params![hb.monitor_id, hb.ts, hb.ok as i64, hb.latency_ms, hb.status_code, hb.error]).map_err(err)?;
        Ok(())
    }
    async fn set_monitor_state(
        &self,
        id: &str,
        state: MonitorState,
        at: i64,
        latency_ms: u32,
    ) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "UPDATE monitors SET last_state=?1, last_check_at=?2, last_latency_ms=?3 WHERE id=?4",
            params![state_to_str(state), at, latency_ms, id],
        )
        .map_err(err)?;
        Ok(())
    }
    async fn heartbeats(&self, id: &str, since: i64) -> Result<Vec<Heartbeat>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT monitor_id,ts,ok,latency_ms,status_code,error FROM heartbeats WHERE monitor_id=?1 AND ts>=?2 ORDER BY ts").map_err(err)?;
        let rows = stmt
            .query_map(params![id, since], |r| {
                Ok(Heartbeat {
                    monitor_id: r.get(0)?,
                    ts: r.get(1)?,
                    ok: r.get::<_, i64>(2)? != 0,
                    latency_ms: r.get(3)?,
                    status_code: r.get(4)?,
                    error: r.get(5)?,
                })
            })
            .map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }
    async fn uptime_pct(&self, id: &str, since: i64) -> Result<f64, PhotonError> {
        let c = self.conn.lock().unwrap();
        let (total, up): (i64, i64) = c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(ok),0) FROM heartbeats WHERE monitor_id=?1 AND ts>=?2",
            params![id, since], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        Ok(if total == 0 {
            100.0
        } else {
            up as f64 * 100.0 / total as f64
        })
    }
    async fn open_incident(
        &self,
        id: &str,
        started_at: i64,
        cause: String,
    ) -> Result<i64, PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO incidents (monitor_id,started_at,ended_at,cause) VALUES (?1,?2,NULL,?3)",
            params![id, started_at, cause],
        )
        .map_err(err)?;
        Ok(c.last_insert_rowid())
    }
    async fn open_incident_id(&self, id: &str) -> Result<Option<i64>, PhotonError> {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT id FROM incidents WHERE monitor_id=?1 AND ended_at IS NULL ORDER BY started_at DESC LIMIT 1",
            params![id], |r| r.get(0)).map(Some).or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(err(e)) })
    }
    async fn close_incident(&self, incident_id: i64, ended_at: i64) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "UPDATE incidents SET ended_at=?1 WHERE id=?2",
            params![ended_at, incident_id],
        )
        .map_err(err)?;
        Ok(())
    }
    async fn incidents(&self, id: &str, limit: u32) -> Result<Vec<Incident>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT id,monitor_id,started_at,ended_at,cause FROM incidents WHERE monitor_id=?1 ORDER BY started_at DESC LIMIT ?2").map_err(err)?;
        let rows = stmt
            .query_map(params![id, limit], |r| {
                Ok(Incident {
                    id: r.get(0)?,
                    monitor_id: r.get(1)?,
                    started_at: r.get(2)?,
                    ended_at: r.get(3)?,
                    cause: r.get(4)?,
                })
            })
            .map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }
    async fn prune_heartbeats(&self, before: i64) -> Result<u64, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute("DELETE FROM heartbeats WHERE ts < ?1", params![before])
            .map_err(err)?;
        Ok(n as u64)
    }
    async fn prune_incidents(&self, before: i64) -> Result<u64, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute(
                "DELETE FROM incidents WHERE ended_at IS NOT NULL AND ended_at < ?1",
                params![before],
            )
            .map_err(err)?;
        Ok(n as u64)
    }
    async fn stats(&self) -> Result<UptimeStats, PhotonError> {
        let c = self.conn.lock().unwrap();
        let monitor_count: i64 = c
            .query_row("SELECT COUNT(*) FROM monitors", [], |r| r.get(0))
            .map_err(err)?;
        let heartbeat_count: i64 = c
            .query_row("SELECT COUNT(*) FROM heartbeats", [], |r| r.get(0))
            .map_err(err)?;
        let incident_count: i64 = c
            .query_row("SELECT COUNT(*) FROM incidents", [], |r| r.get(0))
            .map_err(err)?;
        let (oldest, newest): (Option<i64>, Option<i64>) = c
            .query_row("SELECT MIN(ts), MAX(ts) FROM heartbeats", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(err)?;
        Ok(UptimeStats {
            monitor_count: monitor_count as u64,
            heartbeat_count: heartbeat_count as u64,
            incident_count: incident_count as u64,
            oldest_heartbeat_ts: oldest,
            newest_heartbeat_ts: newest,
        })
    }
}

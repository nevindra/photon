//! SQLite-backed `AlertStore` (rusqlite, bundled). Low-volume OLTP; a single guarded connection
//! is sufficient. All timestamps are Unix ms. `condition`/`channel_ids`/`headers` are stored as
//! JSON `TEXT` columns; `severity`/`kind` as their lowercase string form.
use crate::model::*;
use crate::store::{gen_id, AlertStore};
use async_trait::async_trait;
use photon_core::PhotonError;
use rusqlite::{params, Connection, Row};
use std::sync::Mutex;

pub struct SqliteAlertStore {
    conn: Mutex<Connection>,
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Alerts(e.to_string())
}

/// Wrap a `serde_json` decode failure as a `rusqlite::Error` so it can propagate through a
/// row-mapping closure (which must return `rusqlite::Result`).
fn json_err(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS alert_channels (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
  url TEXT NOT NULL, secret TEXT, headers TEXT, config TEXT, created_at INTEGER, updated_at INTEGER);
CREATE TABLE IF NOT EXISTS alert_rules (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT, enabled INTEGER NOT NULL,
  signal TEXT NOT NULL, condition TEXT NOT NULL, for_secs INTEGER NOT NULL,
  interval_secs INTEGER NOT NULL, severity TEXT NOT NULL, channel_ids TEXT NOT NULL,
  created_at INTEGER, updated_at INTEGER);
CREATE TABLE IF NOT EXISTS alert_incidents (
  id INTEGER PRIMARY KEY AUTOINCREMENT, rule_id TEXT NOT NULL, series_key TEXT NOT NULL,
  started_at INTEGER NOT NULL, ended_at INTEGER, peak_value REAL NOT NULL,
  severity TEXT NOT NULL, summary TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_alert_incidents_open ON alert_incidents(rule_id, series_key) WHERE ended_at IS NULL;
"#;

fn severity_to_str(s: Severity) -> &'static str {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
}
fn severity_from_str(s: &str) -> Severity {
    match s {
        "critical" => Severity::Critical,
        "warning" => Severity::Warning,
        _ => Severity::Info,
    }
}
fn kind_to_str(k: ChannelKind) -> &'static str {
    match k {
        ChannelKind::Webhook => "webhook",
        ChannelKind::Discord => "discord",
        ChannelKind::Telegram => "telegram",
    }
}

impl SqliteAlertStore {
    pub fn open(path: &str) -> Result<Self, PhotonError> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
        }
        let conn = Connection::open(path).map_err(err)?;
        // `busy_timeout`: this DB file may be shared with other control-plane stores (auth,
        // uptime, rum_apps), so a writer that loses the WAL single-writer race waits and retries
        // rather than failing immediately with `SQLITE_BUSY` (rusqlite's default timeout is 0).
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )
        .map_err(err)?;
        conn.execute_batch(SCHEMA).map_err(err)?;
        // Additive migration for pre-existing DBs (the `let _ =` swallows the duplicate-column
        // error on already-migrated DBs — the standard rusqlite idiom, mirrors photon-uptime).
        let _ = conn.execute("ALTER TABLE alert_channels ADD COLUMN config TEXT", []);
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn insert_channel(conn: &Connection, ch: &Channel) -> Result<(), PhotonError> {
        let config = serde_json::to_string(&ch.config).map_err(err)?;
        // Keep the legacy columns populated for display/back-compat: `url` = the effective POST
        // endpoint; `secret`/`headers` only meaningful for the Generic kind.
        let (url, secret, headers) = match &ch.config {
            ChannelConfig::Webhook {
                url,
                secret,
                headers,
            } => (
                url.clone(),
                secret.clone(),
                headers
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(err)?,
            ),
            ChannelConfig::Discord { webhook_url } => (webhook_url.clone(), None, None),
            ChannelConfig::Telegram { bot_token, .. } => (
                format!("https://api.telegram.org/bot{bot_token}/sendMessage"),
                None,
                None,
            ),
        };
        conn.execute(
            "INSERT OR REPLACE INTO alert_channels (id,name,kind,url,secret,headers,config,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![ch.id, ch.name, kind_to_str(ch.kind), url, secret, headers, config, ch.created_at, ch.updated_at],
        ).map_err(err)?;
        Ok(())
    }

    fn insert_rule(conn: &Connection, r: &Rule) -> Result<(), PhotonError> {
        let condition = serde_json::to_string(&r.condition).map_err(err)?;
        let channel_ids = serde_json::to_string(&r.channel_ids).map_err(err)?;
        conn.execute(
            "INSERT OR REPLACE INTO alert_rules
                (id,name,description,enabled,signal,condition,for_secs,interval_secs,severity,channel_ids,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![r.id, r.name, r.description, r.enabled as i64, r.condition.signal(), condition,
                r.for_secs, r.interval_secs, severity_to_str(r.severity), channel_ids, r.created_at, r.updated_at],
        ).map_err(err)?;
        Ok(())
    }
}

fn row_to_channel(row: &Row) -> rusqlite::Result<Channel> {
    // `config` is the source of truth; a NULL `config` means a pre-migration row → synthesize a
    // Webhook config from the legacy `url`/`secret`/`headers` columns.
    let config_json: Option<String> = row.get("config")?;
    let config = match config_json {
        Some(j) => serde_json::from_str(&j).map_err(json_err)?,
        None => {
            let headers: Option<String> = row.get("headers")?;
            let headers = headers
                .map(|h| serde_json::from_str(&h))
                .transpose()
                .map_err(json_err)?;
            ChannelConfig::Webhook {
                url: row.get("url")?,
                secret: row.get("secret")?,
                headers,
            }
        }
    };
    Ok(Channel {
        id: row.get("id")?,
        name: row.get("name")?,
        kind: config.kind(),
        config,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_rule(row: &Row) -> rusqlite::Result<Rule> {
    let condition_json: String = row.get("condition")?;
    let condition: Condition = serde_json::from_str(&condition_json).map_err(json_err)?;
    let channel_ids_json: String = row.get("channel_ids")?;
    let channel_ids: Vec<ChannelId> = serde_json::from_str(&channel_ids_json).unwrap_or_default();
    Ok(Rule {
        id: row.get("id")?,
        name: row.get("name")?,
        description: row.get("description")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        condition,
        for_secs: row.get("for_secs")?,
        interval_secs: row.get("interval_secs")?,
        severity: severity_from_str(&row.get::<_, String>("severity")?),
        channel_ids,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_incident(row: &Row) -> rusqlite::Result<Incident> {
    Ok(Incident {
        id: row.get(0)?,
        rule_id: row.get(1)?,
        series_key: row.get(2)?,
        started_at: row.get(3)?,
        ended_at: row.get(4)?,
        peak_value: row.get(5)?,
        severity: severity_from_str(&row.get::<_, String>(6)?),
        summary: row.get(7)?,
    })
}

const INCIDENT_COLS: &str = "id,rule_id,series_key,started_at,ended_at,peak_value,severity,summary";

#[async_trait]
impl AlertStore for SqliteAlertStore {
    // ---- channels ----
    async fn list_channels(&self) -> Result<Vec<Channel>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT * FROM alert_channels ORDER BY name")
            .map_err(err)?;
        let rows = stmt.query_map([], row_to_channel).map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }
    async fn get_channel(&self, id: &str) -> Result<Option<Channel>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT * FROM alert_channels WHERE id=?1")
            .map_err(err)?;
        let mut rows = stmt.query_map(params![id], row_to_channel).map_err(err)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(err)?)),
            None => Ok(None),
        }
    }
    async fn create_channel(&self, input: ChannelInput) -> Result<Channel, PhotonError> {
        let now = now_ms();
        let ch = Channel {
            id: gen_id("ch"),
            name: input.name,
            kind: input.config.kind(),
            config: input.config,
            created_at: now,
            updated_at: now,
        };
        let c = self.conn.lock().unwrap();
        Self::insert_channel(&c, &ch)?;
        Ok(ch)
    }
    async fn update_channel(
        &self,
        id: &str,
        input: ChannelInput,
    ) -> Result<Option<Channel>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let existing = {
            let mut stmt = c
                .prepare("SELECT * FROM alert_channels WHERE id=?1")
                .map_err(err)?;
            let mut rows = stmt.query_map(params![id], row_to_channel).map_err(err)?;
            match rows.next() {
                Some(r) => r.map_err(err)?,
                None => return Ok(None),
            }
        };
        let ch = Channel {
            id: id.to_string(),
            name: input.name,
            kind: input.config.kind(),
            config: input.config,
            created_at: existing.created_at,
            updated_at: now_ms(),
        };
        Self::insert_channel(&c, &ch)?;
        Ok(Some(ch))
    }
    async fn delete_channel(&self, id: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute("DELETE FROM alert_channels WHERE id=?1", params![id])
            .map_err(err)?;
        Ok(n > 0)
    }

    // ---- rules ----
    async fn list_rules(&self) -> Result<Vec<Rule>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT * FROM alert_rules ORDER BY name")
            .map_err(err)?;
        let rows = stmt.query_map([], row_to_rule).map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }
    async fn get_rule(&self, id: &str) -> Result<Option<Rule>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT * FROM alert_rules WHERE id=?1")
            .map_err(err)?;
        let mut rows = stmt.query_map(params![id], row_to_rule).map_err(err)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(err)?)),
            None => Ok(None),
        }
    }
    async fn create_rule(&self, input: RuleInput) -> Result<Rule, PhotonError> {
        let now = now_ms();
        let r = Rule {
            id: gen_id("rule"),
            name: input.name,
            description: input.description,
            enabled: input.enabled,
            condition: input.condition,
            for_secs: input.for_secs,
            interval_secs: input.interval_secs,
            severity: input.severity,
            channel_ids: input.channel_ids,
            created_at: now,
            updated_at: now,
        };
        let c = self.conn.lock().unwrap();
        Self::insert_rule(&c, &r)?;
        Ok(r)
    }
    async fn update_rule(&self, id: &str, input: RuleInput) -> Result<Option<Rule>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let existing = {
            let mut stmt = c
                .prepare("SELECT * FROM alert_rules WHERE id=?1")
                .map_err(err)?;
            let mut rows = stmt.query_map(params![id], row_to_rule).map_err(err)?;
            match rows.next() {
                Some(r) => r.map_err(err)?,
                None => return Ok(None),
            }
        };
        let r = Rule {
            id: id.to_string(),
            name: input.name,
            description: input.description,
            enabled: input.enabled,
            condition: input.condition,
            for_secs: input.for_secs,
            interval_secs: input.interval_secs,
            severity: input.severity,
            channel_ids: input.channel_ids,
            created_at: existing.created_at,
            updated_at: now_ms(),
        };
        Self::insert_rule(&c, &r)?;
        Ok(Some(r))
    }
    async fn delete_rule(&self, id: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute("DELETE FROM alert_rules WHERE id=?1", params![id])
            .map_err(err)?;
        Ok(n > 0)
    }
    async fn set_rule_enabled(&self, id: &str, enabled: bool) -> Result<Option<Rule>, PhotonError> {
        {
            let c = self.conn.lock().unwrap();
            c.execute(
                "UPDATE alert_rules SET enabled=?1, updated_at=?2 WHERE id=?3",
                params![enabled as i64, now_ms(), id],
            )
            .map_err(err)?;
        }
        self.get_rule(id).await
    }

    // ---- incidents ----
    async fn open_incident(
        &self,
        rule_id: &str,
        series_key: &str,
        started_at: i64,
        value: f64,
        severity: Severity,
        summary: &str,
    ) -> Result<i64, PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO alert_incidents (rule_id,series_key,started_at,ended_at,peak_value,severity,summary)
             VALUES (?1,?2,?3,NULL,?4,?5,?6)",
            params![rule_id, series_key, started_at, value, severity_to_str(severity), summary],
        ).map_err(err)?;
        Ok(c.last_insert_rowid())
    }
    async fn bump_incident_peak(&self, incident_id: i64, value: f64) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "UPDATE alert_incidents SET peak_value = MAX(peak_value, ?1) WHERE id=?2",
            params![value, incident_id],
        )
        .map_err(err)?;
        Ok(())
    }
    async fn close_incident(&self, incident_id: i64, ended_at: i64) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "UPDATE alert_incidents SET ended_at=?1 WHERE id=?2",
            params![ended_at, incident_id],
        )
        .map_err(err)?;
        Ok(())
    }
    async fn open_incident_for(
        &self,
        rule_id: &str,
        series_key: &str,
    ) -> Result<Option<i64>, PhotonError> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT id FROM alert_incidents WHERE rule_id=?1 AND series_key=?2 AND ended_at IS NULL
             ORDER BY started_at DESC LIMIT 1",
            params![rule_id, series_key],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| {
            if e == rusqlite::Error::QueryReturnedNoRows {
                Ok(None)
            } else {
                Err(err(e))
            }
        })
    }
    async fn list_open_incidents(&self) -> Result<Vec<Incident>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare(&format!(
                "SELECT {INCIDENT_COLS} FROM alert_incidents WHERE ended_at IS NULL ORDER BY started_at DESC"
            ))
            .map_err(err)?;
        let rows = stmt.query_map([], row_to_incident).map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }
    async fn list_incidents(
        &self,
        status: Option<&str>,
        rule_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Incident>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut sql = format!("SELECT {INCIDENT_COLS} FROM alert_incidents WHERE 1=1");
        match status {
            Some("triggered") => sql.push_str(" AND ended_at IS NULL"),
            Some("resolved") => sql.push_str(" AND ended_at IS NOT NULL"),
            _ => {}
        }
        if rule_id.is_some() {
            sql.push_str(" AND rule_id = ?");
        }
        sql.push_str(" ORDER BY started_at DESC LIMIT ?");
        let mut stmt = c.prepare(&sql).map_err(err)?;
        let rows = if let Some(rid) = rule_id {
            stmt.query_map(params![rid, limit], row_to_incident)
                .map_err(err)?
        } else {
            stmt.query_map(params![limit], row_to_incident)
                .map_err(err)?
        };
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }
    async fn prune_incidents(&self, before: i64) -> Result<u64, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute(
                "DELETE FROM alert_incidents WHERE ended_at IS NOT NULL AND ended_at < ?1",
                params![before],
            )
            .map_err(err)?;
        Ok(n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp() -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "photon-alerts-test-{}.db",
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let path = p.to_string_lossy().into_owned();
        // The counter restarts at 0 on every fresh test-binary invocation, so a leftover file
        // from a *previous* run (same path, same ordinal) would otherwise carry stale rows into
        // this run's "fresh" database — wipe any pre-existing file (+ WAL/SHM sidecars) first.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{path}-wal"));
        let _ = std::fs::remove_file(format!("{path}-shm"));
        path
    }
    fn cond() -> Condition {
        Condition::Metrics(MetricCondition {
            metric_name: "m".into(),
            label_filters: Default::default(),
            group_by: vec![],
            agg: MetricAgg::Avg,
            window_secs: 60,
            cmp: Cmp::Gt,
            threshold: 1.0,
        })
    }
    fn rule_input() -> RuleInput {
        RuleInput {
            name: "r".into(),
            description: None,
            enabled: true,
            condition: cond(),
            for_secs: 0,
            interval_secs: 60,
            severity: Severity::Warning,
            channel_ids: vec![],
        }
    }

    #[tokio::test]
    async fn rule_crud_roundtrips() {
        let s = SqliteAlertStore::open(&tmp()).unwrap();
        let r = s.create_rule(rule_input()).await.unwrap();
        assert_eq!(s.list_rules().await.unwrap().len(), 1);
        assert!(s.get_rule(&r.id).await.unwrap().is_some());
        let off = s.set_rule_enabled(&r.id, false).await.unwrap().unwrap();
        assert!(!off.enabled);
        assert!(s.delete_rule(&r.id).await.unwrap());
        assert!(s.list_rules().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn incident_open_close_and_rebuild() {
        let s = SqliteAlertStore::open(&tmp()).unwrap();
        let id = s
            .open_incident("r1", "host=web-01", 100, 0.94, Severity::Warning, "avg>0.9")
            .await
            .unwrap();
        assert_eq!(
            s.open_incident_for("r1", "host=web-01").await.unwrap(),
            Some(id)
        );
        assert_eq!(s.list_open_incidents().await.unwrap().len(), 1);
        s.bump_incident_peak(id, 0.99).await.unwrap();
        s.close_incident(id, 200).await.unwrap();
        assert_eq!(
            s.open_incident_for("r1", "host=web-01").await.unwrap(),
            None
        );
        assert_eq!(
            s.list_incidents(Some("resolved"), None, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            s.list_incidents(Some("triggered"), None, 10)
                .await
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn channel_crud_roundtrips_all_kinds() {
        let s = SqliteAlertStore::open(&tmp()).unwrap();
        let discord = s
            .create_channel(ChannelInput {
                name: "discord-ops".into(),
                config: ChannelConfig::Discord {
                    webhook_url: "https://discord.com/api/webhooks/1/abc".into(),
                },
            })
            .await
            .unwrap();
        assert_eq!(discord.kind, ChannelKind::Discord);
        let got = s.get_channel(&discord.id).await.unwrap().unwrap();
        assert!(matches!(got.config, ChannelConfig::Discord { .. }));

        let tg = s
            .create_channel(ChannelInput {
                name: "tg".into(),
                config: ChannelConfig::Telegram {
                    bot_token: "1:x".into(),
                    chat_id: "9".into(),
                },
            })
            .await
            .unwrap();
        let got = s.get_channel(&tg.id).await.unwrap().unwrap();
        assert!(matches!(got.config, ChannelConfig::Telegram { .. }));
        assert!(s.delete_channel(&tg.id).await.unwrap());
    }

    #[tokio::test]
    async fn legacy_channel_row_without_config_loads_as_webhook() {
        let path = tmp();
        let s = SqliteAlertStore::open(&path).unwrap();
        // Simulate a pre-migration row: kind='webhook', url/secret/headers set, config NULL.
        {
            let c = s.conn.lock().unwrap();
            c.execute(
                "INSERT INTO alert_channels (id,name,kind,url,secret,headers,created_at,updated_at)
                 VALUES ('c-old','old','webhook','https://legacy/hook','shh','{\"X-A\":\"1\"}',1,1)",
                [],
            )
            .unwrap();
        }
        let got = s.get_channel("c-old").await.unwrap().unwrap();
        match got.config {
            ChannelConfig::Webhook {
                url,
                secret,
                headers,
            } => {
                assert_eq!(url, "https://legacy/hook");
                assert_eq!(secret.as_deref(), Some("shh"));
                assert_eq!(headers, Some(serde_json::json!({"X-A": "1"})));
            }
            _ => panic!("legacy row must decode as Webhook"),
        }
        assert_eq!(got.kind, ChannelKind::Webhook);
    }

    #[tokio::test]
    async fn list_incidents_filters_by_rule_id_and_orders_desc() {
        let s = SqliteAlertStore::open(&tmp()).unwrap();
        let a = s
            .open_incident("r1", "", 100, 1.0, Severity::Info, "s")
            .await
            .unwrap();
        let b = s
            .open_incident("r1", "", 200, 1.0, Severity::Info, "s")
            .await
            .unwrap();
        s.open_incident("r2", "", 150, 1.0, Severity::Info, "s")
            .await
            .unwrap();
        let all_r1 = s.list_incidents(None, Some("r1"), 10).await.unwrap();
        assert_eq!(all_r1.len(), 2);
        assert_eq!(all_r1[0].id, b); // DESC by started_at
        assert_eq!(all_r1[1].id, a);
    }

    #[tokio::test]
    async fn prune_incidents_only_removes_closed_before_cutoff() {
        let s = SqliteAlertStore::open(&tmp()).unwrap();
        let open = s
            .open_incident("r1", "", 100, 1.0, Severity::Info, "s")
            .await
            .unwrap();
        let closed_old = s
            .open_incident("r1", "a", 100, 1.0, Severity::Info, "s")
            .await
            .unwrap();
        s.close_incident(closed_old, 500).await.unwrap();
        let closed_new = s
            .open_incident("r1", "b", 100, 1.0, Severity::Info, "s")
            .await
            .unwrap();
        s.close_incident(closed_new, 5000).await.unwrap();

        let n = s.prune_incidents(1000).await.unwrap();
        assert_eq!(n, 1); // only closed_old (ended_at=500 < 1000)
        let remaining = s.list_incidents(None, None, 10).await.unwrap();
        let remaining_ids: Vec<i64> = remaining.iter().map(|i| i.id).collect();
        assert!(remaining_ids.contains(&open));
        assert!(remaining_ids.contains(&closed_new));
        assert!(!remaining_ids.contains(&closed_old));
    }
}

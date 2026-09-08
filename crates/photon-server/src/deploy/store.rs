//! SQLite-backed `DeployStore` (rusqlite, bundled) — the Docker fleet-management control plane.
//!
//! Follows the other control-plane stores exactly (`photon_api::deployments::SqliteDeploymentStore`,
//! `photon_alerts::store::sqlite`): one `Mutex<Connection>` on the shared `[storage].db_path` file,
//! WAL mode, `busy_timeout` because other stores write to the same file, `CREATE TABLE IF NOT
//! EXISTS` on open, never `ALTER`, and synchronous bodies behind `#[async_trait]`.
//!
//! Every table is a **document table**: the whole struct lives as JSON in a `doc` column beside the
//! few columns a query filters or orders on. Volumes are small (hundreds of hosts, thousands of
//! stacks) and the business rules live in photon-api and the supervisor, not in SQL — so the shape
//! of a row can grow without a migration, and this store must behave identically to
//! `photon_deploy::store::MemStore`, which is what the rest of the workspace tests against. Table
//! names carry the `deploy_` prefix because the file is shared with auth, uptime, alerts, RUM apps,
//! deployments and settings.

use async_trait::async_trait;
use photon_core::PhotonError;
use photon_deploy::model::*;
use photon_deploy::store::{
    audit_matches, DeployStore, ObservedRecord, RegistryCredentialRecord, SecretBlob,
};
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Mutex;

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

/// Wrap a `serde_json` decode failure as a `rusqlite::Error` so it can propagate through a
/// row-mapping closure (which must return `rusqlite::Result`).
fn json_err(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS deploy_hosts (
  host TEXT PRIMARY KEY, id TEXT NOT NULL UNIQUE, token_hash TEXT NOT NULL UNIQUE, doc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS deploy_observed (
  host TEXT PRIMARY KEY, reported_ms INTEGER NOT NULL, doc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS deploy_actions (
  id TEXT PRIMARY KEY, host TEXT NOT NULL, status TEXT NOT NULL, created_ms INTEGER NOT NULL,
  doc TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_deploy_actions_host ON deploy_actions(host, created_ms);
CREATE TABLE IF NOT EXISTS deploy_stacks (
  id TEXT PRIMARY KEY, host TEXT NOT NULL, name TEXT NOT NULL, doc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS deploy_secrets (
  stack_id TEXT PRIMARY KEY, doc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS deploy_templates (
  id TEXT PRIMARY KEY, doc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS deploy_template_versions (
  id TEXT PRIMARY KEY, template_id TEXT NOT NULL, created_ms INTEGER NOT NULL, doc TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_deploy_template_versions_template
  ON deploy_template_versions(template_id, created_ms);
CREATE TABLE IF NOT EXISTS deploy_registries (
  id TEXT PRIMARY KEY, doc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS deploy_registry_credentials (
  id TEXT PRIMARY KEY, doc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS deploy_rollouts (
  id TEXT PRIMARY KEY, updated_ms INTEGER NOT NULL, doc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS deploy_audit (
  id TEXT PRIMARY KEY, ts_ms INTEGER NOT NULL, actor TEXT NOT NULL, action TEXT NOT NULL,
  host TEXT, result TEXT NOT NULL, doc TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_deploy_audit_ts ON deploy_audit(ts_ms);
"#;

pub struct SqliteDeployStore {
    conn: Mutex<Connection>,
}

impl SqliteDeployStore {
    pub fn open(path: &str) -> Result<Self, PhotonError> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
        }
        let conn = Connection::open(path).map_err(err)?;
        // `busy_timeout`: the file is shared with the other control-plane stores, so a writer that
        // loses the WAL single-writer race waits instead of failing with `SQLITE_BUSY`.
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

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A poisoned mutex means a previous holder panicked mid-statement; the connection itself
        // is still usable (SQLite statements are atomic), so recover rather than cascade.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn encode<T: Serialize>(v: &T) -> Result<String, PhotonError> {
    serde_json::to_string(v).map_err(err)
}

fn decode<T: DeserializeOwned>(doc: &str) -> Result<T, PhotonError> {
    serde_json::from_str(doc).map_err(|e| PhotonError::Io(e.to_string()))
}

/// The single row whose `doc` column matches, decoded; `None` when nothing matched.
fn get_doc<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Option<T>, PhotonError> {
    conn.query_row(sql, params, |r| {
        let doc: String = r.get(0)?;
        serde_json::from_str(&doc).map_err(json_err)
    })
    .optional()
    .map_err(err)
}

/// Every matching row's `doc` column, decoded, in the order the SQL says.
fn list_docs<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<T>, PhotonError> {
    let mut stmt = conn.prepare(sql).map_err(err)?;
    let rows = stmt
        .query_map(params, |r| {
            let doc: String = r.get(0)?;
            serde_json::from_str::<T>(&doc).map_err(json_err)
        })
        .map_err(err)?;
    rows.collect::<Result<Vec<T>, _>>().map_err(err)
}

fn action_status_str(s: ActionStatus) -> &'static str {
    match s {
        ActionStatus::Queued => "queued",
        ActionStatus::Running => "running",
        ActionStatus::Done => "done",
        ActionStatus::Failed => "failed",
        ActionStatus::Expired => "expired",
    }
}

#[async_trait]
impl DeployStore for SqliteDeployStore {
    // --- managed hosts ---

    async fn list_hosts(&self) -> Result<Vec<HostRecord>, PhotonError> {
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_hosts ORDER BY host",
            &[],
        )
    }

    async fn get_host(&self, host: &str) -> Result<Option<HostRecord>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_hosts WHERE host = ?1",
            &[&host],
        )
    }

    async fn get_host_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<HostRecord>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_hosts WHERE token_hash = ?1",
            &[&token_hash],
        )
    }

    async fn put_host(&self, rec: &HostRecord) -> Result<(), PhotonError> {
        let conn = self.lock();
        // Under the one connection lock, so the read and the write are one step.
        let mut rec = rec.clone();
        let cur: Option<String> = conn
            .query_row(
                "SELECT doc FROM deploy_hosts WHERE host = ?1",
                params![rec.host],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        if let Some(doc) = cur {
            if let Ok(cur) = decode::<HostRecord>(&doc) {
                rec.desired_generation = rec.desired_generation.max(cur.desired_generation);
            }
        }
        let doc = encode(&rec)?;
        conn.execute(
            "INSERT OR REPLACE INTO deploy_hosts (host, id, token_hash, doc) VALUES (?1,?2,?3,?4)",
            params![rec.host, rec.id, rec.token_hash, doc],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn update_host(
        &self,
        host: &str,
        f: photon_deploy::store::HostEdit,
    ) -> Result<bool, PhotonError> {
        let conn = self.lock();
        let cur: Option<String> = conn
            .query_row(
                "SELECT doc FROM deploy_hosts WHERE host = ?1",
                params![host],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let Some(doc) = cur else { return Ok(false) };
        let mut rec: HostRecord = decode(&doc)?;
        f(&mut rec);
        let doc = encode(&rec)?;
        conn.execute(
            "UPDATE deploy_hosts SET id = ?2, token_hash = ?3, doc = ?4 WHERE host = ?1",
            params![rec.host, rec.id, rec.token_hash, doc],
        )
        .map_err(err)?;
        Ok(true)
    }

    async fn delete_host(&self, host: &str) -> Result<bool, PhotonError> {
        let conn = self.lock();
        conn.execute("DELETE FROM deploy_observed WHERE host = ?1", params![host])
            .map_err(err)?;
        let n = conn
            .execute("DELETE FROM deploy_hosts WHERE host = ?1", params![host])
            .map_err(err)?;
        Ok(n > 0)
    }

    // --- observed inventory ---

    async fn put_observed(&self, rec: &ObservedRecord) -> Result<(), PhotonError> {
        let doc = encode(rec)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_observed (host, reported_ms, doc) VALUES (?1,?2,?3)",
                params![rec.host, rec.reported_ms, doc],
            )
            .map_err(err)?;
        Ok(())
    }

    async fn get_observed(&self, host: &str) -> Result<Option<ObservedRecord>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_observed WHERE host = ?1",
            &[&host],
        )
    }

    // --- one-shot actions ---

    async fn put_action(&self, a: &ActionRecord) -> Result<(), PhotonError> {
        let doc = encode(a)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_actions (id, host, status, created_ms, doc)
                 VALUES (?1,?2,?3,?4,?5)",
                params![a.id, a.host, action_status_str(a.status), a.created_ms, doc],
            )
            .map_err(err)?;
        Ok(())
    }

    async fn get_action(&self, id: &str) -> Result<Option<ActionRecord>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_actions WHERE id = ?1",
            &[&id],
        )
    }

    async fn list_actions(
        &self,
        host: &str,
        limit: usize,
    ) -> Result<Vec<ActionRecord>, PhotonError> {
        // SQLite's LIMIT is signed; `usize::MAX` would wrap. Clamp to i64 for the "no cap" case.
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_actions WHERE host = ?1 ORDER BY created_ms DESC LIMIT ?2",
            &[&host, &limit],
        )
    }

    async fn open_actions(&self, host: &str) -> Result<Vec<ActionRecord>, PhotonError> {
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_actions WHERE host = ?1 AND status IN ('queued','running')
             ORDER BY created_ms ASC",
            &[&host],
        )
    }

    // --- stacks ---

    async fn list_stacks(&self) -> Result<Vec<Stack>, PhotonError> {
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_stacks ORDER BY id",
            &[],
        )
    }

    async fn get_stack(&self, id: &str) -> Result<Option<Stack>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_stacks WHERE id = ?1",
            &[&id],
        )
    }

    async fn put_stack(&self, s: &Stack) -> Result<(), PhotonError> {
        let doc = encode(s)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_stacks (id, host, name, doc) VALUES (?1,?2,?3,?4)",
                params![s.id, s.host, s.name, doc],
            )
            .map_err(err)?;
        Ok(())
    }

    async fn delete_stack(&self, id: &str) -> Result<bool, PhotonError> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM deploy_secrets WHERE stack_id = ?1",
            params![id],
        )
        .map_err(err)?;
        let n = conn
            .execute("DELETE FROM deploy_stacks WHERE id = ?1", params![id])
            .map_err(err)?;
        Ok(n > 0)
    }

    async fn put_secrets(&self, blob: &SecretBlob) -> Result<(), PhotonError> {
        let doc = encode(blob)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_secrets (stack_id, doc) VALUES (?1,?2)",
                params![blob.stack_id, doc],
            )
            .map_err(err)?;
        Ok(())
    }

    async fn get_secrets(&self, stack_id: &str) -> Result<Option<SecretBlob>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_secrets WHERE stack_id = ?1",
            &[&stack_id],
        )
    }

    // --- templates ---

    async fn list_templates(&self) -> Result<Vec<Template>, PhotonError> {
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_templates ORDER BY id",
            &[],
        )
    }

    async fn get_template(&self, id: &str) -> Result<Option<Template>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_templates WHERE id = ?1",
            &[&id],
        )
    }

    async fn put_template(&self, t: &Template) -> Result<(), PhotonError> {
        let doc = encode(t)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_templates (id, doc) VALUES (?1,?2)",
                params![t.id, doc],
            )
            .map_err(err)?;
        Ok(())
    }

    async fn delete_template(&self, id: &str) -> Result<bool, PhotonError> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM deploy_template_versions WHERE template_id = ?1",
            params![id],
        )
        .map_err(err)?;
        let n = conn
            .execute("DELETE FROM deploy_templates WHERE id = ?1", params![id])
            .map_err(err)?;
        Ok(n > 0)
    }

    async fn get_version(&self, id: &str) -> Result<Option<TemplateVersion>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_template_versions WHERE id = ?1",
            &[&id],
        )
    }

    async fn list_versions(&self, template_id: &str) -> Result<Vec<TemplateVersion>, PhotonError> {
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_template_versions WHERE template_id = ?1
             ORDER BY created_ms DESC",
            &[&template_id],
        )
    }

    async fn insert_version(&self, v: &TemplateVersion) -> Result<(), PhotonError> {
        let doc = encode(v)?;
        // Plain INSERT, never REPLACE: a version is immutable once published (R5.4), and the
        // primary key is what enforces it. The constraint failure becomes the same error
        // `MemStore` returns so callers can't tell the two stores apart.
        let res = self.lock().execute(
            "INSERT INTO deploy_template_versions (id, template_id, created_ms, doc)
             VALUES (?1,?2,?3,?4)",
            params![v.id, v.template_id, v.created_ms, doc],
        );
        match res {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(PhotonError::Io(format!(
                    "template version {} already exists",
                    v.id
                )))
            }
            Err(e) => Err(err(e)),
        }
    }

    // --- registries ---

    async fn list_registries(&self) -> Result<Vec<Registry>, PhotonError> {
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_registries ORDER BY id",
            &[],
        )
    }

    async fn put_registry(&self, r: &Registry) -> Result<(), PhotonError> {
        let doc = encode(r)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_registries (id, doc) VALUES (?1,?2)",
                params![r.id, doc],
            )
            .map_err(err)?;
        Ok(())
    }

    async fn delete_registry(&self, id: &str) -> Result<bool, PhotonError> {
        let n = self
            .lock()
            .execute("DELETE FROM deploy_registries WHERE id = ?1", params![id])
            .map_err(err)?;
        Ok(n > 0)
    }

    // --- image registry credentials ---

    async fn list_registry_credentials(
        &self,
    ) -> Result<Vec<RegistryCredentialRecord>, PhotonError> {
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_registry_credentials ORDER BY id",
            &[],
        )
    }

    async fn put_registry_credential(
        &self,
        r: &RegistryCredentialRecord,
    ) -> Result<(), PhotonError> {
        let doc = encode(r)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_registry_credentials (id, doc) VALUES (?1,?2)",
                params![r.credential.id, doc],
            )
            .map_err(err)?;
        Ok(())
    }

    async fn delete_registry_credential(&self, id: &str) -> Result<bool, PhotonError> {
        let n = self
            .lock()
            .execute(
                "DELETE FROM deploy_registry_credentials WHERE id = ?1",
                params![id],
            )
            .map_err(err)?;
        Ok(n > 0)
    }

    // --- rollouts ---

    async fn list_rollouts(&self) -> Result<Vec<Rollout>, PhotonError> {
        list_docs(
            &self.lock(),
            "SELECT doc FROM deploy_rollouts ORDER BY id",
            &[],
        )
    }

    async fn get_rollout(&self, id: &str) -> Result<Option<Rollout>, PhotonError> {
        get_doc(
            &self.lock(),
            "SELECT doc FROM deploy_rollouts WHERE id = ?1",
            &[&id],
        )
    }

    async fn put_rollout(&self, r: &Rollout) -> Result<(), PhotonError> {
        let doc = encode(r)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_rollouts (id, updated_ms, doc) VALUES (?1,?2,?3)",
                params![r.id, r.updated_ms, doc],
            )
            .map_err(err)?;
        Ok(())
    }

    // --- audit ---

    async fn append_audit(&self, e: &AuditEntry) -> Result<(), PhotonError> {
        let doc = encode(e)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO deploy_audit (id, ts_ms, actor, action, host, result, doc)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    e.id,
                    e.ts_ms,
                    e.actor,
                    e.action,
                    e.target.host,
                    e.result.as_str(),
                    doc
                ],
            )
            .map_err(err)?;
        Ok(())
    }

    async fn query_audit(&self, f: &AuditFilter) -> Result<(Vec<AuditEntry>, usize), PhotonError> {
        // Narrow in SQL where a column makes it cheap (a superset is fine — `_` in LIKE is a
        // wildcard, and so on), then let `audit_matches` have the final word so this store and
        // `MemStore` can never disagree on what a filter means. `q` is text search over the whole
        // entry and only exists in Rust.
        let mut sql = String::from("SELECT doc FROM deploy_audit WHERE 1=1");
        let mut args: Vec<Value> = Vec::new();
        if let Some(since) = f.since_ms {
            sql.push_str(" AND ts_ms >= ?");
            args.push(Value::Integer(since));
        }
        if let Some(a) = f.actor.as_deref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND actor = ?");
            args.push(Value::Text(a.to_string()));
        }
        if let Some(r) = f.result {
            sql.push_str(" AND result = ?");
            args.push(Value::Text(r.as_str().to_string()));
        }
        if let Some(h) = f.host.as_deref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND host = ?");
            args.push(Value::Text(h.to_string()));
        }
        if let Some(a) = f.action.as_deref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND (action = ? OR action LIKE ?)");
            args.push(Value::Text(a.to_string()));
            args.push(Value::Text(format!("{a}.%")));
        }
        sql.push_str(" ORDER BY ts_ms DESC");

        let conn = self.lock();
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                let doc: String = r.get(0)?;
                serde_json::from_str::<AuditEntry>(&doc).map_err(json_err)
            })
            .map_err(err)?;
        let mut v: Vec<AuditEntry> = Vec::new();
        for row in rows {
            let e = row.map_err(err)?;
            if audit_matches(&e, f) {
                v.push(e);
            }
        }
        let total = v.len();
        if f.limit > 0 {
            v.truncate(f.limit);
        }
        Ok((v, total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn store() -> SqliteDeployStore {
        SqliteDeployStore::open_in_memory().unwrap()
    }

    fn host(name: &str, token_hash: &str) -> HostRecord {
        HostRecord {
            id: format!("h_{name}"),
            host: name.into(),
            site: Some("divtik".into()),
            token_hash: token_hash.into(),
            requested_capabilities: vec![AgentCapability::Read, AgentCapability::Lifecycle],
            reported_capabilities: vec![AgentCapability::Read],
            agent_version: Some("1.7.0".into()),
            upgrade_to: None,
            poll_secs: 15,
            last_seen_ms: Some(1_000),
            enrolled_ms: 500,
            docker: Some(DockerFacts {
                version: "27.1.1".into(),
                api_version: "1.46".into(),
                storage_driver: "overlay2".into(),
                os: "linux".into(),
                arch: "amd64".into(),
                containers_running: 3,
                containers_total: 5,
                images: 12,
                disk: DockerDisk::default(),
            }),
            host_id: Some("machine-1".into()),
            arch: Some("amd64".into()),
            os: Some("linux".into()),
            labels: BTreeMap::from([("tier".to_string(), "web".to_string())]),
            desired_generation: 3,
        }
    }

    /// Same race as the MemStore test, on the real table: a stale `put_host` cannot lower the
    /// generation, and `update_host` edits under the connection lock.
    #[tokio::test]
    async fn a_stale_put_cannot_lower_the_host_generation() {
        let s = SqliteDeployStore::open_in_memory().unwrap();
        s.put_host(&host("web-1", "t1")).await.unwrap();
        let stale = s.get_host("web-1").await.unwrap().unwrap();
        assert!(s
            .update_host("web-1", Box::new(|r| r.desired_generation += 1))
            .await
            .unwrap());
        let mut stale = stale;
        stale.agent_version = Some("1.8.0".into());
        s.put_host(&stale).await.unwrap();
        let cur = s.get_host("web-1").await.unwrap().unwrap();
        assert_eq!(cur.desired_generation, 4, "the bump survived the stale put");
        assert_eq!(cur.agent_version.as_deref(), Some("1.8.0"));
        assert!(!s
            .update_host("nope", Box::new(|r| r.desired_generation += 1))
            .await
            .unwrap());
    }

    fn observed(name: &str, reported_ms: i64) -> ObservedRecord {
        ObservedRecord {
            host: name.into(),
            reported_ms,
            observed: Observed {
                facts: None,
                containers: vec![Container {
                    id: "c1".into(),
                    name: "web".into(),
                    image: "nginx:1.27".into(),
                    image_id: "sha256:abc".into(),
                    command: "nginx -g 'daemon off;'".into(),
                    state: ContainerState::Running,
                    health: ContainerHealth::Healthy,
                    status_text: "Up 3 days (healthy)".into(),
                    created_ms: 1,
                    started_ms: Some(2),
                    finished_ms: None,
                    exit_code: None,
                    restart_count: 0,
                    restart_policy: "always".into(),
                    ports: vec![ContainerPort {
                        host_ip: Some("0.0.0.0".into()),
                        host_port: Some(80),
                        container_port: 80,
                        protocol: "tcp".into(),
                    }],
                    mounts: vec![],
                    networks: vec!["default".into()],
                    labels: BTreeMap::new(),
                    env_keys: vec!["PATH".into()],
                    env: BTreeMap::new(),
                    compose_project: Some("web".into()),
                    compose_service: Some("web".into()),
                    managed_stack_id: None,
                    cpu_pct: Some(1.5),
                    mem_bytes: Some(1024),
                    mem_limit_bytes: None,
                    net_rx_bps: None,
                    net_tx_bps: None,
                    block_read_bps: None,
                    block_write_bps: None,
                }],
                images: vec![],
                volumes: vec![],
                networks: vec![],
                stacks: vec![ObservedStack {
                    name: "web".into(),
                    applied_generation: Some(3),
                    config_hashes: BTreeMap::from([("web".to_string(), "h1".to_string())]),
                    working_dir: None,
                    compose_file: None,
                }],
                prune_preview: Some(PrunePreview {
                    host: name.into(),
                    images: PruneCategory {
                        count: 2,
                        bytes: 4096,
                    },
                    containers: PruneCategory::default(),
                    volumes: PruneCategory::default(),
                    build_cache: PruneCategory {
                        count: 1,
                        bytes: 512,
                    },
                }),
            },
        }
    }

    fn action(id: &str, host: &str, status: ActionStatus, created_ms: i64) -> ActionRecord {
        ActionRecord {
            id: id.into(),
            host: host.into(),
            kind: ActionKind::Restart,
            container_id: Some("c1".into()),
            container_name: Some("web".into()),
            image: None,
            volume: None,
            args: ActionArgs::default(),
            status,
            actor: "rina".into(),
            created_ms,
            started_ms: None,
            finished_ms: None,
            expires_ms: created_ms + 60_000,
            result: None,
        }
    }

    fn stack(id: &str, host: &str) -> Stack {
        Stack {
            id: id.into(),
            name: format!("stack-{id}"),
            host: host.into(),
            site: None,
            source: StackSource::Compose,
            template_id: None,
            template_name: None,
            template_version: None,
            template_version_id: None,
            status: StackStatus::Running,
            desired_generation: 2,
            applied_generation: Some(2),
            files: vec![StackFile {
                path: "compose.yaml".into(),
                content: "services:\n  web:\n    image: nginx\n".into(),
            }],
            variables: BTreeMap::from([("PORT".to_string(), "80".to_string())]),
            secrets: vec!["DB_PASSWORD".into()],
            deploy_options: DeployOptions::default(),
            services: vec![],
            drift: vec![],
            apm_services: vec!["web".into()],
            acknowledged_warnings: vec![],
            last_error: None,
            created_by: "rina".into(),
            created_ms: 10,
            updated_ms: 20,
            last_applied_ms: Some(20),
            pending_op: Some(StackOp::Up),
            baseline_config_hashes: BTreeMap::new(),
            previous: None,
            drift_since_ms: None,
        }
    }

    fn manifest(id: &str, version: &str) -> TemplateManifest {
        TemplateManifest {
            id: id.into(),
            version: version.into(),
            name: "T".into(),
            description: String::new(),
            icon: String::new(),
            categories: vec![],
            variables: vec![],
            requires: TemplateRequires::default(),
            ports: vec![],
            volumes: vec![],
            health: None,
            observability: TemplateObservability::default(),
        }
    }

    fn version(id: &str, template_id: &str, ver: &str, created_ms: i64) -> TemplateVersion {
        TemplateVersion {
            id: id.into(),
            template_id: template_id.into(),
            version: ver.into(),
            created_ms,
            created_by: "x".into(),
            notes: None,
            deployed_count: 0,
            manifest: manifest(template_id, ver),
            compose_text: "services: {}\n".into(),
        }
    }

    fn template(id: &str) -> Template {
        Template {
            id: id.into(),
            name: "T".into(),
            description: "desc".into(),
            icon: "box".into(),
            categories: vec!["web".into()],
            source: TemplateSource::Local,
            registry_id: None,
            latest_version: "1.0.0".into(),
            versions: vec![],
            updated_ms: 1,
            stacks: 0,
        }
    }

    fn registry(id: &str) -> Registry {
        Registry {
            id: id.into(),
            name: "Photon".into(),
            url: "https://templates.example".into(),
            kind: RegistryKind::Photon,
            last_sync_ms: None,
            template_count: 0,
            status: RegistryStatus::Ok,
            error: None,
        }
    }

    fn rollout(id: &str, updated_ms: i64) -> Rollout {
        Rollout {
            id: id.into(),
            name: "web 1.1".into(),
            template_id: "t".into(),
            template_name: "T".into(),
            template_version_id: "tv_2".into(),
            template_version: "1.1.0".into(),
            previous_version: Some("1.0.0".into()),
            stack_name: "web".into(),
            selector: TargetSelector::Site {
                sites: vec!["divtik".into()],
            },
            strategy: RolloutStrategy::default(),
            state: RolloutState::PausedGate,
            stage: 1,
            stages: 2,
            targets: vec![RolloutTarget {
                host: "web-1".into(),
                site: Some("divtik".into()),
                stage: 1,
                status: TargetStatus::Healthy,
                attempts: 1,
                last_error: None,
                stack_id: Some("s1".into()),
                previous_version: Some("1.0.0".into()),
                agent_status: AgentStatus::Online,
                updated_ms,
                applied_generation: Some(3),
                healthy_since_ms: Some(updated_ms),
            }],
            variables: BTreeMap::new(),
            created_by: "rina".into(),
            created_ms: 1,
            updated_ms,
            started_ms: Some(2),
            finished_ms: None,
            deploy_options: DeployOptions::default(),
            acknowledged_warnings: vec![],
        }
    }

    fn entry(ts: i64, actor: &str, action: &str, host: &str) -> AuditEntry {
        AuditEntry {
            id: format!("a{ts}"),
            ts_ms: ts,
            actor: actor.into(),
            role: Role::Operator,
            action: action.into(),
            target: AuditTarget {
                host: Some(host.into()),
                ..Default::default()
            },
            result: AuditResultKind::Ok,
            detail: Some("exit 0".into()),
            payload_digest: None,
        }
    }

    #[tokio::test]
    async fn hosts_round_trip_and_lookup_by_token_hash() {
        let s = store();
        let a = host("web-1", "hash-a");
        let b = host("web-2", "hash-b");
        s.put_host(&a).await.unwrap();
        s.put_host(&b).await.unwrap();
        assert_eq!(s.get_host("web-1").await.unwrap(), Some(a.clone()));
        assert_eq!(s.get_host("nope").await.unwrap(), None);
        assert_eq!(
            s.get_host_by_token_hash("hash-b").await.unwrap(),
            Some(b.clone())
        );
        assert_eq!(s.get_host_by_token_hash("hash-z").await.unwrap(), None);
        assert_eq!(s.list_hosts().await.unwrap(), vec![a.clone(), b]);

        // put = replace by host name, and the indexed columns follow the doc.
        let mut a2 = a.clone();
        a2.token_hash = "hash-a2".into();
        a2.desired_generation = 9;
        s.put_host(&a2).await.unwrap();
        assert_eq!(s.get_host_by_token_hash("hash-a").await.unwrap(), None);
        assert_eq!(s.get_host_by_token_hash("hash-a2").await.unwrap(), Some(a2));
        assert_eq!(s.list_hosts().await.unwrap().len(), 2);

        s.put_observed(&observed("web-1", 42)).await.unwrap();
        assert!(s.delete_host("web-1").await.unwrap());
        assert!(!s.delete_host("web-1").await.unwrap(), "already gone");
        assert_eq!(s.get_host("web-1").await.unwrap(), None);
        assert_eq!(
            s.get_observed("web-1").await.unwrap(),
            None,
            "deleting a host drops its observed inventory"
        );
        assert_eq!(s.list_hosts().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn observed_round_trip_replaces_by_host() {
        let s = store();
        assert_eq!(s.get_observed("web-1").await.unwrap(), None);
        let o1 = observed("web-1", 1);
        s.put_observed(&o1).await.unwrap();
        assert_eq!(s.get_observed("web-1").await.unwrap(), Some(o1));
        let o2 = observed("web-1", 2);
        s.put_observed(&o2).await.unwrap();
        assert_eq!(s.get_observed("web-1").await.unwrap(), Some(o2));
    }

    #[tokio::test]
    async fn actions_order_newest_first_and_open_oldest_first() {
        let s = store();
        s.put_action(&action("a1", "web-1", ActionStatus::Done, 10))
            .await
            .unwrap();
        s.put_action(&action("a2", "web-1", ActionStatus::Queued, 30))
            .await
            .unwrap();
        s.put_action(&action("a3", "web-1", ActionStatus::Running, 20))
            .await
            .unwrap();
        s.put_action(&action("a4", "web-2", ActionStatus::Queued, 5))
            .await
            .unwrap();
        s.put_action(&action("a5", "web-1", ActionStatus::Expired, 40))
            .await
            .unwrap();

        let ids = |v: Vec<ActionRecord>| v.into_iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(
            ids(s.list_actions("web-1", 10).await.unwrap()),
            vec!["a5", "a2", "a3", "a1"]
        );
        assert_eq!(
            ids(s.list_actions("web-1", 2).await.unwrap()),
            vec!["a5", "a2"],
            "limit applies after the sort"
        );
        assert_eq!(
            ids(s.list_actions("web-1", 0).await.unwrap()),
            Vec::<String>::new()
        );
        assert_eq!(
            ids(s.open_actions("web-1").await.unwrap()),
            vec!["a3", "a2"],
            "queued + running, oldest first, other hosts excluded"
        );
        assert_eq!(
            ids(s.open_actions("web-3").await.unwrap()),
            Vec::<String>::new()
        );

        // put replaces by id: finishing a3 drops it from the open set.
        let mut a3 = s.get_action("a3").await.unwrap().unwrap();
        a3.status = ActionStatus::Done;
        a3.finished_ms = Some(25);
        a3.result = Some("exit 0".into());
        s.put_action(&a3).await.unwrap();
        assert_eq!(s.get_action("a3").await.unwrap(), Some(a3));
        assert_eq!(ids(s.open_actions("web-1").await.unwrap()), vec!["a2"]);
        assert_eq!(s.list_actions("web-1", 10).await.unwrap().len(), 4);
        assert_eq!(s.get_action("zz").await.unwrap(), None);
    }

    #[tokio::test]
    async fn stacks_and_secrets_round_trip() {
        let s = store();
        let s1 = stack("s1", "web-1");
        let s2 = stack("s2", "web-2");
        s.put_stack(&s2).await.unwrap();
        s.put_stack(&s1).await.unwrap();
        assert_eq!(s.get_stack("s1").await.unwrap(), Some(s1.clone()));
        assert_eq!(s.get_stack("nope").await.unwrap(), None);
        assert_eq!(s.list_stacks().await.unwrap(), vec![s1.clone(), s2.clone()]);

        let mut s1b = s1.clone();
        s1b.status = StackStatus::Drifted;
        s1b.desired_generation = 3;
        s1b.previous = Some(Box::new(StackSnapshot {
            template_version_id: None,
            template_version: None,
            files: s1.files.clone(),
            variables: s1.variables.clone(),
            deploy_options: DeployOptions::default(),
            generation: 2,
        }));
        s.put_stack(&s1b).await.unwrap();
        assert_eq!(s.get_stack("s1").await.unwrap(), Some(s1b));
        assert_eq!(s.list_stacks().await.unwrap().len(), 2);

        let blob = SecretBlob {
            stack_id: "s1".into(),
            values: BTreeMap::from([("DB_PASSWORD".to_string(), "bm9uY2U=".to_string())]),
        };
        assert_eq!(s.get_secrets("s1").await.unwrap(), None);
        s.put_secrets(&blob).await.unwrap();
        assert_eq!(s.get_secrets("s1").await.unwrap(), Some(blob.clone()));
        let mut blob2 = blob.clone();
        blob2.values.insert("API_KEY".into(), "eA==".into());
        s.put_secrets(&blob2).await.unwrap();
        assert_eq!(s.get_secrets("s1").await.unwrap(), Some(blob2));

        assert!(s.delete_stack("s1").await.unwrap());
        assert!(!s.delete_stack("s1").await.unwrap());
        assert_eq!(s.get_stack("s1").await.unwrap(), None);
        assert_eq!(
            s.get_secrets("s1").await.unwrap(),
            None,
            "deleting a stack drops its secrets"
        );
        assert_eq!(s.list_stacks().await.unwrap(), vec![s2]);
    }

    #[tokio::test]
    async fn templates_round_trip_and_delete_cascades_to_versions() {
        let s = store();
        let t = template("t");
        let u = template("u");
        s.put_template(&u).await.unwrap();
        s.put_template(&t).await.unwrap();
        assert_eq!(s.get_template("t").await.unwrap(), Some(t.clone()));
        assert_eq!(s.get_template("nope").await.unwrap(), None);
        assert_eq!(
            s.list_templates().await.unwrap(),
            vec![t.clone(), u.clone()]
        );

        s.insert_version(&version("tv_1", "t", "1.0.0", 10))
            .await
            .unwrap();
        s.insert_version(&version("tv_2", "t", "1.1.0", 20))
            .await
            .unwrap();
        s.insert_version(&version("uv_1", "u", "0.1.0", 15))
            .await
            .unwrap();
        assert_eq!(
            s.get_version("tv_1").await.unwrap(),
            Some(version("tv_1", "t", "1.0.0", 10))
        );
        assert_eq!(s.get_version("nope").await.unwrap(), None);
        let ids: Vec<String> = s
            .list_versions("t")
            .await
            .unwrap()
            .into_iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(
            ids,
            vec!["tv_2", "tv_1"],
            "newest first, other templates excluded"
        );

        let mut t2 = t.clone();
        t2.latest_version = "1.1.0".into();
        t2.versions = vec![version("tv_2", "t", "1.1.0", 20).as_ref()];
        s.put_template(&t2).await.unwrap();
        assert_eq!(s.get_template("t").await.unwrap(), Some(t2));

        assert!(s.delete_template("t").await.unwrap());
        assert!(!s.delete_template("t").await.unwrap());
        assert_eq!(s.get_template("t").await.unwrap(), None);
        assert_eq!(s.get_version("tv_1").await.unwrap(), None);
        assert!(s.list_versions("t").await.unwrap().is_empty());
        assert_eq!(
            s.get_version("uv_1").await.unwrap(),
            Some(version("uv_1", "u", "0.1.0", 15)),
            "another template's versions survive"
        );
        assert_eq!(s.list_templates().await.unwrap(), vec![u]);
    }

    #[tokio::test]
    async fn versions_are_insert_only() {
        let s = store();
        let v = version("tv_1", "t", "1.0.0", 0);
        s.insert_version(&v).await.unwrap();
        let err = s.insert_version(&v).await.unwrap_err();
        assert!(
            matches!(&err, PhotonError::Io(m) if m == "template version tv_1 already exists"),
            "got {err:?}"
        );
        // A re-insert with different content is refused too — the id is the identity.
        let mut v2 = v.clone();
        v2.compose_text = "changed".into();
        assert!(s.insert_version(&v2).await.is_err());
        assert_eq!(
            s.get_version("tv_1").await.unwrap(),
            Some(v),
            "the original is untouched"
        );
    }

    #[tokio::test]
    async fn registries_round_trip() {
        let s = store();
        let r1 = registry("r1");
        let r2 = registry("r2");
        s.put_registry(&r2).await.unwrap();
        s.put_registry(&r1).await.unwrap();
        assert_eq!(
            s.list_registries().await.unwrap(),
            vec![r1.clone(), r2.clone()]
        );
        let mut r1b = r1.clone();
        r1b.status = RegistryStatus::Error;
        r1b.error = Some("timeout".into());
        r1b.last_sync_ms = Some(99);
        s.put_registry(&r1b).await.unwrap();
        assert_eq!(s.list_registries().await.unwrap(), vec![r1b, r2.clone()]);
        assert!(s.delete_registry("r1").await.unwrap());
        assert!(!s.delete_registry("r1").await.unwrap());
        assert_eq!(s.list_registries().await.unwrap(), vec![r2]);
    }

    fn credential(id: &str, server: &str) -> RegistryCredentialRecord {
        RegistryCredentialRecord {
            credential: RegistryCredential {
                id: id.into(),
                server: server.into(),
                username: "deploy".into(),
                label: Some("token".into()),
                scope: Some(TargetSelector::Site {
                    sites: vec!["jakarta".into()],
                }),
                created_ms: 1,
                created_by: "admin".into(),
                updated_ms: 1,
            },
            secret_blob: "AAAA".into(),
        }
    }

    #[tokio::test]
    async fn registry_credentials_round_trip() {
        let s = store();
        let c1 = credential("rc1", "registry.gitlab.com");
        let c2 = credential("rc2", "ghcr.io");
        s.put_registry_credential(&c2).await.unwrap();
        s.put_registry_credential(&c1).await.unwrap();
        assert_eq!(
            s.list_registry_credentials().await.unwrap(),
            vec![c1.clone(), c2.clone()]
        );
        let mut c1b = c1.clone();
        c1b.secret_blob = "BBBB".into();
        c1b.credential.scope = None;
        s.put_registry_credential(&c1b).await.unwrap();
        assert_eq!(
            s.list_registry_credentials().await.unwrap(),
            vec![c1b, c2.clone()]
        );
        assert!(s.delete_registry_credential("rc1").await.unwrap());
        assert!(!s.delete_registry_credential("rc1").await.unwrap());
        assert_eq!(s.list_registry_credentials().await.unwrap(), vec![c2]);
    }

    #[tokio::test]
    async fn rollouts_round_trip() {
        let s = store();
        let r1 = rollout("ro_1", 10);
        let r2 = rollout("ro_2", 20);
        s.put_rollout(&r2).await.unwrap();
        s.put_rollout(&r1).await.unwrap();
        assert_eq!(s.get_rollout("ro_1").await.unwrap(), Some(r1.clone()));
        assert_eq!(s.get_rollout("nope").await.unwrap(), None);
        assert_eq!(
            s.list_rollouts().await.unwrap(),
            vec![r1.clone(), r2.clone()]
        );
        let mut r1b = r1.clone();
        r1b.state = RolloutState::Completed;
        r1b.stage = 2;
        r1b.finished_ms = Some(30);
        r1b.updated_ms = 30;
        r1b.targets[0].status = TargetStatus::Healthy;
        r1b.selector = TargetSelector::Labels {
            labels: BTreeMap::from([("tier".to_string(), "web".to_string())]),
        };
        s.put_rollout(&r1b).await.unwrap();
        assert_eq!(s.get_rollout("ro_1").await.unwrap(), Some(r1b));
        assert_eq!(s.list_rollouts().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn audit_filters_by_family_and_text() {
        let s = store();
        s.append_audit(&entry(1, "rina", "container.restart", "web-1"))
            .await
            .unwrap();
        s.append_audit(&entry(2, "rina", "stack.apply", "web-2"))
            .await
            .unwrap();
        s.append_audit(&entry(3, "bob", "stack.destroy", "web-1"))
            .await
            .unwrap();
        let (v, total) = s
            .query_audit(&AuditFilter {
                action: Some("stack".into()),
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(v[0].ts_ms, 3, "newest first");
        let (v, _) = s
            .query_audit(&AuditFilter {
                host: Some("web-1".into()),
                q: Some("RESTART".into()),
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(v.len(), 1);
        let (v, total) = s
            .query_audit(&AuditFilter {
                limit: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!((v.len(), total), (1, 3));
    }

    #[tokio::test]
    async fn audit_filters_every_column() {
        let st = store();
        let s = &st;
        s.append_audit(&entry(1, "rina", "container.restart", "web-1"))
            .await
            .unwrap();
        s.append_audit(&entry(2, "rina", "stack.apply", "web-2"))
            .await
            .unwrap();
        let mut denied = entry(3, "bob", "stack.destroy", "web-1");
        denied.result = AuditResultKind::Denied;
        denied.detail = Some("operator may not destroy".into());
        s.append_audit(&denied).await.unwrap();
        let mut no_host = entry(4, "bob", "template.publish", "");
        no_host.target = AuditTarget {
            template: Some("t".into()),
            ..Default::default()
        };
        s.append_audit(&no_host).await.unwrap();

        let ts = |v: Vec<AuditEntry>| v.into_iter().map(|e| e.ts_ms).collect::<Vec<_>>();
        let q = |f: AuditFilter| async move { s.query_audit(&f).await.unwrap() };

        // The whole log, newest first, `limit: 0` = no limit.
        let (v, total) = q(AuditFilter::default()).await;
        assert_eq!((ts(v), total), (vec![4, 3, 2, 1], 4));

        let (v, _) = q(AuditFilter {
            since_ms: Some(3),
            ..Default::default()
        })
        .await;
        assert_eq!(ts(v), vec![4, 3]);

        let (v, _) = q(AuditFilter {
            actor: Some("rina".into()),
            ..Default::default()
        })
        .await;
        assert_eq!(ts(v), vec![2, 1]);

        // A full action is exact; a family also matches `family.*` but not a prefix of the noun.
        let (v, _) = q(AuditFilter {
            action: Some("stack.apply".into()),
            ..Default::default()
        })
        .await;
        assert_eq!(ts(v), vec![2]);
        let (v, _) = q(AuditFilter {
            action: Some("sta".into()),
            ..Default::default()
        })
        .await;
        assert!(v.is_empty());

        let (v, _) = q(AuditFilter {
            result: Some(AuditResultKind::Denied),
            ..Default::default()
        })
        .await;
        assert_eq!(ts(v), vec![3]);
        let (v, _) = q(AuditFilter {
            result: Some(AuditResultKind::Ok),
            host: Some("web-1".into()),
            ..Default::default()
        })
        .await;
        assert_eq!(ts(v), vec![1]);

        // Empty strings mean "any", exactly as `audit_matches` reads them.
        let (v, total) = q(AuditFilter {
            actor: Some(String::new()),
            action: Some(String::new()),
            host: Some(String::new()),
            q: Some("   ".into()),
            ..Default::default()
        })
        .await;
        assert_eq!((v.len(), total), (4, 4));

        // `q` searches the target JSON and the detail, case-insensitively.
        let (v, _) = q(AuditFilter {
            q: Some("\"template\":\"t\"".into()),
            ..Default::default()
        })
        .await;
        assert_eq!(ts(v), vec![4]);
        let (v, _) = q(AuditFilter {
            q: Some("MAY NOT".into()),
            ..Default::default()
        })
        .await;
        assert_eq!(ts(v), vec![3]);

        // Combined: SQL narrows, Rust decides, total counts before the limit.
        let (v, total) = q(AuditFilter {
            actor: Some("bob".into()),
            action: Some("stack".into()),
            q: Some("web-1".into()),
            limit: 5,
            ..Default::default()
        })
        .await;
        assert_eq!((ts(v), total), (vec![3], 1));
        let (v, total) = q(AuditFilter {
            actor: Some("bob".into()),
            limit: 1,
            ..Default::default()
        })
        .await;
        assert_eq!((ts(v), total), (vec![4], 2));
    }

    #[tokio::test]
    async fn open_on_disk_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("control.db");
        let path = path.to_str().unwrap();
        {
            let s = SqliteDeployStore::open(path).unwrap();
            s.put_host(&host("web-1", "hash-a")).await.unwrap();
            s.append_audit(&entry(1, "rina", "agent.enroll", "web-1"))
                .await
                .unwrap();
        }
        let s = SqliteDeployStore::open(path).unwrap();
        assert_eq!(
            s.get_host("web-1").await.unwrap(),
            Some(host("web-1", "hash-a"))
        );
        let (v, total) = s.query_audit(&AuditFilter::default()).await.unwrap();
        assert_eq!((v.len(), total), (1, 1));
    }
}

//! Deployments: the release timeline, and the store behind it.
//!
//! Rows arrive two ways and they are equals, not a primary and a fallback:
//!
//! - **Detected** — a `(service, environment, tenant, version)` that ingest has never seen before
//!   (`photon_ingest::deployments`). Free, needs no integration, and works for a service whose CI
//!   nobody controls. It knows only *what* and *when*.
//! - **CI** — `POST /api/deployments` from a pipeline. Knows the commit, the author and the message
//!   too, which is what turns "p99 doubled at 14:03" into a change you can go read.
//!
//! Whichever arrives first creates the row and dates it; the other enriches it. `detected_at_ms` is
//! written on insert and **never updated**, because it is the answer to "when did this first
//! appear", and letting a late CI POST overwrite a real first sighting would move a deploy marker
//! away from the moment the graph actually changed. `source` records which side created the row, so
//! the UI can label a detected deployment as auto-detected without pretending it knows a commit.
//!
//! Storage follows `rum_apps.rs` exactly: one `Mutex<Connection>` on the shared control-plane
//! SQLite DB, WAL mode, `CREATE TABLE IF NOT EXISTS` on open, never `ALTER`.

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use photon_core::PhotonError;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Mutex, RwLock};

use crate::AppState;

/// How a row first came into being. Not "which fields it has" — a detected row that CI later
/// enriched keeps `Detected`, because that is still when and how it was first noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentSource {
    Detected,
    Ci,
}

impl DeploymentSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DeploymentSource::Detected => "detected",
            DeploymentSource::Ci => "ci",
        }
    }
    fn parse(s: &str) -> DeploymentSource {
        match s {
            "ci" => DeploymentSource::Ci,
            _ => DeploymentSource::Detected,
        }
    }
}

/// One deployment. `environment` and `tenant` are empty strings, never NULL, when unreported —
/// SQLite treats NULL as distinct from NULL in a UNIQUE index, so nullable columns would let the
/// same unversioned deployment insert forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    pub id: String,
    pub service: String,
    pub environment: String,
    pub tenant: String,
    pub version: String,
    /// Unix milliseconds of the FIRST sighting, by whichever side saw it first. Never updated.
    pub detected_at_ms: i64,
    pub source: DeploymentSource,
    pub commit_sha: Option<String>,
    pub author: Option<String>,
    pub message: Option<String>,
    pub url: Option<String>,
}

/// The four fields a CI pipeline adds. All optional: a pipeline that only knows the sha still
/// improves the row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CiDetails {
    pub commit_sha: Option<String>,
    pub author: Option<String>,
    pub message: Option<String>,
    pub url: Option<String>,
}

/// The identity of a deployment. All four parts, because the same build in staging and prod are two
/// deployments (different times, independently rollback-able), and the same build at two federated
/// Sites likewise.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeploymentKey {
    pub service: String,
    pub environment: String,
    pub tenant: String,
    pub version: String,
}

/// A stable id derived from the identity — FNV-1a 64-bit, the same construction
/// `photon_core::rum::fingerprint` uses, so it is deterministic across platforms and releases.
///
/// Deterministic on purpose: CI can address a deployment without first looking it up, and a replayed
/// POST is the same row rather than a duplicate. It is a convenience, not the uniqueness guarantee —
/// that is the table's `UNIQUE (service, environment, tenant, version)`, so even a hash collision
/// between two different identities would surface as a constraint failure rather than a silent merge.
pub fn deployment_id(key: &DeploymentKey) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for part in [
        key.service.as_str(),
        key.environment.as_str(),
        key.tenant.as_str(),
        key.version.as_str(),
    ] {
        for byte in part.bytes().chain(std::iter::once(0x1f)) {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:016x}")
}

/// Filters for the list query. Empty strings mean "any".
#[derive(Debug, Clone, Default)]
pub struct DeploymentFilter {
    pub service: String,
    pub environment: String,
    pub tenant: String,
    /// Inclusive window in Unix MILLISECONDS. `None` = unbounded on that side.
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}

/// Persistence boundary. Async so handlers `.await` it uniformly; the SQLite impl is synchronous
/// under the `Mutex`, matching every other control-plane store.
#[async_trait]
pub trait DeploymentStore: Send + Sync {
    /// Deployments matching `filter`, most recent first.
    async fn list(&self, filter: &DeploymentFilter) -> Result<Vec<Deployment>, PhotonError>;
    /// Insert if absent. Returns `true` when a row was created. Never updates an existing row —
    /// a second sighting of a version is not news.
    async fn record_detected(&self, key: &DeploymentKey, at_ms: i64) -> Result<bool, PhotonError>;
    /// Insert-or-enrich from CI. Creates with `source = 'ci'` when absent; otherwise writes only
    /// the supplied detail fields and leaves `detected_at_ms` and `source` alone.
    async fn record_ci(
        &self,
        key: &DeploymentKey,
        at_ms: i64,
        details: &CiDetails,
    ) -> Result<Deployment, PhotonError>;
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS deployments (
    id              TEXT PRIMARY KEY,
    service         TEXT NOT NULL,
    environment     TEXT NOT NULL DEFAULT '',
    tenant          TEXT NOT NULL DEFAULT '',
    version         TEXT NOT NULL,
    detected_at_ms  INTEGER NOT NULL,
    source          TEXT NOT NULL,
    commit_sha      TEXT,
    author          TEXT,
    message         TEXT,
    url             TEXT,
    UNIQUE (service, environment, tenant, version)
);
CREATE INDEX IF NOT EXISTS deployments_by_time ON deployments (detected_at_ms);
CREATE INDEX IF NOT EXISTS deployments_by_service ON deployments (service, environment, detected_at_ms);
"#;

const COLUMNS: &str =
    "id,service,environment,tenant,version,detected_at_ms,source,commit_sha,author,message,url";

pub struct SqliteDeploymentStore {
    conn: Mutex<Connection>,
    /// Every identity already in the table. The ingest hot path answers "seen this?" from here and
    /// never touches SQLite for a version it already knows — which is every batch, forever, after
    /// the first one. Seeded from the table at open, so a restart does not re-announce the fleet's
    /// entire current release set as fresh deployments.
    seen: RwLock<HashSet<DeploymentKey>>,
}

impl SqliteDeploymentStore {
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

    pub fn open_in_memory() -> Result<Self, PhotonError> {
        Self::from_conn(Connection::open_in_memory().map_err(err)?)
    }

    fn from_conn(conn: Connection) -> Result<Self, PhotonError> {
        conn.execute_batch(SCHEMA).map_err(err)?;
        let seen = Self::load_keys(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            seen: RwLock::new(seen),
        })
    }

    fn load_keys(conn: &Connection) -> Result<HashSet<DeploymentKey>, PhotonError> {
        let mut stmt = conn
            .prepare("SELECT service,environment,tenant,version FROM deployments")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(DeploymentKey {
                    service: r.get(0)?,
                    environment: r.get(1)?,
                    tenant: r.get(2)?,
                    version: r.get(3)?,
                })
            })
            .map_err(err)?;
        rows.collect::<rusqlite::Result<HashSet<_>>>().map_err(err)
    }

    /// `true` when this identity is already known — the ingest fast path.
    pub fn already_seen(&self, key: &DeploymentKey) -> bool {
        self.seen.read().unwrap().contains(key)
    }

    /// Record a first sighting, synchronously. This is the ingest path's entry point: it is not
    /// `async` because the common case does no I/O at all (the `seen` set answers it), and putting
    /// an `.await` on the trace receiver for something that almost never works would be a cost with
    /// no matching benefit.
    pub fn record_detected_sync(
        &self,
        key: &DeploymentKey,
        at_ms: i64,
    ) -> Result<bool, PhotonError> {
        self.insert_detected(key, at_ms)
    }

    /// The synchronous body behind both `record_detected` and the ingest watcher.
    fn insert_detected(&self, key: &DeploymentKey, at_ms: i64) -> Result<bool, PhotonError> {
        if self.already_seen(key) {
            return Ok(false);
        }
        let id = deployment_id(key);
        let n = {
            let c = self.conn.lock().unwrap();
            c.execute(
                "INSERT OR IGNORE INTO deployments (id,service,environment,tenant,version,detected_at_ms,source) \
                 VALUES (?1,?2,?3,?4,?5,?6,'detected')",
                params![id, key.service, key.environment, key.tenant, key.version, at_ms],
            )
            .map_err(err)?
        };
        // Cache the identity whether or not WE inserted it: `INSERT OR IGNORE` returning 0 means
        // another writer got there first, and it is still seen.
        self.seen.write().unwrap().insert(key.clone());
        Ok(n > 0)
    }

    fn get(&self, key: &DeploymentKey) -> Result<Option<Deployment>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare(&format!(
                "SELECT {COLUMNS} FROM deployments \
                 WHERE service=?1 AND environment=?2 AND tenant=?3 AND version=?4"
            ))
            .map_err(err)?;
        let mut rows = stmt
            .query_map(
                params![key.service, key.environment, key.tenant, key.version],
                row_to_deployment,
            )
            .map_err(err)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(err)?)),
            None => Ok(None),
        }
    }
}

fn row_to_deployment(r: &rusqlite::Row) -> rusqlite::Result<Deployment> {
    let source: String = r.get(6)?;
    Ok(Deployment {
        id: r.get(0)?,
        service: r.get(1)?,
        environment: r.get(2)?,
        tenant: r.get(3)?,
        version: r.get(4)?,
        detected_at_ms: r.get(5)?,
        source: DeploymentSource::parse(&source),
        commit_sha: r.get(7)?,
        author: r.get(8)?,
        message: r.get(9)?,
        url: r.get(10)?,
    })
}

#[async_trait]
impl DeploymentStore for SqliteDeploymentStore {
    async fn list(&self, filter: &DeploymentFilter) -> Result<Vec<Deployment>, PhotonError> {
        // Bound literals only, never string interpolation of caller input — an empty filter field
        // disables its clause rather than matching `''`.
        let sql = format!(
            "SELECT {COLUMNS} FROM deployments \
             WHERE (?1 = '' OR service = ?1) \
               AND (?2 = '' OR environment = ?2) \
               AND (?3 = '' OR tenant = ?3) \
               AND (?4 < 0 OR detected_at_ms >= ?4) \
               AND (?5 < 0 OR detected_at_ms <= ?5) \
             ORDER BY detected_at_ms DESC, service ASC, version ASC"
        );
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare(&sql).map_err(err)?;
        let rows = stmt
            .query_map(
                params![
                    filter.service,
                    filter.environment,
                    filter.tenant,
                    filter.start_ms.unwrap_or(-1),
                    filter.end_ms.unwrap_or(-1),
                ],
                row_to_deployment,
            )
            .map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }

    async fn record_detected(&self, key: &DeploymentKey, at_ms: i64) -> Result<bool, PhotonError> {
        self.insert_detected(key, at_ms)
    }

    async fn record_ci(
        &self,
        key: &DeploymentKey,
        at_ms: i64,
        details: &CiDetails,
    ) -> Result<Deployment, PhotonError> {
        let id = deployment_id(key);
        {
            let c = self.conn.lock().unwrap();
            // `COALESCE(excluded.x, deployments.x)` so a pipeline that only knows the sha does not
            // blank an author some other post already supplied. `detected_at_ms` and `source` are
            // absent from the SET list on purpose: the first sighting keeps both.
            c.execute(
                "INSERT INTO deployments (id,service,environment,tenant,version,detected_at_ms,source,commit_sha,author,message,url) \
                 VALUES (?1,?2,?3,?4,?5,?6,'ci',?7,?8,?9,?10) \
                 ON CONFLICT(service,environment,tenant,version) DO UPDATE SET \
                   commit_sha = COALESCE(excluded.commit_sha, deployments.commit_sha), \
                   author     = COALESCE(excluded.author,     deployments.author), \
                   message    = COALESCE(excluded.message,    deployments.message), \
                   url        = COALESCE(excluded.url,        deployments.url)",
                params![
                    id,
                    key.service,
                    key.environment,
                    key.tenant,
                    key.version,
                    at_ms,
                    details.commit_sha,
                    details.author,
                    details.message,
                    details.url,
                ],
            )
            .map_err(err)?;
        }
        self.seen.write().unwrap().insert(key.clone());
        self.get(key)?
            .ok_or_else(|| PhotonError::Io("deployment vanished after upsert".into()))
    }
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct ListParams {
    #[serde(default)]
    service: String,
    #[serde(default)]
    environment: String,
    #[serde(default)]
    tenant: String,
    /// Epoch NANOSECOND strings, like every other window on this surface. Converted to the
    /// milliseconds the table stores.
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
}

fn ns_to_ms(s: Option<&String>) -> Result<Option<i64>, String> {
    match s {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => v
            .parse::<i64>()
            .map(|ns| Some(ns / 1_000_000))
            .map_err(|_| format!("invalid window bound: {v}")),
    }
}

/// `GET /api/deployments?service&environment&tenant&start&end` — the release timeline, newest
/// first. Every filter is optional; omitting all of them returns the whole fleet's history.
pub(crate) async fn list(State(st): State<AppState>, Query(p): Query<ListParams>) -> Response {
    let Some(store) = st.deployments.as_ref() else {
        return not_configured();
    };
    let (start_ms, end_ms) = match (ns_to_ms(p.start.as_ref()), ns_to_ms(p.end.as_ref())) {
        (Ok(s), Ok(e)) => (s, e),
        (Err(m), _) | (_, Err(m)) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": m }))).into_response()
        }
    };
    let filter = DeploymentFilter {
        service: p.service,
        environment: p.environment,
        tenant: p.tenant,
        start_ms,
        end_ms,
    };
    match store.list(&filter).await {
        Ok(rows) => Json(json!({
            "deployments": rows.iter().map(deployment_json).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct RecordBody {
    // Defaulted rather than required so a missing field lands in our own 400 with a message a
    // pipeline author can act on, instead of axum's opaque 422 deserialization rejection.
    #[serde(default)]
    service: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    environment: String,
    #[serde(default)]
    tenant: String,
    /// Unix milliseconds the pipeline deployed at. Used only when this POST CREATES the row; a row
    /// that already exists keeps its first sighting.
    #[serde(default)]
    deployed_at_ms: Option<i64>,
    #[serde(default)]
    commit_sha: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// `POST /api/deployments` — record a release from CI. Session-authed like every other write on
/// this surface (a pipeline authenticates the same way a person does).
///
/// Idempotent: posting the same `(service, environment, tenant, version)` twice enriches one row
/// rather than creating two, which matters because a retried pipeline step is normal.
pub(crate) async fn record(State(st): State<AppState>, Json(b): Json<RecordBody>) -> Response {
    let Some(store) = st.deployments.as_ref() else {
        return not_configured();
    };
    if b.service.trim().is_empty() || b.version.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "service and version are required" })),
        )
            .into_response();
    }
    let key = DeploymentKey {
        service: b.service.trim().to_string(),
        environment: b.environment.trim().to_string(),
        tenant: b.tenant.trim().to_string(),
        version: b.version.trim().to_string(),
    };
    let at_ms = b.deployed_at_ms.unwrap_or_else(now_ms);
    let details = CiDetails {
        commit_sha: non_blank(b.commit_sha),
        author: non_blank(b.author),
        message: non_blank(b.message),
        url: non_blank(b.url),
    };
    match store.record_ci(&key, at_ms, &details).await {
        Ok(d) => (StatusCode::OK, Json(deployment_json(&d))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// A blank string from a pipeline template that expanded to nothing is not a value — storing `""`
/// would make the UI render an empty author line as though someone had been recorded.
fn non_blank(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "deployments are not configured on this node" })),
    )
        .into_response()
}

fn deployment_json(d: &Deployment) -> Value {
    json!({
        "id": d.id,
        "service": d.service,
        "environment": d.environment,
        "tenant": d.tenant,
        "version": d.version,
        "detected_at_ms": d.detected_at_ms,
        "source": d.source.as_str(),
        "commit_sha": d.commit_sha,
        "author": d.author,
        "message": d.message,
        "url": d.url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(service: &str, env: &str, version: &str) -> DeploymentKey {
        DeploymentKey {
            service: service.into(),
            environment: env.into(),
            tenant: String::new(),
            version: version.into(),
        }
    }

    fn store() -> SqliteDeploymentStore {
        SqliteDeploymentStore::open_in_memory().unwrap()
    }

    #[tokio::test]
    async fn first_sighting_of_a_version_creates_one_row() {
        let s = store();
        assert!(s
            .record_detected(&key("api", "prod", "1.0.0"), 1_000)
            .await
            .unwrap());
        let rows = s.list(&DeploymentFilter::default()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].version, "1.0.0");
        assert_eq!(rows[0].source, DeploymentSource::Detected);
        assert_eq!(rows[0].detected_at_ms, 1_000);
    }

    #[tokio::test]
    async fn repeat_sightings_do_not_insert() {
        let s = store();
        let k = key("api", "prod", "1.0.0");
        assert!(s.record_detected(&k, 1_000).await.unwrap());
        assert!(!s.record_detected(&k, 2_000).await.unwrap());
        assert!(!s.record_detected(&k, 3_000).await.unwrap());
        let rows = s.list(&DeploymentFilter::default()).await.unwrap();
        assert_eq!(rows.len(), 1);
        // The FIRST sighting dates the deploy; later ones are not news and must not move it.
        assert_eq!(rows[0].detected_at_ms, 1_000);
    }

    #[tokio::test]
    async fn ci_payload_enriches_a_detected_row() {
        let s = store();
        let k = key("api", "prod", "1.0.0");
        s.record_detected(&k, 1_000).await.unwrap();
        let d = s
            .record_ci(
                &k,
                9_999,
                &CiDetails {
                    commit_sha: Some("abc123".into()),
                    author: Some("nevindra".into()),
                    message: Some("bump timeout".into()),
                    url: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(d.commit_sha.as_deref(), Some("abc123"));
        assert_eq!(d.author.as_deref(), Some("nevindra"));
        // Enrichment must not move the deploy marker, nor rewrite how the row was found.
        assert_eq!(d.detected_at_ms, 1_000);
        assert_eq!(d.source, DeploymentSource::Detected);
        assert_eq!(s.list(&DeploymentFilter::default()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ci_creates_the_row_when_nothing_saw_it_first() {
        let s = store();
        let d = s
            .record_ci(
                &key("api", "prod", "2.0.0"),
                5_000,
                &CiDetails {
                    commit_sha: Some("def456".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(d.source, DeploymentSource::Ci);
        assert_eq!(d.detected_at_ms, 5_000);
    }

    #[tokio::test]
    async fn a_later_sighting_of_a_ci_created_row_does_not_duplicate_it() {
        // CI usually posts before the first span of the new build arrives.
        let s = store();
        let k = key("api", "prod", "2.0.0");
        s.record_ci(&k, 5_000, &CiDetails::default()).await.unwrap();
        assert!(!s.record_detected(&k, 6_000).await.unwrap());
        let rows = s.list(&DeploymentFilter::default()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detected_at_ms, 5_000);
        assert_eq!(rows[0].source, DeploymentSource::Ci);
    }

    #[tokio::test]
    async fn a_second_ci_post_does_not_blank_fields_it_omits() {
        let s = store();
        let k = key("api", "prod", "3.0.0");
        s.record_ci(
            &k,
            1_000,
            &CiDetails {
                commit_sha: Some("abc".into()),
                author: Some("nevindra".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let d = s
            .record_ci(
                &k,
                2_000,
                &CiDetails {
                    message: Some("late message".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(d.commit_sha.as_deref(), Some("abc"));
        assert_eq!(d.author.as_deref(), Some("nevindra"));
        assert_eq!(d.message.as_deref(), Some("late message"));
    }

    #[tokio::test]
    async fn set_is_seeded_from_sqlite_at_boot() {
        // Without seeding, a restart would re-announce every version currently running as a fresh
        // deployment — a wall of markers on every chart at 03:00.
        let path = tempfile::tempdir().unwrap().path().join("d.db");
        let path = path.to_string_lossy().to_string();
        {
            let s = SqliteDeploymentStore::open(&path).unwrap();
            s.record_detected(&key("api", "prod", "1.0.0"), 1_000)
                .await
                .unwrap();
        }
        let reopened = SqliteDeploymentStore::open(&path).unwrap();
        assert!(reopened.already_seen(&key("api", "prod", "1.0.0")));
        assert!(!reopened.already_seen(&key("api", "prod", "1.0.1")));
        assert!(!reopened
            .record_detected(&key("api", "prod", "1.0.0"), 2_000)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn the_same_version_in_two_environments_is_two_deployments() {
        let s = store();
        s.record_detected(&key("api", "staging", "1.0.0"), 1_000)
            .await
            .unwrap();
        s.record_detected(&key("api", "prod", "1.0.0"), 2_000)
            .await
            .unwrap();
        assert_eq!(s.list(&DeploymentFilter::default()).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn listing_filters_by_service_environment_and_window() {
        let s = store();
        s.record_detected(&key("api", "prod", "1"), 1_000)
            .await
            .unwrap();
        s.record_detected(&key("api", "staging", "1"), 2_000)
            .await
            .unwrap();
        s.record_detected(&key("web", "prod", "1"), 3_000)
            .await
            .unwrap();

        let only_api = s
            .list(&DeploymentFilter {
                service: "api".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(only_api.len(), 2);

        let only_prod = s
            .list(&DeploymentFilter {
                environment: "prod".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(only_prod.len(), 2);

        let windowed = s
            .list(&DeploymentFilter {
                start_ms: Some(2_000),
                end_ms: Some(2_500),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(windowed.len(), 1);
        assert_eq!(windowed[0].environment, "staging");
    }

    #[tokio::test]
    async fn listing_is_newest_first() {
        let s = store();
        s.record_detected(&key("api", "prod", "1"), 1_000)
            .await
            .unwrap();
        s.record_detected(&key("api", "prod", "3"), 3_000)
            .await
            .unwrap();
        s.record_detected(&key("api", "prod", "2"), 2_000)
            .await
            .unwrap();
        let versions: Vec<String> = s
            .list(&DeploymentFilter::default())
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.version)
            .collect();
        assert_eq!(versions, vec!["3", "2", "1"]);
    }

    #[test]
    fn the_id_is_deterministic_and_identity_scoped() {
        let a = deployment_id(&key("api", "prod", "1.0.0"));
        assert_eq!(a, deployment_id(&key("api", "prod", "1.0.0")));
        assert_ne!(a, deployment_id(&key("api", "staging", "1.0.0")));
        assert_ne!(a, deployment_id(&key("web", "prod", "1.0.0")));
        assert_ne!(a, deployment_id(&key("api", "prod", "1.0.1")));
        // The separator matters: without it, ("ap","i") and ("a","pi") would collide.
        assert_ne!(
            deployment_id(&key("ap", "i", "v")),
            deployment_id(&key("a", "pi", "v"))
        );
    }

    #[test]
    fn a_blank_ci_field_is_absent_not_empty() {
        assert_eq!(non_blank(Some("  ".into())), None);
        assert_eq!(non_blank(Some("".into())), None);
        assert_eq!(non_blank(None), None);
        assert_eq!(non_blank(Some(" abc ".into())), Some("abc".into()));
    }

    // --- HTTP surface ---

    async fn request(
        router: axum::Router,
        req: axum::http::Request<axum::body::Body>,
    ) -> (StatusCode, Value) {
        use tower::ServiceExt;
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn get(uri: &str) -> (StatusCode, Value) {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        request(
            router,
            axum::http::Request::builder()
                .uri(uri)
                .header(axum::http::header::COOKIE, cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn post_to(router: axum::Router, cookie: String, body: Value) -> (StatusCode, Value) {
        request(
            router,
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/deployments")
                .header(axum::http::header::COOKIE, cookie)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    #[tokio::test]
    async fn the_routes_require_a_session() {
        use tower::ServiceExt;
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/deployments")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_empty_table_lists_nothing() {
        let (status, v) = get("/api/deployments").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["deployments"], json!([]));
    }

    #[tokio::test]
    async fn a_ci_post_round_trips_through_the_list() {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let (status, posted) = post_to(
            router.clone(),
            cookie.clone(),
            json!({
                "service": "api",
                "environment": "prod",
                "version": "1.4.2",
                "deployed_at_ms": 1_700_000_000_000i64,
                "commit_sha": "abc123",
                "author": "nevindra",
                "message": "bump timeout",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(posted["source"], json!("ci"));
        assert_eq!(posted["commit_sha"], json!("abc123"));

        let (_, listed) = request(
            router,
            axum::http::Request::builder()
                .uri("/api/deployments?service=api")
                .header(axum::http::header::COOKIE, cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(listed["deployments"].as_array().unwrap().len(), 1);
        assert_eq!(listed["deployments"][0]["version"], json!("1.4.2"));
    }

    #[tokio::test]
    async fn a_post_without_a_version_is_a_400() {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let (status, v) = post_to(router, cookie, json!({ "service": "api" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(v["error"].as_str().unwrap().contains("version"));
    }

    #[tokio::test]
    async fn a_malformed_window_bound_is_a_400() {
        let (status, v) = get("/api/deployments?start=yesterday").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(v["error"]
            .as_str()
            .unwrap()
            .contains("invalid window bound"));
    }

    #[tokio::test]
    async fn a_node_with_no_deployments_table_returns_404_not_an_empty_list() {
        // "Not configured" and "nothing deployed" are different facts; an empty list would claim
        // the second when the first is true.
        use tower::ServiceExt;
        let store = crate::users::SqliteUserStore::open_in_memory().unwrap();
        store.seed("admin", &crate::hash_password("hunter2"));
        let router = crate::test_server_with_users(std::sync::Arc::new(store)).into_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/deployments")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn window_bounds_convert_nanoseconds_to_the_milliseconds_the_table_stores() {
        assert_eq!(
            ns_to_ms(Some(&"1700000000000000000".to_string())),
            Ok(Some(1_700_000_000_000))
        );
        assert_eq!(ns_to_ms(None), Ok(None));
        assert_eq!(ns_to_ms(Some(&String::new())), Ok(None));
        assert!(ns_to_ms(Some(&"nope".to_string())).is_err());
    }
}

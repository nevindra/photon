//! Usage & storage-footprint time-series persistence for the `/data` page.
//!
//! A `SqliteUsageStore` (modeled on `settings.rs`) records 60-second footprint + ingest-counter
//! samples (`usage_samples`) and the sizes of successfully-replicated durable objects
//! (`replicated`). The `/api/usage/series` handler differences the cumulative ingest counters into
//! per-bucket rates via the pure `bucket_series` helper.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use rusqlite::{params, Connection};

use crate::AppState;
use photon_core::PhotonError;

/// One 60-second footprint + ingest-counter snapshot for a single signal. `hot_bytes`,
/// `durable_bytes`, `total_rows`, `file_count` are point-in-time levels; `ingest_rows` /
/// `ingest_bytes` are **cumulative** counter snapshots (the API differences them into rates).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct UsageSampleRow {
    pub ts_ms: i64,
    pub signal: String,
    pub hot_bytes: Option<u64>,
    pub durable_bytes: Option<u64>,
    pub total_rows: Option<u64>,
    pub file_count: Option<u64>,
    pub ingest_rows: Option<u64>,  // cumulative counter snapshot
    pub ingest_bytes: Option<u64>, // cumulative counter snapshot
}

/// Persistence boundary for usage samples + durable-replication accounting.
#[async_trait]
pub trait UsageStore: Send + Sync {
    async fn insert_sample(&self, s: &UsageSampleRow) -> Result<(), PhotonError>;
    async fn prune_samples(&self, before_ms: i64) -> Result<(), PhotonError>;
    /// Raw samples in [start_ms, end_ms], ordered by (signal, ts_ms).
    async fn series(&self, start_ms: i64, end_ms: i64) -> Result<Vec<UsageSampleRow>, PhotonError>;
    async fn record_replicated(
        &self,
        path: &str,
        signal: &str,
        bytes: u64,
        ts_ms: i64,
    ) -> Result<(), PhotonError>;
    async fn durable_bytes(&self, signal: &str) -> Result<u64, PhotonError>;
    async fn last_replicated_ms(&self) -> Result<Option<i64>, PhotonError>;
}

/// Server-supplied view of the replicator (photon-api cannot depend on photon-storage).
pub trait ReplicationStatus: Send + Sync {
    fn configured(&self) -> bool;
    fn pending(&self) -> usize;
}

/// Attribute a replicated object path to its signal. `None` = not a parquet file we track.
pub fn signal_from_path(path: &str) -> Option<&'static str> {
    if !path.ends_with(".parquet") {
        return None;
    }
    if path.starts_with("data-spans/") {
        return Some("traces");
    }
    if path.starts_with("data-metrics/") {
        return Some("metrics");
    }
    if path.starts_with("data/") {
        return Some("logs");
    }
    None
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS usage_samples (
    ts_ms         INTEGER NOT NULL,
    signal        TEXT    NOT NULL,
    hot_bytes     INTEGER,
    durable_bytes INTEGER,
    total_rows    INTEGER,
    file_count    INTEGER,
    ingest_rows   INTEGER,
    ingest_bytes  INTEGER
);
CREATE INDEX IF NOT EXISTS ix_usage_samples ON usage_samples (signal, ts_ms);
CREATE TABLE IF NOT EXISTS replicated (
    path   TEXT PRIMARY KEY,
    signal TEXT NOT NULL,
    bytes  INTEGER NOT NULL,
    ts_ms  INTEGER NOT NULL
);
"#;

/// SQLite-backed [`UsageStore`], sharing the control-plane DB pattern of `settings.rs`
/// (`Mutex<Connection>`, WAL, `CREATE TABLE IF NOT EXISTS`, in-memory variant for tests).
pub struct SqliteUsageStore {
    conn: Mutex<Connection>,
}

impl SqliteUsageStore {
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

/// `u64` level → nullable SQLite `INTEGER` (stored as `i64`).
fn opt_i64(v: Option<u64>) -> Option<i64> {
    v.map(|x| x as i64)
}

/// Nullable SQLite `INTEGER` (`i64`) → `u64` level.
fn opt_u64(v: Option<i64>) -> Option<u64> {
    v.map(|x| x as u64)
}

#[async_trait]
impl UsageStore for SqliteUsageStore {
    async fn insert_sample(&self, s: &UsageSampleRow) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO usage_samples
               (ts_ms, signal, hot_bytes, durable_bytes, total_rows, file_count, ingest_rows, ingest_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                s.ts_ms,
                s.signal,
                opt_i64(s.hot_bytes),
                opt_i64(s.durable_bytes),
                opt_i64(s.total_rows),
                opt_i64(s.file_count),
                opt_i64(s.ingest_rows),
                opt_i64(s.ingest_bytes),
            ],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn prune_samples(&self, before_ms: i64) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "DELETE FROM usage_samples WHERE ts_ms < ?1",
            params![before_ms],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn series(&self, start_ms: i64, end_ms: i64) -> Result<Vec<UsageSampleRow>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare(
                "SELECT ts_ms, signal, hot_bytes, durable_bytes, total_rows, file_count,
                        ingest_rows, ingest_bytes
                 FROM usage_samples
                 WHERE ts_ms >= ?1 AND ts_ms <= ?2
                 ORDER BY signal, ts_ms",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![start_ms, end_ms], |r| {
                Ok(UsageSampleRow {
                    ts_ms: r.get(0)?,
                    signal: r.get(1)?,
                    hot_bytes: opt_u64(r.get(2)?),
                    durable_bytes: opt_u64(r.get(3)?),
                    total_rows: opt_u64(r.get(4)?),
                    file_count: opt_u64(r.get(5)?),
                    ingest_rows: opt_u64(r.get(6)?),
                    ingest_bytes: opt_u64(r.get(7)?),
                })
            })
            .map_err(err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(err)?);
        }
        Ok(out)
    }

    async fn record_replicated(
        &self,
        path: &str,
        signal: &str,
        bytes: u64,
        ts_ms: i64,
    ) -> Result<(), PhotonError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT OR IGNORE INTO replicated (path, signal, bytes, ts_ms) VALUES (?1, ?2, ?3, ?4)",
            params![path, signal, bytes as i64, ts_ms],
        )
        .map_err(err)?;
        Ok(())
    }

    async fn durable_bytes(&self, signal: &str) -> Result<u64, PhotonError> {
        let c = self.conn.lock().unwrap();
        let total: i64 = c
            .query_row(
                "SELECT COALESCE(SUM(bytes), 0) FROM replicated WHERE signal=?1",
                params![signal],
                |r| r.get(0),
            )
            .map_err(err)?;
        Ok(total as u64)
    }

    async fn last_replicated_ms(&self) -> Result<Option<i64>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let ts: Option<i64> = c
            .query_row("SELECT MAX(ts_ms) FROM replicated", [], |r| r.get(0))
            .map_err(err)?;
        Ok(ts)
    }
}

/// One bucketed point of the `/usage/series` response for a signal.
#[derive(serde::Serialize)]
pub struct SeriesPoint {
    pub ts: i64,
    pub hot_bytes: Option<u64>,
    pub durable_bytes: Option<u64>,
    pub total_rows: Option<u64>,
    pub ingest_rows: Option<u64>,
    pub ingest_bytes: Option<u64>,
}

/// Group raw samples into fixed-width buckets. Levels = last sample in bucket; ingest_* =
/// positive delta of the cumulative counter vs the previous bucket (None on reset / first bucket).
pub fn bucket_series(
    rows: Vec<UsageSampleRow>,
    start_ms: i64,
    end_ms: i64,
    bucket_ms: i64,
) -> BTreeMap<String, Vec<SeriesPoint>> {
    let bucket_ms = bucket_ms.max(1);
    // signal -> (bucket_idx -> last row seen in that bucket). BTreeMap keeps buckets time-ordered;
    // a later ts in the same bucket overwrites, so we retain the last sample per bucket.
    let mut by_signal: BTreeMap<String, BTreeMap<i64, UsageSampleRow>> = BTreeMap::new();
    for r in rows {
        if r.ts_ms < start_ms || r.ts_ms > end_ms {
            continue;
        }
        let idx = (r.ts_ms - start_ms) / bucket_ms;
        by_signal
            .entry(r.signal.clone())
            .or_default()
            .insert(idx, r);
    }
    fn delta(cur: Option<u64>, prev: Option<u64>) -> Option<u64> {
        match (cur, prev) {
            (Some(c), Some(p)) if c >= p => Some(c - p),
            _ => None,
        }
    }
    let mut out: BTreeMap<String, Vec<SeriesPoint>> = BTreeMap::new();
    for (signal, buckets) in by_signal {
        let mut points = Vec::with_capacity(buckets.len());
        let (mut prev_rows, mut prev_bytes): (Option<u64>, Option<u64>) = (None, None);
        for (idx, row) in buckets {
            let d_rows = delta(row.ingest_rows, prev_rows);
            let d_bytes = delta(row.ingest_bytes, prev_bytes);
            prev_rows = row.ingest_rows; // carry cumulative forward even across a reset (delta was None)
            prev_bytes = row.ingest_bytes;
            points.push(SeriesPoint {
                ts: start_ms + idx * bucket_ms + bucket_ms,
                hot_bytes: row.hot_bytes,
                durable_bytes: row.durable_bytes,
                total_rows: row.total_rows,
                ingest_rows: d_rows,
                ingest_bytes: d_bytes,
            });
        }
        out.insert(signal, points);
    }
    out
}

// ---- GET /usage/series --------------------------------------------------------------------

fn err_json(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": msg.into() })))
}

#[derive(serde::Deserialize)]
pub(crate) struct SeriesParams {
    window: Option<String>,
}

/// Map a window token to `(span_ms, bucket_ms)`. Unknown tokens fall back to `24h`.
fn window_to_bucket(w: &str) -> (i64, i64) {
    match w {
        "1h" => (3_600_000, 60_000),
        "7d" => (7 * 86_400_000, 1_800_000),
        "30d" => (30 * 86_400_000, 7_200_000),
        _ => (86_400_000, 300_000), // "24h" default
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) async fn usage_series(
    State(st): State<AppState>,
    Query(p): Query<SeriesParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let window = p.window.unwrap_or_else(|| "24h".into());
    let (span_ms, bucket_ms) = window_to_bucket(&window);
    let now_ms = now_ms();
    let start_ms = now_ms - span_ms;
    let series = match st.usage.as_ref() {
        Some(u) => {
            let rows = u
                .series(start_ms, now_ms)
                .await
                .map_err(|e| err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            bucket_series(rows, start_ms, now_ms, bucket_ms)
        }
        None => Default::default(),
    };
    Ok(Json(
        serde_json::json!({ "window": window, "bucket_ms": bucket_ms, "series": series }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ts_ms: i64, signal: &str, hot: u64, irows: u64) -> UsageSampleRow {
        UsageSampleRow {
            ts_ms,
            signal: signal.into(),
            hot_bytes: Some(hot),
            durable_bytes: Some(0),
            total_rows: Some(hot / 10),
            file_count: Some(1),
            ingest_rows: Some(irows),
            ingest_bytes: Some(irows * 10),
        }
    }

    #[test]
    fn signal_from_path_maps_prefixes() {
        assert_eq!(signal_from_path("data/seg-1.parquet"), Some("logs"));
        assert_eq!(signal_from_path("data-spans/seg-1.parquet"), Some("traces"));
        assert_eq!(
            signal_from_path("data-metrics/seg-1.parquet"),
            Some("metrics")
        );
        assert_eq!(signal_from_path("data/seg-1.idx"), None); // skip sidecars
        assert_eq!(signal_from_path("other/x.parquet"), None);
    }

    #[tokio::test]
    async fn samples_roundtrip_and_prune() {
        let s = SqliteUsageStore::open_in_memory().unwrap();
        s.insert_sample(&row(1_000, "logs", 100, 10)).await.unwrap();
        s.insert_sample(&row(2_000, "logs", 200, 25)).await.unwrap();
        let all = s.series(0, 10_000).await.unwrap();
        assert_eq!(all.len(), 2);
        s.prune_samples(1_500).await.unwrap(); // drops the ts=1000 row
        assert_eq!(s.series(0, 10_000).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn durable_bytes_sums_unique_paths_per_signal() {
        let s = SqliteUsageStore::open_in_memory().unwrap();
        s.record_replicated("data/a.parquet", "logs", 100, 5)
            .await
            .unwrap();
        s.record_replicated("data/b.parquet", "logs", 50, 6)
            .await
            .unwrap();
        s.record_replicated("data/a.parquet", "logs", 999, 7)
            .await
            .unwrap(); // dup path ignored
        s.record_replicated("data-spans/c.parquet", "traces", 30, 8)
            .await
            .unwrap();
        assert_eq!(s.durable_bytes("logs").await.unwrap(), 150);
        assert_eq!(s.durable_bytes("traces").await.unwrap(), 30);
        assert_eq!(s.last_replicated_ms().await.unwrap(), Some(8));
    }

    #[test]
    fn bucket_series_computes_deltas_with_reset_and_gap() {
        // Two buckets of width 1000ms. Cumulative ingest_rows: 10 -> 25 -> (reset) 3.
        let rows = vec![
            row(500, "logs", 100, 10),
            row(1500, "logs", 200, 25), // bucket 1..2 delta = 25-10 = 15
            row(2500, "logs", 300, 3),  // reset (3 < 25) -> null
        ];
        let out = bucket_series(rows, 0, 3000, 1000);
        let logs = out.get("logs").unwrap();
        assert_eq!(logs.len(), 3);
        assert_eq!(logs[0].ingest_rows, None); // first bucket: no previous
        assert_eq!(logs[1].ingest_rows, Some(15));
        assert_eq!(logs[2].ingest_rows, None); // counter reset -> gap
        assert_eq!(logs[1].hot_bytes, Some(200)); // level = last-in-bucket
    }
}

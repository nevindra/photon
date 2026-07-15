//! Task A6 end-to-end: a browser beacon POSTed to `POST /api/rum` becomes a queryable metric row
//! (a Web Vital) and a queryable log row (a JS error). This proves the whole RUM ingest spine
//! (config → `photon_core::rum` mappers → `photon_api::RumApi` handler → `RumSink` → the existing
//! metrics + logs WALs → compactor → query engines) is wired end to end.
//!
//! Two independent proofs:
//!   * `rum_beacon_http_auth` boots the real `ApiServer` (RUM registered) on a loopback port and
//!     asserts the public beacon's per-app-key + Origin auth over the wire (204 on a good beacon,
//!     403 on a bogus key or a disallowed Origin).
//!   * `rum_beacon_maps_to_queryable_rows` drives the deterministic no-network round trip: a
//!     beacon is mapped by `photon_core::rum`, written through the *same* `RumSink` the server
//!     uses, drained by the real compactors into hot Parquet, and then queried — asserting the LCP
//!     vital surfaces as a `web_vitals.lcp` metric point carrying the posted value and the JS error
//!     surfaces as an ERROR log row for `service.name = web`.
//!
//! `RumWalSink` lives in the server BINARY (`main.rs`) and cannot be imported from a test crate, so
//! this file defines its own equivalent `TestRumSink` (same body as A5's sink) generic over the WAL
//! type. `spawn_live_server`/`free_addr`/`wait_until_connectable` in the sibling `e2e.rs` are also
//! not importable across test binaries, so the pieces we need are copied here.

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arrow::array::{Array, Float64Array, StringArray};

use photon_api::rum_apps::{RumApp, RumAppStore, SqliteRumAppStore};
use photon_api::users::{SqliteUserStore, UserStore};
use photon_api::{ApiServer, RumApi, RumSink};

use photon_compact::{Compactor, MetricsCompactor};
use photon_core::config::{StorageConfig, WalConfig};
use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
use photon_core::metric_schema::{metric_type, MetricSchema};
use photon_core::record::{LogRecord, RecordBatchBuilder};
use photon_core::rum::{self, Beacon};
use photon_core::schema::{self, LogSchema};
use photon_core::span_schema::SpanSchema;
use photon_core::PhotonError;
use photon_query::{MetricsQueryEngine, QueryEngine, QueryRequest, SpanQueryEngine};
use photon_storage::{Replicator, Storage};
use photon_wal::{DiskWal, Wal};

// The one registered RUM app. Its `name` ("web") becomes `service.name` on every row; `key` is the
// public app-key the beacon carries; `allowed_origins` is the exact-match Origin allowlist.
const RUM_KEY: &str = "pk_rum_e2e";
const RUM_ORIGIN: &str = "http://localhost";
const RUM_SERVICE: &str = "web";

// Distinctive payload values so the query assertions are unambiguous.
const LCP_VALUE: f64 = 2500.0;
const ERROR_MSG: &str = "boom from rum e2e";

/// A valid W3C example trace id (16 bytes / 32 hex).
const ERROR_TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

const ONE_HOUR_NANOS: i64 = 3_600_000_000_000;

fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64
}

fn rum_app() -> RumApp {
    RumApp {
        name: RUM_SERVICE.into(),
        key: RUM_KEY.into(),
        allowed_origins: vec![RUM_ORIGIN.into()],
        sample_rate: 1.0,
        rate_limit: 5000,
        created_at: 0,
    }
}

/// The frozen beacon shape (Task A3): one `LCP` vital and one JS error. `text/plain` JSON so the
/// browser SDK's POST stays a CORS "simple request".
fn beacon_json() -> String {
    format!(
        r#"{{"app":"web","key":"{RUM_KEY}","session":"s1",
            "view":{{"id":"v1","route":"/checkout","path":"/checkout"}},
            "ctx":{{"ua":"iPhone Safari","conn":"4g"}},
            "trace":"{ERROR_TRACE_ID}",
            "vitals":[{{"n":"LCP","v":{LCP_VALUE}}}],
            "errors":[{{"kind":"exception","type":"TypeError","msg":"{ERROR_MSG}","src":"app.js","line":42}}]}}"#
    )
}

/// The server-side RUM write path, copied verbatim from A5's `RumWalSink` in `photon-server`'s
/// `main.rs` (which a test crate cannot import). Vitals → the metrics WAL, errors → the logs WAL —
/// no new WAL/schema/compactor. Generic over the WAL type so it drives both a plain `DiskWal` (the
/// no-network round trip) and, via `Arc<dyn RumSink>`, the live server.
struct TestRumSink<M: Wal, L: Wal> {
    metrics_wal: Arc<M>,
    logs_wal: Arc<L>,
    metric_schema: MetricSchema,
    log_schema: LogSchema,
}

#[async_trait::async_trait]
impl<M, L> RumSink for TestRumSink<M, L>
where
    M: Wal + Send + Sync + 'static,
    L: Wal + Send + Sync + 'static,
{
    async fn ingest_vitals(&self, points: Vec<MetricPoint>) -> Result<(), PhotonError> {
        let mut b = MetricBatchBuilder::with_capacity(&self.metric_schema, points.len());
        for p in &points {
            b.append(p);
        }
        self.metrics_wal.append(b.finish()?).await
    }

    async fn ingest_errors(&self, records: Vec<LogRecord>) -> Result<(), PhotonError> {
        let mut b = RecordBatchBuilder::with_capacity(&self.log_schema, records.len());
        for r in &records {
            b.append(r);
        }
        self.logs_wal.append(b.finish()?).await
    }
}

// ---------------------------------------------------------------------------
// Live-server harness (copied from e2e.rs) — proves the over-the-wire beacon auth.
// ---------------------------------------------------------------------------

/// Bind to an OS-assigned loopback port, read it back, then drop the listener — a "very likely
/// free" port for a real server to bind next. (Same pattern as `e2e.rs`.)
fn free_addr() -> SocketAddr {
    let l = StdTcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

/// Poll `addr` with a plain TCP connect until it accepts, bounded by an overall timeout —
/// `tokio::spawn`ing a server task doesn't guarantee its listener is bound yet on return.
async fn wait_until_connectable(addr: SocketAddr) {
    tokio::time::timeout(Duration::from_secs(5), async move {
        loop {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("server never became connectable on {addr}"));
}

struct RumServer {
    api_base: String,
    _tmp: tempfile::TempDir,
}

impl RumServer {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }
}

/// A trimmed real `ApiServer` (loopback port) with a single RUM app registered over a `TestRumSink`
/// backed by real `DiskWal`s. No ingest/live-hub/compactors are wired — this harness only exercises
/// the public `POST /api/rum` auth path, so nothing else is needed.
async fn spawn_rum_server() -> RumServer {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();

    let log_schema = LogSchema::new(&["service.name".to_string()]);
    let span_schema = SpanSchema::new(&["service.name".to_string()]);
    // `host.name` is a required promoted metrics column (compactor sort key
    // `(metric_name, service.name, host.name, timestamp)`) — mirrors the default
    // `photon.example.toml` config; this harness never sets a `host.name` attribute so the
    // column is simply all-null for its rows.
    let metric_schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);

    let wal_cfg = WalConfig {
        segment_max_bytes: 64 * 1024 * 1024,
        segment_max_age_secs: 3600,
        group_commit_max_delay_ms: 5,
    };

    let logs_wal = Arc::new(
        DiskWal::open(hot.join("wal"), log_schema.clone(), wal_cfg.clone())
            .await
            .unwrap(),
    );
    let metrics_wal = Arc::new(
        DiskWal::open_arrow(
            hot.join("wal-metrics"),
            metric_schema.arrow.clone(),
            wal_cfg.clone(),
        )
        .await
        .unwrap(),
    );

    // Empty engines: this harness never queries Parquet, but `ApiServer::new` requires all three.
    let query = QueryEngine::new(hot.clone(), log_schema.clone()).unwrap();
    let span_query = SpanQueryEngine::new(hot.clone(), span_schema).unwrap();
    let metrics_query = MetricsQueryEngine::new(hot.clone(), metric_schema.clone()).unwrap();

    // A user store is required by `ApiServer::new`; the RUM beacon route is public (no session) so
    // no user is seeded.
    let user_store = SqliteUserStore::open(hot.join("photon.db").to_str().unwrap()).unwrap();

    let sink: Arc<dyn RumSink> = Arc::new(TestRumSink {
        metrics_wal,
        logs_wal,
        metric_schema,
        log_schema,
    });
    let rum_store = SqliteRumAppStore::open(hot.join("photon.db").to_str().unwrap()).unwrap();
    rum_store.create(&rum_app()).await.unwrap();
    let rum_store: Arc<dyn RumAppStore> = Arc::new(rum_store);
    let rum = RumApi::new(rum_store, sink).await;

    let api = ApiServer::new(
        query,
        span_query,
        metrics_query,
        Arc::new(user_store) as Arc<dyn UserStore>,
        "a-long-random-session-signing-secret-value-for-rum-e2e",
    )
    .with_rum(Some(rum));

    let api_addr = free_addr();
    tokio::spawn(async move {
        api.serve(api_addr).await.unwrap();
    });
    wait_until_connectable(api_addr).await;

    RumServer {
        api_base: format!("http://{api_addr}"),
        _tmp: tmp,
    }
}

#[tokio::test]
async fn rum_beacon_http_auth() {
    let server = spawn_rum_server().await;
    let client = reqwest::Client::new();

    // (a) Good key + allowed Origin + text/plain body -> 204 No Content.
    let ok = client
        .post(server.url("/api/rum"))
        .header("origin", RUM_ORIGIN)
        .header("content-type", "text/plain")
        .body(beacon_json())
        .send()
        .await
        .unwrap();
    assert_eq!(
        ok.status(),
        reqwest::StatusCode::NO_CONTENT,
        "a good beacon (valid key + allowed Origin) should be accepted with 204"
    );

    // (b) Bogus key -> 403 Forbidden (the key selects no registered app).
    let bad_key_body = beacon_json().replace(RUM_KEY, "pk_not_registered");
    let bad_key = client
        .post(server.url("/api/rum"))
        .header("origin", RUM_ORIGIN)
        .header("content-type", "text/plain")
        .body(bad_key_body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        bad_key.status(),
        reqwest::StatusCode::FORBIDDEN,
        "an unregistered app key should be rejected with 403"
    );

    // (c) Disallowed Origin -> 403 Forbidden (right key, Origin not in the allowlist).
    let bad_origin = client
        .post(server.url("/api/rum"))
        .header("origin", "http://evil.example")
        .header("content-type", "text/plain")
        .body(beacon_json())
        .send()
        .await
        .unwrap();
    assert_eq!(
        bad_origin.status(),
        reqwest::StatusCode::FORBIDDEN,
        "a disallowed Origin should be rejected with 403"
    );
}

/// The management endpoints mutate the live registry: an app created over HTTP is immediately
/// beacon-able (CORS predicate + handler read the same cache), and deleting it 403s the beacon —
/// no restart. Proves the whole UI-management spine end to end.
#[tokio::test]
async fn rum_apps_managed_over_http() {
    let server = spawn_rum_server().await;
    let client = reqwest::Client::new();

    // First-run onboarding creates the initial UI user and auto-logins, returning a signed session
    // cookie. The workspace `reqwest` has no `cookies` feature, so we thread the session cookie by
    // hand: the `Set-Cookie` header's first `;`-delimited segment is exactly `name=value`, which is
    // what the `Cookie` request header wants.
    let setup = client
        .post(server.url("/api/setup"))
        .json(&serde_json::json!({ "username": "admin", "password": "hunter2-long-enough" }))
        .send()
        .await
        .unwrap();
    assert!(
        setup.status().is_success(),
        "onboarding setup should succeed, got {}",
        setup.status()
    );
    let session_cookie = setup
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .expect("setup should set a session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Register a brand-new app (distinct from the seed app) over HTTP, carrying the session cookie.
    let created = client
        .post(server.url("/api/rum/apps"))
        .header(reqwest::header::COOKIE, &session_cookie)
        .json(&serde_json::json!({
            "name": "shop",
            "allowed_origins": ["https://shop.example.com"],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let body: serde_json::Value = created.json().await.unwrap();
    let key = body["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("pk_live_"), "server mints a pk_live_ key");

    // A beacon for the just-created app + its origin is accepted (204) — CORS + handler see it live.
    let beacon = serde_json::json!({
        "app": "shop", "key": key, "session": "s1",
        "view": { "id": "v1", "route": "/", "path": "/" },
        "ctx": { "ua": "x", "conn": "4g" },
        "vitals": [{ "n": "LCP", "v": 1000.0 }]
    });
    let ok = client
        .post(server.url("/api/rum"))
        .header("origin", "https://shop.example.com")
        .header("content-type", "text/plain")
        .body(beacon.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(
        ok.status(),
        reqwest::StatusCode::NO_CONTENT,
        "new app's beacon accepted"
    );

    // Delete the app; the same beacon is now forbidden (registry revoked live).
    let del = client
        .delete(server.url("/api/rum/apps/shop"))
        .header(reqwest::header::COOKIE, &session_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), reqwest::StatusCode::NO_CONTENT);

    let gone = client
        .post(server.url("/api/rum"))
        .header("origin", "https://shop.example.com")
        .header("content-type", "text/plain")
        .body(beacon.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(
        gone.status(),
        reqwest::StatusCode::FORBIDDEN,
        "deleted app's beacon rejected"
    );
}

// ---------------------------------------------------------------------------
// No-network round trip — proves the mapped rows become queryable after compaction.
// ---------------------------------------------------------------------------

/// A trailing throwaway metric append that closes the beacon's (size=1) segment so the compactor
/// drains it. Its ts (=1) is far outside the query window and its service is distinct, so it can
/// never match a query.
async fn seal_metrics(wal: &Arc<DiskWal>, schema: &MetricSchema) {
    let mut attrs = BTreeMap::new();
    attrs.insert("service.name".to_string(), "__seal__".to_string());
    let seal = MetricPoint {
        metric_name: "__seal__".to_string(),
        metric_type: metric_type::GAUGE,
        timestamp_nanos: 1,
        value: Some(0.0),
        attributes: attrs,
        ..MetricPoint::default()
    };
    let mut b = MetricBatchBuilder::new(schema);
    b.append(&seal);
    wal.append(b.finish().unwrap()).await.unwrap();
}

/// Log counterpart of [`seal_metrics`]: closes the logs WAL's segment with a non-matching row.
async fn seal_logs(wal: &Arc<DiskWal>, schema: &LogSchema) {
    let mut attrs = BTreeMap::new();
    attrs.insert("service.name".to_string(), "__seal__".to_string());
    let seal = LogRecord {
        timestamp_nanos: 1,
        severity_number: Some(9), // INFO
        severity_text: Some("INFO".to_string()),
        body: Some("__seal__".to_string()),
        attributes: attrs,
        ..LogRecord::default()
    };
    let mut b = RecordBatchBuilder::new(schema);
    b.append(&seal);
    wal.append(b.finish().unwrap()).await.unwrap();
}

#[tokio::test]
async fn rum_beacon_maps_to_queryable_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();

    let log_schema = LogSchema::new(&["service.name".to_string()]);
    // See the matching comment in `spawn_rum_server` above: `host.name` is a required promoted
    // metrics column post the Phase-1 sort-key change; this fixture never sets it, so the column
    // is all-null.
    let metric_schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);

    // Tiny segments so each append rotates; a trailing seal append then closes the beacon's
    // segment for the compactor to drain (mirrors photon-query/tests/metric_query.rs).
    let wal_cfg = WalConfig {
        segment_max_bytes: 1,
        segment_max_age_secs: 0,
        group_commit_max_delay_ms: 0,
    };

    let metrics_wal = Arc::new(
        DiskWal::open_arrow(
            hot.join("wal-metrics"),
            metric_schema.arrow.clone(),
            wal_cfg.clone(),
        )
        .await
        .unwrap(),
    );
    let logs_wal = Arc::new(
        DiskWal::open(hot.join("wal"), log_schema.clone(), wal_cfg.clone())
            .await
            .unwrap(),
    );

    let sink = TestRumSink {
        metrics_wal: metrics_wal.clone(),
        logs_wal: logs_wal.clone(),
        metric_schema: metric_schema.clone(),
        log_schema: log_schema.clone(),
    };

    // Map the frozen beacon exactly as the handler does, then write it through the same sink.
    let now = now_nanos();
    let beacon: Beacon = serde_json::from_str(&beacon_json()).unwrap();
    let points = rum::beacon_to_metric_points(&beacon, RUM_SERVICE, now);
    let records = rum::beacon_to_log_records(&beacon, RUM_SERVICE, now);
    assert_eq!(points.len(), 1, "the LCP vital maps to one metric point");
    assert_eq!(records.len(), 1, "the JS error maps to one log record");
    assert_eq!(points[0].metric_name, "web_vitals.lcp");

    sink.ingest_vitals(points).await.unwrap();
    sink.ingest_errors(records).await.unwrap();

    // Close each beacon segment, then compact both WALs to hot Parquet. Running the compactors
    // synchronously (no background tasks) keeps the round trip fully deterministic.
    seal_metrics(&metrics_wal, &metric_schema).await;
    seal_logs(&logs_wal, &log_schema).await;

    let storage = Storage::from_config(&StorageConfig {
        hot_dir: hot.clone(),
        db_path: String::new(),
        durable: None,
        zstd_level: 1,
    })
    .unwrap();
    let replicator = Arc::new(Replicator::new(storage.clone()));

    let metrics_compactor = MetricsCompactor::new(
        metrics_wal.clone(),
        storage.clone(),
        replicator.clone(),
        metric_schema.clone(),
    );
    while metrics_compactor.run_once().await.unwrap().is_some() {}

    let logs_compactor = Compactor::new(
        logs_wal.clone(),
        storage.clone(),
        replicator.clone(),
        log_schema.clone(),
    );
    while logs_compactor.run_once().await.unwrap().is_some() {}

    // (1) The LCP vital is queryable as a `web_vitals.lcp` gauge point carrying the posted value,
    // attributed to `service.name = web`.
    let metrics_engine = MetricsQueryEngine::new(hot.clone(), metric_schema).unwrap();
    let batches = metrics_engine
        .sql(
            "SELECT value FROM metrics \
             WHERE metric_name = 'web_vitals.lcp' AND \"service.name\" = 'web'",
        )
        .await
        .unwrap();
    let values: Vec<f64> = batches
        .iter()
        .flat_map(|b| {
            let col = b
                .column_by_name("value")
                .unwrap()
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("value column should be Float64");
            (0..b.num_rows())
                .filter(|&i| !col.is_null(i))
                .map(|i| col.value(i))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        values,
        vec![LCP_VALUE],
        "expected exactly one web_vitals.lcp point for service=web carrying the posted value"
    );

    // (2) The JS error is queryable as an ERROR log row for `service.name = web`.
    let logs_engine = QueryEngine::new(hot.clone(), log_schema).unwrap();
    let results = logs_engine
        .search(QueryRequest {
            start_ts_nanos: now - ONE_HOUR_NANOS,
            end_ts_nanos: now + ONE_HOUR_NANOS,
            services: vec![RUM_SERVICE.to_string()],
            severities: vec![(rum::ERROR_SEVERITY_NUMBER, rum::ERROR_SEVERITY_NUMBER)],
            text: Some(ERROR_MSG.to_string()),
            query: None,
            limit: 100,
        })
        .await
        .unwrap();
    assert!(!results.is_empty(), "expected non-empty log search results");

    let found = results.iter().any(|b| {
        let body = b
            .column_by_name(schema::BODY)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("body column should be a Utf8 StringArray");
        (0..b.num_rows()).any(|i| !body.is_null(i) && body.value(i).contains(ERROR_MSG))
    });
    assert!(
        found,
        "expected the ERROR log row for service=web carrying the posted error message"
    );

    // (3) The RUM error row carries the pageview trace id on the native trace_id column.
    let trace_found = results.iter().any(|b| {
        let tid = b
            .column_by_name(schema::TRACE_ID)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("trace_id column should be a Utf8 StringArray");
        (0..b.num_rows()).any(|i| !tid.is_null(i) && tid.value(i) == ERROR_TRACE_ID)
    });
    assert!(
        trace_found,
        "expected the ERROR row to carry the beacon's trace_id"
    );
}

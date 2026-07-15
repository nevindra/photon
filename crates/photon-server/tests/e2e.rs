//! End-to-end test: OTLP request -> WAL -> compactor -> query engine, wired from the real
//! components (no fakes) against a tempdir hot store.
//!
//! Proves the full ingest path a running `photon-server` exercises: an
//! `ExportLogsServiceRequest` is mapped to `LogRecord`s, built into a `RecordBatch`, appended
//! to a `DiskWal` (sized so the append closes a segment), drained by the `Compactor` into a
//! sorted Parquet file + skip index + manifest, and finally found by a free-text
//! `QueryEngine::search`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow::array::{Array, StringArray};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::resource::v1::Resource;

use photon_compact::{Compactor, MetricsCompactor};
use photon_core::config::{StorageConfig, WalConfig};
use photon_core::record::RecordBatchBuilder;
use photon_core::schema::{self, LogSchema};
use photon_ingest::otlp_logs_to_records;
use photon_query::{QueryEngine, QueryRequest};
use photon_storage::{Replicator, Storage};
use photon_wal::DiskWal;

// BE-6 live-tail e2e additions: a real over-the-wire server (ingest HTTP + API) plus argon2,
// prost, and the streaming pieces the component-level test above doesn't need.
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};
use photon_api::users::{SqliteUserStore, UserStore};
use photon_api::{ApiServer, LiveHub};
use photon_core::config::LiveConfig;
use photon_core::metric_schema::MetricSchema;
use photon_core::span_schema::SpanSchema;
use photon_ingest::IngestServer;
use photon_ingest::{
    Label as PromLabel, Sample as PromSample, TimeSeries as PromTimeSeries,
    WriteRequest as PromWriteRequest,
};
use photon_query::{MetricsQueryEngine, SpanQueryEngine};
use photon_wal::BroadcastingWal;
use prost::Message as _;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::time::Duration;

const TARGET_LINE: &str = "error when indexing document 7";
const ONE_HOUR_NANOS: i64 = 3_600_000_000_000;

fn any_str(s: &str) -> AnyValue {
    AnyValue {
        value: Some(Value::StringValue(s.to_string())),
    }
}

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(any_str(value)),
    }
}

fn log_record(ts_nanos: i64, body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: ts_nanos as u64,
        observed_time_unix_nano: 0,
        severity_number: SeverityNumber::Error as i32,
        severity_text: "ERROR".to_string(),
        body: Some(any_str(body)),
        attributes: vec![],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: vec![],
        span_id: vec![],
    }
}

/// A request with a `service.name` resource attribute and three log records, one of which is
/// the `TARGET_LINE`.
fn export_request(now_nanos: i64) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "api"), kv("host.name", "host-1")],
                dropped_attributes_count: 0,
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![
                    log_record(now_nanos, TARGET_LINE),
                    log_record(now_nanos + 1, "request completed in 12ms"),
                    log_record(now_nanos + 2, "cache miss for key user:42"),
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

#[tokio::test]
async fn ingest_wal_compact_query_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();

    let schema = LogSchema::new(&["service.name".to_string(), "host.name".to_string()]);

    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;

    // `segment_max_bytes` is tiny so any non-empty append pushes the active segment past the
    // limit and `Writer::maybe_rotate` seals it (see photon-wal/src/disk.rs). Rotation runs
    // inside the writer task after the append's ack, so we append twice sequentially: by the
    // time the second append resolves, the first segment is guaranteed sealed.
    let wal_cfg = WalConfig {
        segment_max_bytes: 64,
        segment_max_age_secs: 3600,
        group_commit_max_delay_ms: 5,
    };
    let wal = Arc::new(
        DiskWal::open(hot.join("wal"), schema.clone(), wal_cfg)
            .await
            .unwrap(),
    );

    // OTLP -> LogRecords -> RecordBatch.
    let req = export_request(now_nanos);
    let records = otlp_logs_to_records(req);
    assert_eq!(records.len(), 3, "expected one record per log line");

    let mut builder = RecordBatchBuilder::new(&schema);
    for r in &records {
        builder.append(r);
    }
    let batch = builder.finish().unwrap();

    // Append twice to force at least one CLOSED segment (see the wal_cfg comment).
    wal.append(batch.clone()).await.unwrap();
    wal.append(batch.clone()).await.unwrap();

    let closed = wal.list_closed_segments().unwrap();
    assert!(
        !closed.is_empty(),
        "expected at least one closed WAL segment, got {closed:?}"
    );

    // Compact every closed segment into the hot store.
    let storage = Storage::from_config(&StorageConfig {
        hot_dir: hot.clone(),
        db_path: String::new(),
        durable: None,
        zstd_level: 1,
    })
    .unwrap();
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = Compactor::new(wal.clone(), storage.clone(), replicator, schema.clone());

    let mut compacted = 0;
    while let Some(_seg) = compactor.run_once().await.unwrap() {
        compacted += 1;
    }
    assert!(
        compacted >= 1,
        "expected the compactor to process a segment"
    );

    // Free-text query for "indexing" over the (now Parquet-backed) hot store.
    let engine = QueryEngine::new(hot.clone(), schema.clone()).unwrap();
    let results = engine
        .search(QueryRequest {
            start_ts_nanos: now_nanos - ONE_HOUR_NANOS,
            end_ts_nanos: now_nanos + ONE_HOUR_NANOS,
            services: vec![],
            severities: vec![],
            text: Some("indexing".to_string()),
            query: None,
            limit: 100,
        })
        .await
        .unwrap();

    assert!(!results.is_empty(), "expected non-empty query results");

    let found = results.iter().any(|b| {
        let body = b
            .column_by_name(schema::BODY)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("body column should be a Utf8 StringArray");
        (0..b.num_rows()).any(|i| !body.is_null(i) && body.value(i).contains(TARGET_LINE))
    });
    assert!(found, "expected the target log line in the results");
}

// ---------------------------------------------------------------------------
// BE-6: end-to-end SSE live-tail test.
//
// Unlike the component-level test above (which drives the WAL/compactor/query-engine
// directly), this spins up the *real* over-the-wire stack — `IngestServer` (OTLP/HTTP) and
// `ApiServer` (REST + SSE), each bound to a loopback port via `axum::serve`/tonic — wired
// exactly like `photon-server`'s `main` (WALs wrapped in `BroadcastingWal`, a `LiveHub`
// attached via `ApiServer::with_live_hub`), so it proves the full wire path: OTLP POST -> WAL
// append -> broadcast -> `/api/stream/logs` SSE, and the session-cookie auth gate in front of
// it. No compactor is spawned — the stream reads off the broadcast tap, never Parquet, so
// compaction is irrelevant here.
// ---------------------------------------------------------------------------

const LIVE_INGEST_TOKEN: &str = "e2e-stream-test-token";

/// Bind to an OS-assigned loopback port, read it back, then drop the listener — a "very
/// likely free" port for a real server to bind next. Same pattern `photon-server`'s own
/// `healthcheck` tests use (see `crates/photon-server/src/main.rs`).
fn free_addr() -> SocketAddr {
    let l = StdTcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

/// Poll `addr` with a plain TCP connect until it accepts, bounded by an overall timeout —
/// `tokio::spawn`ing a server task doesn't guarantee its listener is bound yet by the time the
/// spawn call returns.
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

/// Argon2 PHC hash with an OS-random salt — mirrors `photon-server`'s own `hash-password`
/// subcommand. `photon_api::users::SqliteUserStore`'s test-only seeding helpers are
/// `pub(crate)` (invisible outside the `photon-api` crate), so this harness seeds a user via
/// the public `UserStore::create` API instead, hashing the password itself.
fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

/// A fully wired, real over-the-wire Photon stack for one test: an ingest HTTP/gRPC front end
/// and the API/UI server (with the live-tail hub attached), both on loopback ports, backed by a
/// tempdir hot store that is dropped (and cleaned up) when this struct is.
struct LiveServer {
    api_base: String,
    ingest_http_base: String,
    /// The tempdir hot-store root shared by every signal's WAL + `data-*`/manifest dirs — kept
    /// so a test can build its own `Storage`/compactor over the same root the live query engines
    /// read from (see `compact_metrics`).
    hot_dir: PathBuf,
    /// A second handle to the metrics WAL — the same `Arc` `IngestServer` appends to — kept so a
    /// test can drive `photon_compact::MetricsCompactor` directly after posting through
    /// `/api/v1/write`. `spawn_live_server` runs no background compactor loop (`/api/metrics/*`
    /// reads Parquet, not the WAL), so this is required before any post-ingest metrics query.
    metrics_wal: Arc<BroadcastingWal<DiskWal>>,
    metric_schema: MetricSchema,
    _tmp: tempfile::TempDir,
}

impl LiveServer {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }
}

async fn spawn_live_server() -> LiveServer {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();

    let schema = LogSchema::new(&["service.name".to_string()]);
    let span_schema = SpanSchema::new(&["service.name".to_string()]);
    // `host.name` is a required promoted metrics column (the compactor sort key is
    // `(metric_name, service.name, host.name, timestamp)` — see `metrics_compactor::sort_metrics`)
    // — mirrors the default `photon.example.toml` config even though nothing in this file's
    // fixtures sets a `host.name` attribute (the column is simply all-null for those rows).
    let metric_schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);

    let wal_cfg = WalConfig {
        segment_max_bytes: 64 * 1024 * 1024,
        segment_max_age_secs: 3600,
        group_commit_max_delay_ms: 5,
    };

    let logs_wal_inner = DiskWal::open(hot.join("wal"), schema.clone(), wal_cfg.clone())
        .await
        .unwrap();
    let logs_wal = Arc::new(BroadcastingWal::new(logs_wal_inner, 1024));
    let logs_tx = logs_wal.sender();

    let spans_wal_inner = DiskWal::open_arrow(
        hot.join("wal-traces"),
        span_schema.arrow.clone(),
        wal_cfg.clone(),
    )
    .await
    .unwrap();
    let spans_wal = Arc::new(BroadcastingWal::new(spans_wal_inner, 1024));
    let spans_tx = spans_wal.sender();

    // A dedicated, tiny-`segment_max_bytes` config (mirrors `ingest_wal_compact_query_end_to_end`'s
    // `wal_cfg` above) so any real remote-write batch immediately exceeds the size threshold and
    // closes its segment on the very round that writes it — logs/spans keep the large shared
    // `wal_cfg` since nothing in this harness compacts them.
    let metrics_wal_cfg = WalConfig {
        segment_max_bytes: 64,
        segment_max_age_secs: 3600,
        group_commit_max_delay_ms: 5,
    };
    let metrics_wal_inner = DiskWal::open_arrow(
        hot.join("wal-metrics"),
        metric_schema.arrow.clone(),
        metrics_wal_cfg,
    )
    .await
    .unwrap();
    let metrics_wal = Arc::new(BroadcastingWal::new(metrics_wal_inner, 1024));
    // Kept for `LiveServer` (a test-only compaction seam) before the original is moved into
    // `IngestServer::new` below.
    let metrics_wal_for_compaction = metrics_wal.clone();

    // Empty query engines: this test never queries Parquet (no compactor runs), only the live
    // SSE tap — but `ApiServer::new` still requires all three engines up front.
    let query = QueryEngine::new(hot.clone(), schema.clone()).unwrap();
    let span_query = SpanQueryEngine::new(hot.clone(), span_schema.clone()).unwrap();
    let metrics_query = MetricsQueryEngine::new(hot.clone(), metric_schema.clone()).unwrap();
    // Kept for `LiveServer` before the original `metric_schema` is moved into `IngestServer::new` below.
    let metric_schema_for_compaction = metric_schema.clone();

    // Seed a single admin user via the public `UserStore` API.
    let db_path = hot.join("photon.db");
    let user_store = SqliteUserStore::open(db_path.to_str().unwrap()).unwrap();
    user_store
        .create("admin", &hash_password("admin"))
        .await
        .unwrap();

    let live_hub = LiveHub::new(
        logs_tx,
        spans_tx,
        LiveConfig {
            broadcast_capacity: 1024,
            // Fast coalescing flush so the test doesn't wait the production default (250ms)
            // repeatedly across retries.
            flush_interval_ms: 20,
            max_rows_per_flush: 200,
            max_connections: 32,
        },
    );

    let api = ApiServer::new(
        query,
        span_query,
        metrics_query,
        Arc::new(user_store) as Arc<dyn UserStore>,
        "a-long-random-session-signing-secret-value-for-e2e",
    )
    .with_live_hub(live_hub);

    let api_addr = free_addr();
    tokio::spawn(async move {
        api.serve(api_addr).await.unwrap();
    });

    let ingest = IngestServer::new(
        logs_wal,
        spans_wal,
        metrics_wal,
        LIVE_INGEST_TOKEN.to_string(),
        schema,
        span_schema,
        metric_schema,
        256,
        16 * 1024 * 1024,
        Arc::new(photon_core::ingest_counters::IngestCounters::new()),
    );
    let grpc_addr = free_addr();
    let http_addr = free_addr();
    tokio::spawn(async move {
        ingest.serve(grpc_addr, http_addr).await.unwrap();
    });

    wait_until_connectable(api_addr).await;
    wait_until_connectable(http_addr).await;

    LiveServer {
        api_base: format!("http://{api_addr}"),
        ingest_http_base: format!("http://{http_addr}"),
        hot_dir: hot,
        metrics_wal: metrics_wal_for_compaction,
        metric_schema: metric_schema_for_compaction,
        _tmp: tmp,
    }
}

/// Log in as the seeded admin, returning the `photon_session=...` cookie pair.
async fn login(server: &LiveServer, username: &str, password: &str) -> String {
    let resp = reqwest::Client::new()
        .post(server.url("/api/login"))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "login failed: {}",
        resp.status()
    );
    let raw = resp
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .expect("login should set a session cookie")
        .to_str()
        .unwrap();
    raw.split(';').next().unwrap().to_string()
}

/// Best-effort OTLP log POST to the running ingest HTTP receiver — `service.name` = `service`,
/// one log record per `bodies` entry. Returns whether the POST got a 2xx, never panics (so it
/// is safe to call from a background retry loop).
async fn try_post_otlp_logs(ingest_http_base: &str, service: &str, bodies: &[&str]) -> bool {
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let log_records = bodies
        .iter()
        .enumerate()
        .map(|(i, body)| log_record(now_nanos + i as i64, body))
        .collect();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("service.name", service)],
                dropped_attributes_count: 0,
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let body = req.encode_to_vec();
    match reqwest::Client::new()
        .post(format!("{ingest_http_base}/v1/logs"))
        .header("content-type", "application/x-protobuf")
        .header("authorization", format!("Bearer {LIVE_INGEST_TOKEN}"))
        .body(body)
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Asserting wrapper around [`try_post_otlp_logs`] for the one call whose success actually
/// matters (proves the ingest pipeline itself is wired correctly, independent of the SSE race
/// this test also guards against).
async fn post_otlp_logs(ingest_http_base: &str, service: &str, bodies: &[&str]) {
    assert!(
        try_post_otlp_logs(ingest_http_base, service, bodies).await,
        "otlp log post to {ingest_http_base} failed"
    );
}

/// Read SSE chunks off `resp` until the accumulated body contains `needle`, bounded by an
/// overall `timeout`. Panics with the accumulated body (for debugging) on timeout or if the
/// stream ends first — never blocks forever.
async fn read_sse_until(resp: &mut reqwest::Response, needle: &str, timeout: Duration) -> String {
    let mut acc = String::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "timed out after {timeout:?} waiting for an SSE frame containing {needle:?}; \
                 accumulated body so far:\n{acc}"
            );
        }
        match tokio::time::timeout(remaining, resp.chunk()).await {
            Ok(Ok(Some(bytes))) => {
                acc.push_str(&String::from_utf8_lossy(&bytes));
                if acc.contains(needle) {
                    return acc;
                }
            }
            Ok(Ok(None)) => panic!(
                "SSE stream ended before an frame containing {needle:?} arrived; \
                 accumulated body:\n{acc}"
            ),
            Ok(Err(e)) => panic!("error reading an SSE chunk: {e}"),
            Err(_) => panic!(
                "timed out after {timeout:?} waiting for an SSE frame containing {needle:?}; \
                 accumulated body so far:\n{acc}"
            ),
        }
    }
}

#[tokio::test]
async fn stream_logs_pushes_matching_rows() {
    let server = spawn_live_server().await;

    // (a) Unauthenticated -> 401, before a session even exists.
    let unauth = reqwest::Client::new()
        .get(server.url("/api/stream/logs?q=service:e2e"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);

    // (b) Authenticated: open the stream *first* and confirm it's live (the `stream_logs`
    // handler resolves `hub.logs.subscribe()` synchronously before the response is returned,
    // so a 200 here already implies the subscribe has happened) before ingesting anything.
    let cookie = login(&server, "admin", "admin").await;
    let mut resp = reqwest::Client::new()
        .get(server.url("/api/stream/logs?q=service:e2e"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "stream open failed: {}",
        resp.status()
    );

    // Ingest the matching log — asserted once (proves the ingest pipeline itself works).
    post_otlp_logs(&server.ingest_http_base, "e2e", &["hello from e2e"]).await;

    // Defensive re-posting against subscribe-timing races: keep nudging the same row in every
    // ~200ms for a few seconds, concurrently with the read below, in case the first append's
    // broadcast still somehow preceded the subscribe.
    let ingest_base = server.ingest_http_base.clone();
    tokio::spawn(async move {
        for _ in 0..25 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            try_post_otlp_logs(&ingest_base, "e2e", &["hello from e2e"]).await;
        }
    });

    let body = read_sse_until(&mut resp, "\"service\":\"e2e\"", Duration::from_secs(10)).await;
    assert!(body.contains("event: rows"), "missing rows event:\n{body}");
    assert!(
        body.contains("hello from e2e"),
        "missing expected body text:\n{body}"
    );
}

// ---------------------------------------------------------------------------
// Prometheus remote-write (Plan 1): the /api/v1/write receiver over the real wire.
// Proves the route is mounted on the ingest HTTP server, is bearer-gated, and accepts a
// snappy+protobuf RW 1.0 body. Data-correctness of the mapping is covered by the
// photon-ingest handler test (valid_request_maps_and_appends); this guards the wire contract.
// ---------------------------------------------------------------------------

fn snappy_write_request() -> Vec<u8> {
    let req = PromWriteRequest {
        timeseries: vec![PromTimeSeries {
            labels: vec![
                PromLabel {
                    name: "__name__".into(),
                    value: "http_requests_total".into(),
                },
                PromLabel {
                    name: "job".into(),
                    value: "api".into(),
                },
            ],
            samples: vec![PromSample {
                value: 7.0,
                timestamp: 1_700_000_000_000,
            }],
        }],
    };
    let proto = req.encode_to_vec();
    let mut encoder = snap::raw::Encoder::new();
    encoder.compress_vec(&proto).unwrap()
}

#[tokio::test]
async fn promrw_remote_write_end_to_end() {
    let server = spawn_live_server().await;
    let write_url = format!("{}/api/v1/write", server.ingest_http_base);
    let client = reqwest::Client::new();
    let body = snappy_write_request();

    // Valid token + valid body → 200.
    let ok = client
        .post(&write_url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {LIVE_INGEST_TOKEN}"),
        )
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), reqwest::StatusCode::OK);

    // No token → 401.
    let unauth = client
        .post(&write_url)
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Valid token + garbage body → 400 (route reached, decode rejected).
    let bad = client
        .post(&write_url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {LIVE_INGEST_TOKEN}"),
        )
        .body(vec![0xFFu8; 8])
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Prometheus remote-write (Plan 2): classic-histogram percentile query end-to-end. Proves the
// whole feature over the real stack — remote-write a classic histogram (flat
// `<base>_bucket{le=..}`/`_sum`/`_count` cumulative SUM series), compact it into the hot store,
// then confirm `/api/metrics/query` reassembles p90/count/avg for `<base>` and
// `/api/metrics/catalog` folds the family into one HISTOGRAM-typed `h` entry.
// ---------------------------------------------------------------------------

/// Drain every currently-closed metrics WAL segment into `server`'s hot store via a fresh
/// `MetricsCompactor` — the metrics analogue of the top-of-file test's `Compactor::run_once`
/// loop. `spawn_live_server` runs no background compactor (only SSE live-tail is served, and
/// `/api/metrics/*` reads Parquet, not the WAL), so a test that posts through `/api/v1/write`
/// must call this before querying.
async fn compact_metrics(server: &LiveServer) {
    let storage = Storage::from_config(&StorageConfig {
        hot_dir: server.hot_dir.clone(),
        db_path: String::new(),
        durable: None,
        zstd_level: 1,
    })
    .unwrap();
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = MetricsCompactor::new(
        server.metrics_wal.clone(),
        storage,
        replicator,
        server.metric_schema.clone(),
    );

    let mut compacted = 0;
    while compactor.run_once().await.unwrap().is_some() {
        compacted += 1;
    }
    assert!(
        compacted >= 1,
        "expected the metrics compactor to process at least one closed WAL segment"
    );
}

fn promrw_label(name: &str, value: &str) -> PromLabel {
    PromLabel {
        name: name.to_string(),
        value: value.to_string(),
    }
}

fn promrw_sample(value: f64, timestamp_ms: i64) -> PromSample {
    PromSample {
        value,
        timestamp: timestamp_ms,
    }
}

fn snappy_encode_write_request(req: &PromWriteRequest) -> Vec<u8> {
    let proto = req.encode_to_vec();
    let mut encoder = snap::raw::Encoder::new();
    encoder.compress_vec(&proto).unwrap()
}

/// A classic-histogram `h` remote-write fixture (`job="api"`), posted at two timestamps: `t0_ms`
/// (baseline — every series 0) and `t1_ms` (the real observation). `reset_aware_series`
/// (`metric_query.rs`) contributes 0 for a series' first sample — without the `t0` baseline, the
/// `t1` reading would itself look like a first sample and every percentile would be `None`.
/// Numbers mirror `metric_classic_hist.rs`'s `engine_tests::classic_engine` fixture: cumulative
/// le=1→10, le=2→30, le=+Inf→30, sum→45, count→30 — p90 rank 27 lands in (1,2] at
/// `1 + (27-10)/20 = 1.85`; avg = 45/30 = 1.5.
fn classic_histogram_write_request(t0_ms: i64, t1_ms: i64) -> PromWriteRequest {
    fn bucket_series(le: &str, t0_v: f64, t1_v: f64, t0_ms: i64, t1_ms: i64) -> PromTimeSeries {
        PromTimeSeries {
            labels: vec![
                promrw_label("__name__", "h_bucket"),
                promrw_label("job", "api"),
                promrw_label("le", le),
            ],
            samples: vec![promrw_sample(t0_v, t0_ms), promrw_sample(t1_v, t1_ms)],
        }
    }
    fn counter_series(name: &str, t0_v: f64, t1_v: f64, t0_ms: i64, t1_ms: i64) -> PromTimeSeries {
        PromTimeSeries {
            labels: vec![promrw_label("__name__", name), promrw_label("job", "api")],
            samples: vec![promrw_sample(t0_v, t0_ms), promrw_sample(t1_v, t1_ms)],
        }
    }
    PromWriteRequest {
        timeseries: vec![
            bucket_series("1", 0.0, 10.0, t0_ms, t1_ms),
            bucket_series("2", 0.0, 30.0, t0_ms, t1_ms),
            bucket_series("+Inf", 0.0, 30.0, t0_ms, t1_ms),
            counter_series("h_sum", 0.0, 45.0, t0_ms, t1_ms),
            counter_series("h_count", 0.0, 30.0, t0_ms, t1_ms),
        ],
    }
}

/// An unrelated, minimal remote-write body whose only purpose is to force a second WAL round.
/// The WAL's writer task processes rounds strictly sequentially, so once this POST's 200
/// response is observed, the previous round's `maybe_rotate` (segment-close check) is guaranteed
/// to have already completed — closing the segment holding the histogram fixture (the metrics
/// WAL's `segment_max_bytes` is tiny — see `spawn_live_server`) before `compact_metrics` looks
/// for it. Mirrors the "append twice" trick `ingest_wal_compact_query_end_to_end` uses directly
/// against the WAL, translated to two sequential HTTP round-trips.
fn seal_write_request(ts_ms: i64) -> PromWriteRequest {
    PromWriteRequest {
        timeseries: vec![PromTimeSeries {
            labels: vec![
                promrw_label("__name__", "seal_signal"),
                promrw_label("job", "api"),
            ],
            samples: vec![promrw_sample(1.0, ts_ms)],
        }],
    }
}

async fn post_promrw(server: &LiveServer, client: &reqwest::Client, req: &PromWriteRequest) {
    let resp = client
        .post(format!("{}/api/v1/write", server.ingest_http_base))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {LIVE_INGEST_TOKEN}"),
        )
        .body(snappy_encode_write_request(req))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "remote-write POST failed"
    );
}

/// POST `/api/metrics/query` for one `agg` over metric `h`, returning every point's `v` from
/// series 0 across the window (`response shape: { results:[{ id, series:[{points:[{t,v}]}] }] }`).
async fn query_h_agg(
    server: &LiveServer,
    client: &reqwest::Client,
    cookie: &str,
    agg: &str,
    start_ns: &str,
    end_ns: &str,
) -> Vec<Option<f64>> {
    let resp = client
        .post(server.url("/api/metrics/query"))
        .header("cookie", cookie)
        .json(&serde_json::json!({
            "queries": [{ "id": "a", "metric": "h", "agg": agg, "group_by": [], "filter": "" }],
            "start": start_ns,
            "end": end_ns,
        }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "metrics query (agg={agg}) failed: {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let series = body["results"][0]["series"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a series array for agg={agg}: {body}"));
    assert!(
        !series.is_empty(),
        "expected at least one series for agg={agg}: {body}"
    );
    series[0]["points"]
        .as_array()
        .expect("points should be an array")
        .iter()
        .map(|p| p["v"].as_f64())
        .collect()
}

#[tokio::test]
async fn promrw_histogram_percentile_query_end_to_end() {
    let server = spawn_live_server().await;
    let client = reqwest::Client::new();

    let t0_ms: i64 = 1_700_000_000_000;
    let t1_ms: i64 = t0_ms + 5_000; // 5s later — well within the query window below.

    // 1) Remote-write the classic-histogram fixture over the real wire.
    post_promrw(
        &server,
        &client,
        &classic_histogram_write_request(t0_ms, t1_ms),
    )
    .await;

    // 2) A second, unrelated write forces a fresh WAL round, guaranteeing the segment above is
    // sealed by the time this POST's response arrives (see `seal_write_request`'s doc comment).
    post_promrw(&server, &client, &seal_write_request(t1_ms + 1)).await;

    // 3) The live harness serves /api/metrics/query from Parquet, not the WAL.
    compact_metrics(&server).await;

    // 4) p90/count/avg over a window spanning both timestamps.
    let cookie = login(&server, "admin", "admin").await;
    let start_ns = ((t0_ms - 60_000) * 1_000_000).to_string();
    let end_ns = ((t1_ms + 60_000) * 1_000_000).to_string();

    let p90 = query_h_agg(&server, &client, &cookie, "p90", &start_ns, &end_ns).await;
    assert!(
        p90.iter()
            .any(|v| v.is_some_and(|v| (v - 1.85).abs() < 1e-6)),
        "expected a p90 point ≈ 1.85, got {p90:?}"
    );

    let count = query_h_agg(&server, &client, &cookie, "count", &start_ns, &end_ns).await;
    assert!(
        count
            .iter()
            .any(|v| v.is_some_and(|v| (v - 30.0).abs() < 1e-9)),
        "expected a count point == 30, got {count:?}"
    );

    let avg = query_h_agg(&server, &client, &cookie, "avg", &start_ns, &end_ns).await;
    assert!(
        avg.iter()
            .any(|v| v.is_some_and(|v| (v - 1.5).abs() < 1e-9)),
        "expected an avg point ≈ 1.5, got {avg:?}"
    );

    // 5) Catalog fold: "h" appears as a histogram; "h_bucket" is folded away.
    let catalog_resp = client
        .get(server.url(&format!(
            "/api/metrics/catalog?start={start_ns}&end={end_ns}"
        )))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(
        catalog_resp.status().is_success(),
        "catalog query failed: {}",
        catalog_resp.status()
    );
    let catalog: serde_json::Value = catalog_resp.json().await.unwrap();
    let entries = catalog
        .as_array()
        .expect("catalog response should be a JSON array");
    assert!(
        entries
            .iter()
            .any(|e| e["name"] == "h" && e["type"] == "histogram"),
        "expected a histogram entry named `h` in the catalog: {catalog}"
    );
    assert!(
        !entries.iter().any(|e| e["name"] == "h_bucket"),
        "expected `h_bucket` to be folded away from the catalog: {catalog}"
    );
}

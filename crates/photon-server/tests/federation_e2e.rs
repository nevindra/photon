//! End-to-end federation test: a real over-the-wire "central" `photon-server` stack — tenant
//! registration, per-tenant token resolution + server-side `tenant` stamping at ingest, the
//! curated `/api/tenants/summary` board endpoint, and token rotation. Boot scaffolding is a
//! federation-flavored copy of `crates/photon-server/tests/e2e.rs`'s `spawn_live_server`
//! (`LiveServer`) — this file drives everything already built by earlier tasks, so it goes
//! straight to green: it is the verification layer, not TDD of new code.

use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point::Value as NumVal, AggregationTemporality, Gauge, Metric,
    NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message as _;

use photon_api::tenants::{SqliteTenantStore, TenantApi, TenantStore};
use photon_api::users::{SqliteUserStore, UserStore};
use photon_api::{ApiServer, LiveHub};
use photon_compact::Compactor;
use photon_core::config::{LiveConfig, WalConfig};
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_schema::MetricSchema;
use photon_core::schema::LogSchema;
use photon_core::span_schema::SpanSchema;
use photon_core::TenantTokenMap;
use photon_ingest::IngestServer;
use photon_query::{MetricsQueryEngine, QueryEngine, SpanQueryEngine};
use photon_storage::{Replicator, Storage};
use photon_wal::{BroadcastingWal, DiskWal};

const LOCAL_TOKEN: &str = "federation-e2e-local-token";

fn free_addr() -> SocketAddr {
    let l = StdTcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

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

fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

/// A real over-the-wire "central" Photon stack: `IngestServer` + `ApiServer`, both on loopback
/// ports, with `promoted_attributes = ["service.name", "host.name", "tenant"]` so `tenant` rides
/// the normal promoted-column path in every engine — plus the tenant registry wired into both
/// (mirrors `photon-server::main`'s `tenant_tokens` sharing). WAL segments are sized tiny so a
/// single POST closes them, letting the test compact + query without a background loop.
struct CentralServer {
    api_base: String,
    ingest_http_base: String,
    hot_dir: PathBuf,
    logs_wal: Arc<BroadcastingWal<DiskWal>>,
    log_schema: LogSchema,
    metrics_wal: Arc<BroadcastingWal<DiskWal>>,
    metric_schema: MetricSchema,
    _tmp: tempfile::TempDir,
}

impl CentralServer {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }
}

async fn spawn_central() -> CentralServer {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();

    let promoted = vec![
        "service.name".to_string(),
        "host.name".to_string(),
        "tenant".to_string(),
    ];
    let log_schema = LogSchema::new(&promoted);
    let span_schema = SpanSchema::new(&promoted);
    let metric_schema = MetricSchema::new(&promoted);

    // Tiny segment size so a single ingest POST closes its WAL segment, ready for `Compactor`.
    let wal_cfg = WalConfig {
        segment_max_bytes: 64,
        segment_max_age_secs: 3600,
        group_commit_max_delay_ms: 5,
    };

    let logs_wal_inner = DiskWal::open(hot.join("wal"), log_schema.clone(), wal_cfg.clone())
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

    let metrics_wal_inner = DiskWal::open_arrow(
        hot.join("wal-metrics"),
        metric_schema.arrow.clone(),
        wal_cfg,
    )
    .await
    .unwrap();
    let metrics_wal = Arc::new(BroadcastingWal::new(metrics_wal_inner, 1024));
    let metrics_wal_for_compaction = metrics_wal.clone();

    let query = QueryEngine::new(hot.clone(), log_schema.clone()).unwrap();
    let span_query = SpanQueryEngine::new(hot.clone(), span_schema.clone()).unwrap();
    let metrics_query = MetricsQueryEngine::new(hot.clone(), metric_schema.clone()).unwrap();

    let db_path = hot.join("photon.db");
    let user_store = SqliteUserStore::open(db_path.to_str().unwrap()).unwrap();
    user_store
        .create("admin", &hash_password("admin"))
        .await
        .unwrap();

    // Tenant registry: shares the control-plane SQLite file with the user store (WAL mode
    // allows this, same as production) and the live `TenantTokenMap` with `IngestServer`.
    let tenant_tokens = TenantTokenMap::default();
    let tenant_store: Arc<dyn TenantStore> =
        Arc::new(SqliteTenantStore::open(db_path.to_str().unwrap()).unwrap());
    let tenant_api = TenantApi::new(tenant_store, tenant_tokens.clone())
        .await
        .unwrap();

    let live_hub = LiveHub::new(
        logs_tx,
        spans_tx,
        LiveConfig {
            broadcast_capacity: 1024,
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
        "a-long-random-session-signing-secret-value-for-federation-e2e",
    )
    .with_live_hub(live_hub)
    .with_tenant_store(Some(tenant_api));

    let api_addr = free_addr();
    tokio::spawn(async move {
        api.serve(api_addr).await.unwrap();
    });

    let logs_wal_for_compaction = logs_wal.clone();
    let mut ingest = IngestServer::new(
        logs_wal,
        spans_wal,
        metrics_wal,
        LOCAL_TOKEN.to_string(),
        log_schema.clone(),
        span_schema,
        metric_schema.clone(),
        256,
        16 * 1024 * 1024,
        Arc::new(IngestCounters::new()),
    );
    ingest.tenant_tokens = tenant_tokens;

    let grpc_addr = free_addr();
    let http_addr = free_addr();
    tokio::spawn(async move {
        ingest.serve(grpc_addr, http_addr).await.unwrap();
    });

    wait_until_connectable(api_addr).await;
    wait_until_connectable(http_addr).await;

    CentralServer {
        api_base: format!("http://{api_addr}"),
        ingest_http_base: format!("http://{http_addr}"),
        hot_dir: hot,
        logs_wal: logs_wal_for_compaction,
        log_schema,
        metrics_wal: metrics_wal_for_compaction,
        metric_schema,
        _tmp: tmp,
    }
}

async fn login(server: &CentralServer, username: &str, password: &str) -> String {
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

/// Drain every closed WAL segment for `server`'s logs WAL into hot-store Parquet, so `/api/search`
/// (which reads Parquet, not the WAL) can find rows just posted.
async fn compact_logs(server: &CentralServer) {
    let storage = Storage::from_config(&photon_core::config::StorageConfig {
        hot_dir: server.hot_dir.clone(),
        db_path: String::new(),
        durable: None,
        zstd_level: 1,
    })
    .unwrap();
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = Compactor::new(
        server.logs_wal.clone(),
        storage,
        replicator,
        server.log_schema.clone(),
    );
    let mut compacted = 0;
    while compactor.run_once().await.unwrap().is_some() {
        compacted += 1;
    }
    assert!(compacted >= 1, "expected the logs compactor to run");
}

/// Drain every closed WAL segment for `server`'s metrics WAL into hot-store Parquet.
async fn compact_metrics(server: &CentralServer) {
    let storage = Storage::from_config(&photon_core::config::StorageConfig {
        hot_dir: server.hot_dir.clone(),
        db_path: String::new(),
        durable: None,
        zstd_level: 1,
    })
    .unwrap();
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = photon_compact::MetricsCompactor::new(
        server.metrics_wal.clone(),
        storage,
        replicator,
        server.metric_schema.clone(),
    );
    let mut compacted = 0;
    while compactor.run_once().await.unwrap().is_some() {
        compacted += 1;
    }
    assert!(compacted >= 1, "expected the metrics compactor to run");
}

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

/// A single-record OTLP log export, `service.name = "cpin-app"`, with a spoofed `tenant`
/// resource attribute — the client-supplied value that must never survive server-side stamping.
fn spoofed_log_request(now_nanos: i64) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "cpin-app"), kv("tenant", "cpin")],
                dropped_attributes_count: 0,
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: now_nanos as u64,
                    observed_time_unix_nano: 0,
                    severity_number: SeverityNumber::Info as i32,
                    severity_text: "INFO".to_string(),
                    body: Some(any_str("federated log line")),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

async fn post_otlp_logs(
    client: &reqwest::Client,
    ingest_http_base: &str,
    token: &str,
    req: &ExportLogsServiceRequest,
) -> reqwest::StatusCode {
    client
        .post(format!("{ingest_http_base}/v1/logs"))
        .header("content-type", "application/x-protobuf")
        .header("authorization", format!("Bearer {token}"))
        .body(req.encode_to_vec())
        .send()
        .await
        .unwrap()
        .status()
}

fn gauge_metric(name: &str, value: f64, attrs: Vec<KeyValue>, now: u64) -> Metric {
    Metric {
        name: name.to_string(),
        description: String::new(),
        unit: String::new(),
        metadata: vec![],
        data: Some(Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: attrs,
                start_time_unix_nano: 0,
                time_unix_nano: now,
                exemplars: vec![],
                flags: 0,
                value: Some(NumVal::AsDouble(value)),
            }],
        })),
    }
}

fn sum_metric(name: &str, signal: &str, value: f64, now: u64) -> Metric {
    Metric {
        name: name.to_string(),
        description: String::new(),
        unit: String::new(),
        metadata: vec![],
        data: Some(Data::Sum(Sum {
            data_points: vec![NumberDataPoint {
                attributes: vec![kv("signal", signal)],
                start_time_unix_nano: 0,
                time_unix_nano: now,
                exemplars: vec![],
                flags: 0,
                value: Some(NumVal::AsDouble(value)),
            }],
            aggregation_temporality: AggregationTemporality::Cumulative as i32,
            is_monotonic: true,
        })),
    }
}

/// A minimal `photon.federation.*` summary push (mirrors `federation::otlp::build_summary`,
/// which lives in `photon-server`'s binary-private `federation` module and so isn't reachable
/// from this integration-test crate): `up=1` + one cumulative `ingest.rows` point.
fn federation_summary_request(now_nanos: i64) -> ExportMetricsServiceRequest {
    let now = now_nanos as u64;
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "photon")],
                dropped_attributes_count: 0,
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics: vec![
                    gauge_metric(
                        "photon.federation.up",
                        1.0,
                        vec![kv("mode", "summary")],
                        now,
                    ),
                    sum_metric("photon.federation.ingest.rows", "logs", 42.0, now),
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

#[tokio::test]
async fn federation_stamping_summary_and_rotation() {
    let server = spawn_central().await;
    let client = reqwest::Client::new();
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;

    // 1. Session-auth, register a tenant, capture the minted token.
    let cookie = login(&server, "admin", "admin").await;
    let create_resp = client
        .post(server.url("/api/tenants"))
        .header("cookie", &cookie)
        .json(&serde_json::json!({ "name": "divtik" }))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), reqwest::StatusCode::CREATED);
    let tenant_body: serde_json::Value = create_resp.json().await.unwrap();
    let tenant_token = tenant_body["token"].as_str().unwrap().to_string();

    // 2. POST /v1/logs with the tenant token and a client-spoofed `tenant=cpin` resource attr.
    let status = post_otlp_logs(
        &client,
        &server.ingest_http_base,
        &tenant_token,
        &spoofed_log_request(now_nanos),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK);

    // 3. Compact + search: the stamped `tenant` wins over the spoofed client value.
    compact_logs(&server).await;
    let search_resp = client
        .post(server.url("/api/search"))
        .header("cookie", &cookie)
        .json(&serde_json::json!({
            "start_ts_nanos": (now_nanos - 3_600_000_000_000i64).to_string(),
            "end_ts_nanos": (now_nanos + 3_600_000_000_000i64).to_string(),
            "text": "federated log line",
        }))
        .send()
        .await
        .unwrap();
    assert!(search_resp.status().is_success());
    let search_body: serde_json::Value = search_resp.json().await.unwrap();
    let rows = search_body["rows"].as_array().unwrap();
    assert!(
        !rows.is_empty(),
        "expected the federated log row: {search_body}"
    );
    assert!(
        rows.iter().any(|r| r["attributes"]["tenant"] == "divtik"),
        "expected tenant=divtik (stamp wins over the spoofed value): {rows:?}"
    );
    assert!(
        rows.iter().all(|r| r["attributes"]["tenant"] != "cpin"),
        "the client-spoofed tenant must never survive: {rows:?}"
    );

    // 4. POST /v1/metrics with the tenant token: photon.federation.up + ingest.rows.
    let metrics_status = client
        .post(format!("{}/v1/metrics", server.ingest_http_base))
        .header("content-type", "application/x-protobuf")
        .header("authorization", format!("Bearer {tenant_token}"))
        .body(federation_summary_request(now_nanos).encode_to_vec())
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(metrics_status, reqwest::StatusCode::OK);
    compact_metrics(&server).await;

    // 5. GET /api/tenants/summary → divtik reports "up", the pushed mode, rate >= 0.
    let summary_resp = client
        .get(server.url("/api/tenants/summary"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(summary_resp.status().is_success());
    let summary: serde_json::Value = summary_resp.json().await.unwrap();
    let divtik = summary
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "divtik")
        .unwrap_or_else(|| panic!("expected a divtik entry: {summary}"));
    assert_eq!(divtik["status"], "up", "expected divtik up: {divtik}");
    assert_eq!(divtik["mode"], "summary");
    assert!(
        divtik["ingest_rows_per_sec"].as_f64().unwrap() >= 0.0,
        "expected a non-negative rate: {divtik}"
    );

    // 6. Unknown token -> 401 on /v1/logs; the tenant token -> 401 on /api/v1/write (v1
    // remote-write only accepts the local token, per the plan's Global Constraints).
    let unknown_status = post_otlp_logs(
        &client,
        &server.ingest_http_base,
        "not-a-real-token",
        &spoofed_log_request(now_nanos),
    )
    .await;
    assert_eq!(unknown_status, reqwest::StatusCode::UNAUTHORIZED);

    let promrw_status = client
        .post(format!("{}/api/v1/write", server.ingest_http_base))
        .header("authorization", format!("Bearer {tenant_token}"))
        .body(Vec::<u8>::new())
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(promrw_status, reqwest::StatusCode::UNAUTHORIZED);

    // 7. Rotate the token: the old one stops working, the new one is accepted.
    let rotate_resp = client
        .post(server.url("/api/tenants/divtik/rotate-token"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(rotate_resp.status(), reqwest::StatusCode::OK);
    let rotate_body: serde_json::Value = rotate_resp.json().await.unwrap();
    let new_token = rotate_body["token"].as_str().unwrap().to_string();
    assert_ne!(new_token, tenant_token);

    let old_token_status = post_otlp_logs(
        &client,
        &server.ingest_http_base,
        &tenant_token,
        &spoofed_log_request(now_nanos),
    )
    .await;
    assert_eq!(old_token_status, reqwest::StatusCode::UNAUTHORIZED);

    let new_token_status = post_otlp_logs(
        &client,
        &server.ingest_http_base,
        &new_token,
        &spoofed_log_request(now_nanos),
    )
    .await;
    assert_eq!(new_token_status, reqwest::StatusCode::OK);
}

//! End-to-end test for the curated `/api/infra/*` vertical (Phase 3): an OTLP `/v1/metrics` POST
//! carrying `host.name` resource attributes for two hosts becomes queryable through
//! `photon_query::infra`'s curated engine methods over the real over-the-wire stack — mirrors
//! `e2e.rs`'s `spawn_live_server` harness (real `IngestServer` + `ApiServer` on loopback ports,
//! `MetricsCompactor` driven directly since this harness runs no background compactor loop).

use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, number_data_point::Value as NumVal, Gauge, Metric, NumberDataPoint,
    ResourceMetrics, ScopeMetrics,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message as _;

use photon_api::users::{SqliteUserStore, UserStore};
use photon_api::ApiServer;
use photon_compact::MetricsCompactor;
use photon_core::config::{StorageConfig, WalConfig};
use photon_core::metric_schema::MetricSchema;
use photon_core::schema::LogSchema;
use photon_core::span_schema::SpanSchema;
use photon_ingest::IngestServer;
use photon_query::{MetricsQueryEngine, QueryEngine, SpanQueryEngine};
use photon_storage::{Replicator, Storage};
use photon_wal::DiskWal;

const INGEST_TOKEN: &str = "infra-e2e-ingest-token";
const CPU_METRIC: &str = "system.cpu.utilization";

fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64
}

fn kv(k: &str, v: &str) -> KeyValue {
    KeyValue {
        key: k.into(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(v.into())),
        }),
    }
}

/// A `system.cpu.utilization` gauge point (`cpu=total`) for one host, at `time_unix_nano`.
fn cpu_point(time_unix_nano: u64, value: f64) -> NumberDataPoint {
    NumberDataPoint {
        attributes: vec![kv("cpu", "total")],
        start_time_unix_nano: 0,
        time_unix_nano,
        exemplars: vec![],
        flags: 0,
        value: Some(NumVal::AsDouble(value)),
    }
}

/// One `ExportMetricsServiceRequest` carrying a `system.cpu.utilization` gauge for each of
/// `hosts`, resource-scoped by `host.name` (+ a shared `service.name`) — the exact shape a real
/// `photon-agent` would emit for the CPU headline metric (Global Constants).
fn cpu_util_points(hosts: &[&str], time_unix_nano: u64, value: f64) -> ExportMetricsServiceRequest {
    let resource_metrics = hosts
        .iter()
        .map(|host| ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "host-agent"), kv("host.name", host)],
                dropped_attributes_count: 0,
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics: vec![Metric {
                    name: CPU_METRIC.into(),
                    description: String::new(),
                    unit: "1".into(),
                    metadata: vec![],
                    data: Some(Data::Gauge(Gauge {
                        data_points: vec![cpu_point(time_unix_nano, value)],
                    })),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        })
        .collect();
    ExportMetricsServiceRequest { resource_metrics }
}

/// A minimal, distinctly-named metrics request whose only purpose is to force a second WAL round
/// on the (tiny-`segment_max_bytes`) metrics WAL — mirrors `e2e.rs`'s `seal_write_request`: once
/// this POST's response is observed, the WAL writer's sequential round processing guarantees the
/// previous round's segment-close check already ran, so `compact_metrics` finds a closed segment.
fn seal_points(time_unix_nano: u64) -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "__seal__"), kv("host.name", "__seal__")],
                dropped_attributes_count: 0,
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics: vec![Metric {
                    name: "__seal__".into(),
                    description: String::new(),
                    unit: "1".into(),
                    metadata: vec![],
                    data: Some(Data::Gauge(Gauge {
                        data_points: vec![cpu_point(time_unix_nano, 0.0)],
                    })),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

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

/// A fully wired, real over-the-wire Photon stack: an OTLP/HTTP ingest front end and the
/// REST/UI `ApiServer`, both on loopback ports, over a tempdir hot store. No background
/// compactor runs (mirrors `e2e.rs`'s `LiveServer`) — `compact_metrics` drains the metrics WAL
/// on demand before a test queries `/api/infra/*`.
struct InfraServer {
    api_base: String,
    ingest_http_base: String,
    hot_dir: PathBuf,
    metrics_wal: Arc<DiskWal>,
    metric_schema: MetricSchema,
    _tmp: tempfile::TempDir,
}

impl InfraServer {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }
}

async fn spawn_infra_server() -> InfraServer {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();

    let log_schema = LogSchema::new(&["service.name".to_string()]);
    let span_schema = SpanSchema::new(&["service.name".to_string()]);
    // `host.name` promoted — the whole point of this vertical: infra_hosts/infra_host_detail/
    // infra_host_series group and prune by this column (Phase 1's compactor sort key + skip-index
    // host range).
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
    let spans_wal = Arc::new(
        DiskWal::open_arrow(
            hot.join("wal-traces"),
            span_schema.arrow.clone(),
            wal_cfg.clone(),
        )
        .await
        .unwrap(),
    );

    // A tiny `segment_max_bytes` so a real metrics POST immediately exceeds the threshold and
    // closes its segment (mirrors `e2e.rs`'s `spawn_live_server`), letting `compact_metrics` find
    // it after the `seal_points` follow-up POST.
    let metrics_wal_cfg = WalConfig {
        segment_max_bytes: 64,
        segment_max_age_secs: 3600,
        group_commit_max_delay_ms: 5,
    };
    let metrics_wal = Arc::new(
        DiskWal::open_arrow(
            hot.join("wal-metrics"),
            metric_schema.arrow.clone(),
            metrics_wal_cfg,
        )
        .await
        .unwrap(),
    );
    // A second handle kept for `compact_metrics`, before the original is moved into `IngestServer`.
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

    let api = ApiServer::new(
        query,
        span_query,
        metrics_query,
        Arc::new(user_store) as Arc<dyn UserStore>,
        "a-long-random-session-signing-secret-value-for-infra-e2e",
    );

    let api_addr = free_addr();
    tokio::spawn(async move {
        api.serve(api_addr).await.unwrap();
    });

    let ingest = IngestServer::new(
        logs_wal,
        spans_wal,
        metrics_wal,
        INGEST_TOKEN.to_string(),
        log_schema,
        span_schema,
        metric_schema.clone(),
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

    InfraServer {
        api_base: format!("http://{api_addr}"),
        ingest_http_base: format!("http://{http_addr}"),
        hot_dir: hot,
        metrics_wal: metrics_wal_for_compaction,
        metric_schema,
        _tmp: tmp,
    }
}

/// Argon2 PHC hash with an OS-random salt — mirrors `e2e.rs`'s harness (photon-api's test-only
/// seeding helpers are `pub(crate)`, invisible outside that crate).
fn hash_password(password: &str) -> String {
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::SaltString;
    use argon2::{Argon2, PasswordHasher};
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

async fn login(server: &InfraServer, username: &str, password: &str) -> String {
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

async fn post_otlp_metrics(ingest_http_base: &str, req: &ExportMetricsServiceRequest) {
    let resp = reqwest::Client::new()
        .post(format!("{ingest_http_base}/v1/metrics"))
        .header("content-type", "application/x-protobuf")
        .header("authorization", format!("Bearer {INGEST_TOKEN}"))
        .body(req.encode_to_vec())
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "otlp metrics post failed: {}",
        resp.status()
    );
}

/// Drain every currently-closed metrics WAL segment into `server`'s hot store — the metrics
/// analogue of `e2e.rs`'s `compact_metrics` (this harness runs no background compactor loop, so a
/// test that posts through `/v1/metrics` must call this before querying `/api/infra/*`).
async fn compact_metrics(server: &InfraServer) {
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

async fn get_json(server: &InfraServer, path: &str, cookie: &str) -> serde_json::Value {
    let resp = reqwest::Client::new()
        .get(server.url(path))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "GET {path} failed: {}",
        resp.status()
    );
    resp.json().await.unwrap()
}

#[tokio::test]
async fn infra_endpoints_report_ingested_host_metrics() {
    let server = spawn_infra_server().await;

    let now = now_nanos() as u64;
    post_otlp_metrics(
        &server.ingest_http_base,
        &cpu_util_points(&["web-1", "web-2"], now, 0.42),
    )
    .await;
    // Force a second WAL round so the (tiny-segment) round above is guaranteed closed by the
    // time `compact_metrics` looks for it — see `seal_points`'s doc comment.
    post_otlp_metrics(&server.ingest_http_base, &seal_points(now + 1)).await;
    compact_metrics(&server).await;

    let cookie = login(&server, "admin", "admin").await;

    // GET /api/infra/hosts — both hosts show up with a CPU reading.
    let hosts = get_json(
        &server,
        "/api/infra/hosts?start=0&end=9223372036854775807",
        &cookie,
    )
    .await;
    let rows = hosts["hosts"]
        .as_array()
        .expect("hosts should be a JSON array");
    let names: Vec<&str> = rows.iter().map(|h| h["host"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"web-1") && names.contains(&"web-2"),
        "expected both hosts in the list: {hosts}"
    );
    let web1 = rows.iter().find(|h| h["host"] == "web-1").unwrap();
    assert!(
        web1["cpuUtil"]
            .as_f64()
            .is_some_and(|v| (v - 0.42).abs() < 1e-9),
        "expected web-1's cpuUtil ≈ 0.42: {web1}"
    );
    assert!(
        web1["lastSeenNs"].is_string(),
        "lastSeenNs should be a ns string: {web1}"
    );

    // GET /api/infra/hosts/:host — per-host detail.
    let detail = get_json(
        &server,
        "/api/infra/hosts/web-1?start=0&end=9223372036854775807",
        &cookie,
    )
    .await;
    assert_eq!(detail["host"], "web-1");
    assert!(detail["lastSeenNs"].is_string());

    // GET /api/infra/hosts/:host/timeseries?resource=cpu — scoped to web-1 only.
    let ts = get_json(
        &server,
        "/api/infra/hosts/web-1/timeseries?resource=cpu&start=0&end=9223372036854775807",
        &cookie,
    )
    .await;
    assert_eq!(ts["resource"], "cpu");
    let series = ts["series"].as_array().expect("series should be an array");
    assert!(!series.is_empty(), "expected at least one series: {ts}");
}

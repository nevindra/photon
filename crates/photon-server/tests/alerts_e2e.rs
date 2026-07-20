//! Task 10 end-to-end: the webhook-alert engine trigger→resolve lifecycle over the *real*
//! delivery path, plus real-data value extraction for the metrics + traces `ConditionSource`.
//!
//! Three independent proofs:
//!   * `alert_triggers_and_resolves_end_to_end` boots a real `ApiServer` wired with `with_alerts`
//!     (a `MemStore` + a controllable fake `ConditionSource`), creates a channel + rule over HTTP,
//!     exercises the controllable source through `POST /api/alerts/rules/:id/test`, then drives one
//!     `photon_alerts::scheduler::process_sample` at value 0.95 (breaching) and one at 0.1
//!     (recovering) through a *real* `WebhookNotifier`. A `tokio`-spawned loopback HTTP recorder
//!     captures the delivered bodies: the first is `"status":"triggered"` (incident open), the
//!     second `"status":"resolved"` (incident closed) — asserted both against the store and the
//!     `GET /api/alerts/incidents` REST surface.
//!   * `engine_metrics_sample_extracts_breaching_value` populates a real `MetricsQueryEngine`
//!     (WAL → `MetricsCompactor` → hot Parquet) with one gauge point above threshold and reproduces
//!     the exact `MetricSeriesRequest` + reduction `EngineConditionSource::sample_metrics` performs,
//!     asserting the derived `SeriesSample` value + breaching (it mirrors the reduction rather than
//!     importing the type — kept as originally written).
//!   * `engine_traces_sample_returns_error_rate_percentage` populates a real `SpanQueryEngine` with
//!     20 spans (1 error) and drives the *real* `EngineConditionSource::sample` — imported via this
//!     crate's library target (`src/lib.rs`) — asserting the error rate comes back as the percentage
//!     `5.0`, not the 0–1 fraction `0.05`. This is the coverage that was missing when the unit
//!     mismatch shipped; it fails before the `* 100.0` fix and passes after.

use std::collections::{BTreeMap, HashMap};
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use photon_alerts::model::{
    now_ms, Cmp, Condition, Rule, SchedulerCommand, SeriesSample, SeriesState, TraceCondition,
    TraceKind,
};
use photon_alerts::notify::WebhookNotifier;
use photon_alerts::scheduler::process_sample;
use photon_alerts::source::ConditionSource;
use photon_alerts::store::mem::MemStore;
use photon_alerts::store::AlertStore;
use photon_core::PhotonError;

use photon_api::alerts::AlertsApi;
use photon_api::users::{SqliteUserStore, UserStore};
use photon_api::ApiServer;

use photon_compact::{MetricsCompactor, SpanCompactor};
use photon_core::config::{StorageConfig, WalConfig};
use photon_core::metric_agg::Agg;
use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
use photon_core::metric_schema::{metric_type, MetricSchema};
use photon_core::schema::LogSchema;
use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
use photon_core::span_schema::SpanSchema;
use photon_query::{MetricSeriesRequest, MetricsQueryEngine, QueryEngine, SpanQueryEngine};
use photon_server::alerts_source::EngineConditionSource;
use photon_storage::{Replicator, Storage};
use photon_wal::DiskWal;

const SESSION_SECRET: &str = "a-long-random-session-signing-secret-value-for-alerts-e2e";

// ---------------------------------------------------------------------------
// A controllable fake `ConditionSource`: `sample` returns whatever series the test set. Lets the
// REST `/test` path be driven deterministically without any real query engine.
// ---------------------------------------------------------------------------

struct FakeSource {
    series: Mutex<Vec<SeriesSample>>,
}
impl FakeSource {
    fn new() -> Self {
        Self {
            series: Mutex::new(Vec::new()),
        }
    }
    fn set(&self, series: Vec<SeriesSample>) {
        *self.series.lock().unwrap() = series;
    }
}
#[async_trait]
impl ConditionSource for FakeSource {
    async fn sample(
        &self,
        _cond: &Condition,
        _now_ms: i64,
    ) -> Result<Vec<SeriesSample>, PhotonError> {
        Ok(self.series.lock().unwrap().clone())
    }
}

// ---------------------------------------------------------------------------
// A minimal loopback HTTP recorder: accepts each webhook POST, parses the Content-Length body as
// JSON, and appends it to a shared Vec. Raw TCP (no extra dep) — `WebhookNotifier` sends HTTP/1.1
// with a fixed Content-Length body, so parsing is straightforward.
// ---------------------------------------------------------------------------

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn header_content_length(headers: &[u8]) -> usize {
    let text = String::from_utf8_lossy(headers);
    for line in text.split("\r\n") {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                return v.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

/// Bind a loopback port, spawn an accept loop recording every POST body, and return
/// `(url, recorded)`. Each connection is served once and closed (`Connection: close`).
async fn spawn_webhook_recorder() -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = recorded.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(hdr_end) = find_subslice(&buf, b"\r\n\r\n") {
                        let body_start = hdr_end + 4;
                        let len = header_content_length(&buf[..hdr_end]);
                        if buf.len() >= body_start + len {
                            if let Ok(v) =
                                serde_json::from_slice::<Value>(&buf[body_start..body_start + len])
                            {
                                sink.lock().unwrap().push(v);
                            }
                            break;
                        }
                    }
                }
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
                let _ = sock.flush().await;
            });
        }
    });
    (format!("http://{addr}/"), recorded)
}

/// Poll `recorded` until it holds at least `n` bodies, or panic after 5s. Delivery is
/// fire-and-forget (a detached task inside `WebhookNotifier::deliver`), so a bounded wait is needed.
async fn wait_for_bodies(recorded: &Arc<Mutex<Vec<Value>>>, n: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        {
            let g = recorded.lock().unwrap();
            if g.len() >= n {
                return g.clone();
            }
        }
        if Instant::now() >= deadline {
            let got = recorded.lock().unwrap().len();
            panic!("timed out waiting for {n} webhook deliveries (got {got})");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// Live `ApiServer` harness (mirrors `rum_e2e.rs::spawn_rum_server`).
// ---------------------------------------------------------------------------

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

struct AlertsServer {
    api_base: String,
    _tmp: tempfile::TempDir,
}
impl AlertsServer {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }
}

/// A real `ApiServer` on a loopback port, wired via `with_alerts` to the shared `store` + the
/// controllable `source`. Empty query engines (never queried here) satisfy `ApiServer::new`.
async fn spawn_alerts_server(
    store: Arc<MemStore>,
    cmd_tx: mpsc::Sender<SchedulerCommand>,
    source: Arc<FakeSource>,
) -> AlertsServer {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();

    let query =
        QueryEngine::new(hot.clone(), LogSchema::new(&["service.name".to_string()])).unwrap();
    let span_query =
        SpanQueryEngine::new(hot.clone(), SpanSchema::new(&["service.name".to_string()])).unwrap();
    let metrics_query = MetricsQueryEngine::new(
        hot.clone(),
        MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]),
    )
    .unwrap();
    let user_store = SqliteUserStore::open(hot.join("photon.db").to_str().unwrap()).unwrap();

    let alerts = AlertsApi {
        store: store as Arc<dyn AlertStore>,
        cmd_tx,
        source: source as Arc<dyn ConditionSource>,
    };

    let api = ApiServer::new(
        query,
        span_query,
        metrics_query,
        Arc::new(user_store) as Arc<dyn UserStore>,
        SESSION_SECRET,
    )
    .with_alerts(Some(alerts));

    let addr = free_addr();
    tokio::spawn(async move {
        api.serve(addr).await.unwrap();
    });
    wait_until_connectable(addr).await;

    AlertsServer {
        api_base: format!("http://{addr}"),
        _tmp: tmp,
    }
}

/// Run first-run onboarding and return the signed session cookie (`name=value`). The workspace
/// `reqwest` has no `cookies` feature, so we thread the cookie by hand (same as `rum_e2e.rs`).
async fn setup_cookie(client: &reqwest::Client, server: &AlertsServer) -> String {
    let setup = client
        .post(server.url("/api/setup"))
        .json(&json!({ "username": "admin", "password": "hunter2-long-enough" }))
        .send()
        .await
        .unwrap();
    assert!(
        setup.status().is_success(),
        "onboarding setup should succeed, got {}",
        setup.status()
    );
    setup
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .expect("setup should set a session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn incident_count(client: &reqwest::Client, server: &AlertsServer, cookie: &str) -> usize {
    let resp = client
        .get(server.url("/api/alerts/incidents?status=triggered"))
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.json::<Value>()
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .len()
}

#[tokio::test]
async fn alert_triggers_and_resolves_end_to_end() {
    // (1) A loopback recorder standing in for the customer's webhook endpoint.
    let (webhook_url, recorded) = spawn_webhook_recorder().await;

    // (2) The shared store + controllable source + command channel that back the alerts subsystem.
    let store = Arc::new(MemStore::new());
    let (cmd_tx, _cmd_rx) = mpsc::channel::<SchedulerCommand>(64);
    let source = Arc::new(FakeSource::new());

    // (3) Boot a real ApiServer wired with `with_alerts` over that store + source.
    let server = spawn_alerts_server(store.clone(), cmd_tx, source.clone()).await;
    let client = reqwest::Client::new();

    // The alerts routes are MOUNTED behind session auth: unauthenticated ⇒ 401 (not 404).
    let unauth = client
        .get(server.url("/api/alerts/rules"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        unauth.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "/api/alerts/rules must be mounted (401), not absent (404)"
    );

    let cookie = setup_cookie(&client, &server).await;

    // (4) Create a channel pointing at the recorder, over HTTP.
    let ch = client
        .post(server.url("/api/alerts/channels"))
        .header(reqwest::header::COOKIE, &cookie)
        .json(&json!({ "name": "ops", "config": { "type": "webhook", "url": webhook_url } }))
        .send()
        .await
        .unwrap();
    assert_eq!(ch.status(), reqwest::StatusCode::OK);
    let channel_id = ch.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // (5) Create a metrics rule referencing it: `for_secs: 0` (trigger on the first breach),
    // threshold 0.9, `cmp: gt`, grouped by host.name.
    let rule_resp = client
        .post(server.url("/api/alerts/rules"))
        .header(reqwest::header::COOKIE, &cookie)
        .json(&json!({
            "name": "high cpu",
            "condition": {
                "signal": "metrics", "metric_name": "system.cpu.utilization",
                "group_by": ["host.name"], "agg": "avg",
                "window_secs": 60, "cmp": "gt", "threshold": 0.9
            },
            "for_secs": 0,
            "channel_ids": [channel_id],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rule_resp.status(), reqwest::StatusCode::OK);
    let rule: Rule = rule_resp.json().await.unwrap();

    // The controllable source drives the REST `/test` preview: set one breaching series and assert
    // the dry-run reflects value + breaching (exercises the `ConditionSource` seam over HTTP).
    source.set(vec![SeriesSample {
        key: vec![("host.name".to_string(), "web-01".to_string())],
        value: 0.95,
    }]);
    let test_resp = client
        .post(server.url(&format!("/api/alerts/rules/{}/test", rule.id)))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(test_resp.status(), reqwest::StatusCode::OK);
    let tv: Value = test_resp.json().await.unwrap();
    assert_eq!(tv["series"][0]["value"].as_f64(), Some(0.95));
    assert_eq!(tv["series"][0]["breaching"].as_bool(), Some(true));

    // (6) Drive a breaching sample through the domain scheduler + a REAL WebhookNotifier.
    let notifier = WebhookNotifier::new();
    let mut states: HashMap<String, SeriesState> = HashMap::new();
    let breaching = vec![SeriesSample {
        key: vec![("host.name".to_string(), "web-01".to_string())],
        value: 0.95,
    }];
    process_sample(&*store, &notifier, &rule, &mut states, breaching, now_ms())
        .await
        .unwrap();

    // The triggered webhook lands on the recorder with the expected shape.
    let bodies = wait_for_bodies(&recorded, 1).await;
    assert_eq!(bodies[0]["status"].as_str(), Some("triggered"));
    assert_eq!(bodies[0]["value"].as_f64(), Some(0.95));
    assert_eq!(bodies[0]["threshold"].as_f64(), Some(0.9));
    assert_eq!(bodies[0]["rule"]["name"].as_str(), Some("high cpu"));
    assert_eq!(bodies[0]["series"]["host.name"].as_str(), Some("web-01"));

    // An incident is open — via the store and the REST surface.
    assert_eq!(store.list_open_incidents().await.unwrap().len(), 1);
    assert_eq!(incident_count(&client, &server, &cookie).await, 1);

    // (7) Drive a recovering sample: the same series drops below threshold ⇒ resolve.
    let recovering = vec![SeriesSample {
        key: vec![("host.name".to_string(), "web-01".to_string())],
        value: 0.1,
    }];
    process_sample(
        &*store,
        &notifier,
        &rule,
        &mut states,
        recovering,
        now_ms() + 1000,
    )
    .await
    .unwrap();

    let bodies = wait_for_bodies(&recorded, 2).await;
    assert_eq!(bodies[1]["status"].as_str(), Some("resolved"));

    // The incident is closed — via the store and the REST surface.
    assert_eq!(store.list_open_incidents().await.unwrap().len(), 0);
    assert_eq!(incident_count(&client, &server, &cookie).await, 0);
}

// ---------------------------------------------------------------------------
// Real-data metrics extraction — the value/breaching contract of the metrics ConditionSource.
// ---------------------------------------------------------------------------

const T0: i64 = 1_000_000_000_000; // window start (ns)
const WINDOW_NS: i64 = 60_000_000_000; // 60s window
const END: i64 = T0 + WINDOW_NS;

/// Ingest one metric point → compact to hot Parquet → a populated `MetricsQueryEngine`. Mirrors
/// `rum_e2e.rs` / `photon-query/tests/metric_query.rs`: a `segment_max_bytes: 1` WAL plus a trailing
/// seal append closes the data segment so the compactor drains it.
async fn engine_with_metric(
    schema: &MetricSchema,
    point: MetricPoint,
) -> (MetricsQueryEngine, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();
    let wal_cfg = WalConfig {
        segment_max_bytes: 1,
        segment_max_age_secs: 0,
        group_commit_max_delay_ms: 0,
    };
    let wal = Arc::new(
        DiskWal::open_arrow(hot.join("wal-metrics"), schema.arrow.clone(), wal_cfg)
            .await
            .unwrap(),
    );

    let mut b = MetricBatchBuilder::new(schema);
    b.append(&point);
    wal.append(b.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();

    // Trailing seal (distinct name, ts far before the window) closes the data segment.
    let mut seal_attrs = BTreeMap::new();
    seal_attrs.insert("service.name".to_string(), "__seal__".to_string());
    let seal = MetricPoint {
        metric_name: "__seal__".to_string(),
        metric_type: metric_type::GAUGE,
        timestamp_nanos: 1,
        value: Some(0.0),
        attributes: seal_attrs,
        ..MetricPoint::default()
    };
    let mut t = MetricBatchBuilder::new(schema);
    t.append(&seal);
    wal.append(t.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();

    let storage = Storage::from_config(&StorageConfig {
        hot_dir: hot.clone(),
        db_path: String::new(),
        durable: None,
        zstd_level: 1,
    })
    .unwrap();
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = MetricsCompactor::new(wal.clone(), storage, replicator, schema.clone());
    while compactor.run_once().await.unwrap().is_some() {}

    (MetricsQueryEngine::new(hot, schema.clone()).unwrap(), tmp)
}

#[tokio::test]
async fn engine_metrics_sample_extracts_breaching_value() {
    let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
    let mut attrs = BTreeMap::new();
    attrs.insert("service.name".to_string(), "web".to_string());
    attrs.insert("host.name".to_string(), "web-01".to_string());
    let point = MetricPoint {
        metric_name: "system.cpu.utilization".to_string(),
        metric_type: metric_type::GAUGE,
        timestamp_nanos: T0 + WINDOW_NS / 2,
        value: Some(0.95),
        attributes: attrs,
        ..MetricPoint::default()
    };
    let (engine, _tmp) = engine_with_metric(&schema, point).await;

    // The exact request `EngineConditionSource::sample_metrics` builds: one bucket over the window,
    // the mapped agg (avg), grouped by service.name.
    let result = engine
        .query_series(MetricSeriesRequest {
            metric: "system.cpu.utilization".to_string(),
            agg: Some(Agg::Avg),
            group_by: vec!["service.name".to_string()],
            filter: None,
            start_ts_nanos: T0,
            end_ts_nanos: END,
            buckets: 1,
        })
        .await
        .unwrap();

    assert_eq!(result.series.len(), 1, "one grouped series expected");
    let s = &result.series[0];

    // Reduce exactly as `sample_metrics`: one bucket ⇒ one point; its value is the window aggregate.
    let value = s
        .points
        .last()
        .and_then(|p| p.v)
        .expect("the in-window point yields a window aggregate");
    let key: Vec<(String, String)> = s.labels.clone().into_iter().collect();
    let sample = SeriesSample { key, value };

    assert!(
        (sample.value - 0.95).abs() < 1e-9,
        "expected the ingested gauge value, got {}",
        sample.value
    );
    assert_eq!(
        sample.key,
        vec![("service.name".to_string(), "web".to_string())],
        "the group-by label identifies the series"
    );
    // threshold 0.9, cmp `gt` ⇒ 0.95 breaches.
    assert!(
        Cmp::Gt.test(sample.value, 0.9),
        "0.95 must breach a `> 0.9` condition"
    );
}

// ---------------------------------------------------------------------------
// Real-data traces extraction — the error-rate UNIT contract of the traces ConditionSource.
// Unlike the metrics test above (which mirrors the reduction), this one drives the *real*
// `EngineConditionSource::sample` over a populated `SpanQueryEngine` — the coverage gap that let the
// "0–1 fraction vs. percentage" bug ship. It fails before the `* 100.0` fix and passes after.
// ---------------------------------------------------------------------------

/// Ingest a batch of spans → compact to hot Parquet → a populated `SpanQueryEngine`. Same
/// `segment_max_bytes: 1` WAL + trailing-seal trick as `engine_with_metric`.
async fn engine_with_spans(
    schema: &SpanSchema,
    spans: Vec<SpanRecord>,
) -> (SpanQueryEngine, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();
    let wal_cfg = WalConfig {
        segment_max_bytes: 1,
        segment_max_age_secs: 0,
        group_commit_max_delay_ms: 0,
    };
    let wal = Arc::new(
        DiskWal::open_arrow(hot.join("wal-spans"), schema.arrow.clone(), wal_cfg)
            .await
            .unwrap(),
    );

    let mut b = SpanBatchBuilder::new(schema);
    for s in &spans {
        b.append(s);
    }
    wal.append(b.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();

    // Trailing seal (distinct service, ts far before the window) closes the data segment.
    let mut seal_attrs = BTreeMap::new();
    seal_attrs.insert("service.name".to_string(), "__seal__".to_string());
    let seal = SpanRecord {
        trace_id: "seal".to_string(),
        span_id: "seal".to_string(),
        start_time_nanos: 1,
        attributes: seal_attrs,
        ..SpanRecord::default()
    };
    let mut t = SpanBatchBuilder::new(schema);
    t.append(&seal);
    wal.append(t.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();

    let storage = Storage::from_config(&StorageConfig {
        hot_dir: hot.clone(),
        db_path: String::new(),
        durable: None,
        zstd_level: 1,
    })
    .unwrap();
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = SpanCompactor::new(wal.clone(), storage, replicator, schema.clone());
    while compactor.run_once().await.unwrap().is_some() {}

    (SpanQueryEngine::new(hot, schema.clone()).unwrap(), tmp)
}

#[tokio::test]
async fn engine_traces_sample_returns_error_rate_percentage() {
    // 20 spans for service `web`, exactly one of them an OTEL ERROR (status_code 2) ⇒ a 5% error
    // rate — a ratio the create dialog enters as the percentage `5` with a `%` unit label.
    let schema = SpanSchema::new(&["service.name".to_string()]);
    let mut spans = Vec::new();
    for i in 0..20u32 {
        let mut attrs = BTreeMap::new();
        attrs.insert("service.name".to_string(), "web".to_string());
        spans.push(SpanRecord {
            trace_id: format!("t{i}"),
            span_id: format!("s{i}"),
            name: Some("GET /checkout".to_string()),
            start_time_nanos: T0 + WINDOW_NS / 2,
            end_time_nanos: Some(T0 + WINDOW_NS / 2 + 1_000_000),
            duration_nanos: Some(1_000_000),
            status_code: Some(if i == 0 { 2 } else { 1 }), // one ERROR, nineteen OK
            attributes: attrs,
            ..SpanRecord::default()
        });
    }
    let (spans_engine, tmp) = engine_with_spans(&schema, spans).await;

    // The logs/metrics engines are unused for a traces condition; empty ones over the same hot dir.
    let hot = tmp.path().to_path_buf();
    let logs =
        QueryEngine::new(hot.clone(), LogSchema::new(&["service.name".to_string()])).unwrap();
    let metrics = MetricsQueryEngine::new(
        hot,
        MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]),
    )
    .unwrap();
    let source = EngineConditionSource::new(logs, spans_engine, metrics);

    // Window [T0, END]: sample() takes now_ms (→ now_ns = END) and subtracts window_secs (60s = T0).
    let now_ms = END / 1_000_000;
    let cond = Condition::Traces(TraceCondition {
        service: "web".to_string(),
        operation: None,
        kind: TraceKind::ErrorRate,
        window_secs: 60,
        cmp: Cmp::Gt,
        threshold: 4.0,
    });
    let out = source.sample(&cond, now_ms).await.unwrap();

    assert_eq!(out.len(), 1, "one series for service `web`");
    let s = &out[0];
    assert_eq!(
        s.key,
        vec![("service.name".to_string(), "web".to_string())],
        "the series is keyed by service.name"
    );
    // Post-Fix-1: the error rate is a PERCENTAGE (1/20 = 5.0), NOT the 0–1 fraction 0.05.
    assert!(
        (s.value - 5.0).abs() < 1e-6,
        "expected a 5.0% error rate, got {} (0.05 would mean the fraction bug is back)",
        s.value
    );
    // threshold 4(%), cmp `gt` ⇒ 5% breaches; the buggy 0.05 fraction would NOT.
    assert!(
        Cmp::Gt.test(s.value, 4.0),
        "5% must breach a `> 4%` condition (0.05 would not)"
    );
}

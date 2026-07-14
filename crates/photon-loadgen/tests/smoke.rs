//! Integration smoke tests: POST a generated batch to a tiny in-process axum receiver and
//! confirm the request carries the bearer header and a decodable OTLP request with the expected
//! record/span count — i.e. the bytes we send are shaped for the real receiver. One test per
//! signal (logs, traces).

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use photon_loadgen::config::SpanRange;
use photon_loadgen::logs::build_batch;
use photon_loadgen::traces::build_request_bytes;
use prost::Message;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Captured {
    requests: usize,
    auth: Option<String>,
    records: usize,
}

type Shared = Arc<Mutex<Captured>>;

async fn handler(State(cap): State<Shared>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let mut c = cap.lock().unwrap();
    c.requests += 1;
    c.auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Ok(req) = ExportLogsServiceRequest::decode(&body[..]) {
        c.records += req
            .resource_logs
            .iter()
            .flat_map(|rl| rl.scope_logs.iter())
            .map(|sl| sl.log_records.len())
            .sum::<usize>();
    }
    StatusCode::OK
}

#[tokio::test]
async fn posts_well_formed_authenticated_otlp() {
    let captured: Shared = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route("/v1/logs", post(handler))
        .with_state(captured.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{addr}/v1/logs");
    let token = "smoke-token";

    let mut rng = SmallRng::seed_from_u64(42);
    let body = build_batch(20, 3, &mut rng);

    let resp = reqwest::Client::new()
        .post(&endpoint)
        .bearer_auth(token)
        .header("content-type", "application/x-protobuf")
        .body(body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let c = captured.lock().unwrap();
    assert_eq!(c.requests, 1);
    assert_eq!(c.auth.as_deref(), Some("Bearer smoke-token"));
    assert_eq!(c.records, 20);
}

#[derive(Default)]
struct TraceCaptured {
    requests: usize,
    auth: Option<String>,
    spans: usize,
}

type TraceShared = Arc<Mutex<TraceCaptured>>;

async fn trace_handler(
    State(cap): State<TraceShared>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let mut c = cap.lock().unwrap();
    c.requests += 1;
    c.auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Ok(req) = ExportTraceServiceRequest::decode(&body[..]) {
        c.spans += req
            .resource_spans
            .iter()
            .flat_map(|rs| rs.scope_spans.iter())
            .map(|ss| ss.spans.len())
            .sum::<usize>();
    }
    StatusCode::OK
}

#[tokio::test]
async fn posts_well_formed_authenticated_traces() {
    let captured: TraceShared = Arc::new(Mutex::new(TraceCaptured::default()));
    let app = Router::new()
        .route("/v1/traces", post(trace_handler))
        .with_state(captured.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{addr}/v1/traces");
    let token = "smoke-token";

    let mut rng = SmallRng::seed_from_u64(99);
    let (body, spans) = build_request_bytes(3, 4, SpanRange { min: 4, max: 10 }, &mut rng);

    let resp = reqwest::Client::new()
        .post(&endpoint)
        .bearer_auth(token)
        .header("content-type", "application/x-protobuf")
        .body(body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let c = captured.lock().unwrap();
    assert_eq!(c.requests, 1);
    assert_eq!(c.auth.as_deref(), Some("Bearer smoke-token"));
    assert_eq!(c.spans as u64, spans);
    assert!(spans > 0);
}

//! The resource sampler loop: sample the host (+ GPU) on a fixed interval and POST the
//! OTLP/HTTP protobuf payload, mirroring `photon-loadgen/src/worker.rs`'s POST shape (bearer
//! auth, `application/x-protobuf`, prost-encoded body). A 10s request timeout keeps a hung
//! server from blocking the loop forever.
//!
//! Where the payload goes is an `IngestTarget` behind a shared handle: `--endpoint` sets it up
//! front, and when `--server` is set without one, the sync loop fills it in from the server's
//! `ingest` hint — so a host enrolled with one agent token needs nothing else.
use std::sync::{Arc, Mutex};

use prost::Message;

use crate::config::AgentConfig;
use crate::gpu;
use crate::otlp::to_otlp;
use crate::sample::Sampler;
use crate::sysinfo_sampler::SysinfoSampler;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestTarget {
    pub endpoint: String,
    pub token: String,
}

pub type SharedTarget = Arc<Mutex<Option<IngestTarget>>>;

/// The initial target: `--endpoint` when given; otherwise the legacy default only when there
/// is no control channel to learn a better one from.
pub fn initial_target(cfg: &AgentConfig) -> SharedTarget {
    let t = match (&cfg.endpoint, cfg.control_channel()) {
        (Some(e), _) => Some(IngestTarget {
            endpoint: e.clone(),
            token: cfg.ingest_token.clone(),
        }),
        (None, false) => Some(IngestTarget {
            endpoint: crate::config::DEFAULT_ENDPOINT.to_string(),
            token: cfg.ingest_token.clone(),
        }),
        (None, true) => None,
    };
    Arc::new(Mutex::new(t))
}

pub async fn run(
    cfg: AgentConfig,
    target: SharedTarget,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let host = cfg.resolved_host();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut host_sampler = SysinfoSampler::new();
    let mut gpu_sampler = gpu::init(!cfg.no_gpu);
    let mut ticker =
        tokio::time::interval(std::time::Duration::from_secs(cfg.interval_secs.max(1)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Prime CPU deltas.
    let _ = host_sampler.sample();
    let start_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mut warned = false;

    loop {
        ticker.tick().await;
        let Some(t) = target.lock().unwrap().clone() else {
            if !warned {
                eprintln!(
                    "photon-agent: no metrics endpoint yet — waiting for the server's ingest hint"
                );
                warned = true;
            }
            continue;
        };
        let mut sample = host_sampler.sample();
        sample.metrics.extend(gpu_sampler.sample());
        // Real-clock timestamp for a real binary (unlike deterministic workflow scripts,
        // SystemTime::now() is the correct source of truth here).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let body = to_otlp(&host, &sample, start_nanos, now).encode_to_vec();
        let res = client
            .post(&t.endpoint)
            .bearer_auth(&t.token)
            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
            .body(body)
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => eprintln!("photon-agent: ingest returned {}", r.status()),
            Err(e) => eprintln!("photon-agent: send failed: {e}"),
        }
    }
}

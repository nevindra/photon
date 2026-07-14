//! The sender loop: sample the host (+ GPU) on a fixed interval and POST the OTLP/HTTP protobuf
//! payload to `cfg.endpoint`, mirroring `photon-loadgen/src/worker.rs`'s POST shape (bearer auth,
//! `application/x-protobuf`, prost-encoded body).
use prost::Message;

use crate::config::AgentConfig;
use crate::gpu;
use crate::otlp::to_otlp;
use crate::sample::Sampler;
use crate::sysinfo_sampler::SysinfoSampler;

pub async fn run(cfg: AgentConfig) -> Result<(), Box<dyn std::error::Error>> {
    let host = cfg.resolved_host();
    let client = reqwest::Client::new();
    let mut host_sampler = SysinfoSampler::new();
    let mut gpu_sampler = gpu::init(!cfg.no_gpu);
    let mut ticker =
        tokio::time::interval(std::time::Duration::from_secs(cfg.interval_secs.max(1)));
    // Prime CPU deltas.
    let _ = host_sampler.sample();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!("photon-agent: shutting down");
                return Ok(());
            }
            _ = ticker.tick() => {
                let mut sample = host_sampler.sample();
                sample.metrics.extend(gpu_sampler.sample());
                // Real-clock timestamp for a real binary (unlike deterministic workflow scripts,
                // SystemTime::now() is the correct source of truth here).
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64;
                let body = to_otlp(&host, &sample, now).encode_to_vec();
                let res = client
                    .post(&cfg.endpoint)
                    .bearer_auth(&cfg.token)
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
    }
}

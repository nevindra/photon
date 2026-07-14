use clap::Parser;

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4318/v1/metrics";

#[derive(Parser, Debug, Clone)]
#[command(
    name = "photon-agent",
    about = "Photon host/GPU resource-metrics agent"
)]
pub struct AgentConfig {
    /// OTLP/HTTP metrics endpoint.
    #[arg(long, env = "PHOTON_AGENT_ENDPOINT", default_value = DEFAULT_ENDPOINT)]
    pub endpoint: String,

    /// Ingest bearer token.
    #[arg(long, env = "PHOTON_INGEST_TOKEN", default_value = "dev-ingest-token")]
    pub token: String,

    /// Hostname reported as `host.name`. Defaults to the OS hostname.
    #[arg(long, env = "PHOTON_AGENT_HOST")]
    pub host_name: Option<String>,

    /// Seconds between samples.
    #[arg(long, env = "PHOTON_AGENT_INTERVAL", default_value_t = 15)]
    pub interval_secs: u64,

    /// Disable GPU sampling even when built with the `gpu` feature.
    #[arg(long, env = "PHOTON_AGENT_NO_GPU", default_value_t = false)]
    pub no_gpu: bool,
}

impl AgentConfig {
    pub fn resolved_host(&self) -> String {
        self.host_name.clone().unwrap_or_else(|| {
            sysinfo::System::host_name().unwrap_or_else(|| "unknown-host".to_string())
        })
    }
}

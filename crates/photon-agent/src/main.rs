mod config;
mod gpu;
mod otlp;
mod sample;
mod send;
mod sysinfo_sampler;

use config::AgentConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = <AgentConfig as clap::Parser>::parse();
    send::run(cfg).await
}

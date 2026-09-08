//! The binary: parse the CLI, dispatch a subcommand, or run the two loops — the resource
//! sampler and, when `--server` is set, the Docker control channel — until Ctrl-C.
use photon_agent::config::{Cli, Command};
use photon_agent::{install, send, sync};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = <Cli as clap::Parser>::parse();
    match cli.command {
        Some(Command::Version) => {
            println!("photon-agent {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(Command::Install(cfg)) => {
            install::install(&cfg).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            return Ok(());
        }
        Some(Command::Uninstall) => {
            install::uninstall().map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            return Ok(());
        }
        None => {}
    }
    let cfg = cli.cfg;

    if cfg.dry_inventory {
        return dry_inventory(&cfg).await;
    }

    let ingest = send::initial_target(&cfg);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    let sampler = if cfg.no_metrics {
        None
    } else {
        Some(tokio::spawn(send::run(cfg.clone(), ingest.clone())))
    };
    let control = if cfg.control_channel() {
        Some(tokio::spawn(sync::run(cfg.clone(), ingest.clone())))
    } else {
        eprintln!("photon-agent: no --server set; running the resource sampler only");
        None
    };
    if sampler.is_none() && control.is_none() {
        return Err("nothing to do: --no-metrics without --server".into());
    }

    tokio::select! {
        _ = &mut ctrl_c => {
            eprintln!("photon-agent: shutting down");
        }
        r = async { match sampler { Some(h) => h.await, None => std::future::pending().await } } => {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e as Box<dyn std::error::Error>),
                Err(e) => return Err(format!("sampler panicked: {e}").into()),
            }
        }
        r = async { match control { Some(h) => h.await, None => std::future::pending().await } } => {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e as Box<dyn std::error::Error>),
                Err(e) => return Err(format!("sync loop panicked: {e}").into()),
            }
        }
    }
    Ok(())
}

/// `--dry-inventory`: print the observed Docker inventory as JSON and exit.
async fn dry_inventory(
    cfg: &photon_agent::config::AgentConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    use photon_agent::docker::{Caps, DockerRuntime};
    let caps = Caps::from_config(cfg);
    let data_dir = photon_agent::state::resolve_data_dir(cfg.data_dir.as_deref());
    let rt = DockerRuntime::new(caps, &cfg.docker_host, &data_dir);
    let state =
        photon_agent::state::LocalState::load(&data_dir.join(photon_agent::state::STATE_FILE));
    let observed = rt
        .observe(&cfg.resolved_host(), &state)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    println!("{}", serde_json::to_string_pretty(&observed)?);
    Ok(())
}

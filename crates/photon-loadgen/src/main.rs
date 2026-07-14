//! photon-loadgen binary: parse the `logs`/`traces` subcommand, build the matching payload +
//! run config, and hand off to the shared driver. All logic lives in the library modules.

use clap::Parser;
use photon_loadgen::config::{Cli, Command};
use photon_loadgen::driver::{self, Report};
use photon_loadgen::logs::LogsPayload;
use photon_loadgen::metrics::MetricsPayload;
use photon_loadgen::payload::Payload;
use photon_loadgen::traces::TracePayload;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    match Cli::parse().command {
        Command::Logs(args) => {
            let cfg = exit_on_err(args.validate());
            let payload: Arc<dyn Payload> = Arc::new(LogsPayload {
                batch: cfg.batch,
                services: cfg.run.services,
            });
            let report = Report {
                unit: "logs",
                secondary: None,
                detail: format!("batch={} services={}", cfg.batch, cfg.run.services),
            };
            driver::run(cfg.run, payload, report).await;
        }
        Command::Traces(args) => {
            let cfg = exit_on_err(args.validate());
            let payload: Arc<dyn Payload> = Arc::new(TracePayload {
                traces_per_request: cfg.traces_per_request,
                services: cfg.run.services,
                spans_per_trace: cfg.spans_per_trace,
            });
            let report = Report {
                unit: "traces",
                secondary: Some("spans"),
                detail: format!(
                    "traces/req={} spans/trace={}..{} services={}",
                    cfg.traces_per_request,
                    cfg.spans_per_trace.min,
                    cfg.spans_per_trace.max,
                    cfg.run.services
                ),
            };
            driver::run(cfg.run, payload, report).await;
        }
        Command::Metrics(args) => {
            let cfg = exit_on_err(args.validate());
            let payload: Arc<dyn Payload> = Arc::new(MetricsPayload {
                metrics_per_request: cfg.metrics_per_request,
                services: cfg.run.services,
            });
            let report = Report {
                unit: "datapoints",
                secondary: None,
                detail: format!(
                    "metrics/req={} services={}",
                    cfg.metrics_per_request, cfg.run.services
                ),
            };
            driver::run(cfg.run, payload, report).await;
        }
    }
}

/// Print a validation error and exit(2), matching the previous CLI behavior.
fn exit_on_err<T>(r: Result<T, String>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}

//! CLI parsing (a `logs` / `traces` subcommand pair sharing one common arg group) and the
//! validated configs the driver + payloads run on. Validation (mutually-exclusive modes,
//! non-zero bounds, span-range sanity) happens in the `validate` methods so the pure config
//! types never hold an invalid combination.

use clap::{Args, Parser, Subcommand};
use std::time::Duration;

/// Default OTLP/HTTP endpoints, filled in per subcommand when `--endpoint` is omitted.
pub const DEFAULT_LOGS_ENDPOINT: &str = "http://127.0.0.1:4318/v1/logs";
pub const DEFAULT_TRACES_ENDPOINT: &str = "http://127.0.0.1:4318/v1/traces";
pub const DEFAULT_METRICS_ENDPOINT: &str = "http://127.0.0.1:4318/v1/metrics";

/// Raw command-line surface: a top-level subcommand choosing the signal.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "photon-loadgen",
    about = "High-throughput OTLP/HTTP load generator for Photon (logs + traces + metrics)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Generate OTLP log load against `POST /v1/logs`.
    Logs(LogsArgs),
    /// Generate OTLP trace load against `POST /v1/traces`.
    Traces(TraceArgs),
    /// Generate OTLP metric load against `POST /v1/metrics`.
    Metrics(MetricsArgs),
}

/// Options shared by every signal. Flattened into each subcommand's args.
#[derive(Args, Debug, Clone)]
pub struct CommonArgs {
    /// Target ingest URL (OTLP/HTTP protobuf endpoint). Defaults to the signal's `/v1/*` route.
    #[arg(long)]
    pub endpoint: Option<String>,

    /// Bearer token. Falls back to $PHOTON_INGEST_TOKEN, then the `make dev` default.
    #[arg(long, env = "PHOTON_INGEST_TOKEN", default_value = "dev-ingest-token")]
    pub token: String,

    /// Steady target throughput in units/second (logs/s or traces/s). Mutually exclusive with
    /// --saturate.
    #[arg(long)]
    pub rate: Option<u64>,

    /// Push as hard as concurrency allows. Mutually exclusive with --rate.
    #[arg(long)]
    pub saturate: bool,

    /// Concurrent in-flight senders (pooled connections).
    #[arg(long, default_value_t = 16)]
    pub concurrency: usize,

    /// Distinct service.name cardinality.
    #[arg(long, default_value_t = 10)]
    pub services: usize,

    /// Stop after N seconds; omit to run continuously.
    #[arg(long)]
    pub duration: Option<u64>,

    /// Optional cap on total units sent (logs or traces).
    #[arg(long)]
    pub total: Option<u64>,

    /// Live-stats print cadence in seconds.
    #[arg(long, default_value_t = 1)]
    pub report_interval: u64,
}

/// `logs` subcommand args.
#[derive(Args, Debug, Clone)]
pub struct LogsArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Log records per request.
    #[arg(long, default_value_t = 500)]
    pub batch: usize,
}

/// `traces` subcommand args.
#[derive(Args, Debug, Clone)]
pub struct TraceArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Traces per request. Each trace is a span tree, so keep this well below the logs `--batch`.
    #[arg(long, default_value_t = 20)]
    pub traces_per_request: usize,

    /// Spans per trace, as a single `N` or an inclusive `MIN..MAX` range.
    #[arg(long, value_parser = parse_span_range, default_value = "4..20")]
    pub spans_per_trace: SpanRange,
}

/// `metrics` subcommand args.
#[derive(Args, Debug, Clone)]
pub struct MetricsArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Metrics emitted per request (across all 5 types), default 50.
    #[arg(long, default_value_t = 50)]
    pub metrics_per_request: usize,
}

/// Inclusive span-count range for a generated trace. A bare `N` parses to `N..N`.
#[derive(Debug, Clone, Copy)]
pub struct SpanRange {
    pub min: usize,
    pub max: usize,
}

fn parse_span_range(s: &str) -> Result<SpanRange, String> {
    let s = s.trim();
    let (min, max) = match s.split_once("..") {
        Some((a, b)) => (
            a.trim()
                .parse()
                .map_err(|_| format!("invalid span count '{}'", a.trim()))?,
            b.trim()
                .parse()
                .map_err(|_| format!("invalid span count '{}'", b.trim()))?,
        ),
        None => {
            let n = s.parse().map_err(|_| format!("invalid span count '{s}'"))?;
            (n, n)
        }
    };
    Ok(SpanRange { min, max })
}

/// How the workers pace themselves.
#[derive(Debug, Clone)]
pub enum Pacing {
    /// Hold roughly this many units/second in aggregate.
    Rate(u64),
    /// No pacing — bounded only by concurrency and server ack latency.
    Saturate,
}

/// Validated run parameters shared by every signal.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub endpoint: String,
    pub token: String,
    pub pacing: Pacing,
    pub concurrency: usize,
    pub services: usize,
    pub duration: Option<Duration>,
    pub total: Option<u64>,
    pub report_interval: Duration,
}

/// Validated `logs` configuration.
#[derive(Debug, Clone)]
pub struct LogsConfig {
    pub run: RunConfig,
    pub batch: usize,
}

/// Validated `traces` configuration.
#[derive(Debug, Clone)]
pub struct TraceConfig {
    pub run: RunConfig,
    pub traces_per_request: usize,
    pub spans_per_trace: SpanRange,
}

/// Validated `metrics` configuration.
#[derive(Debug, Clone)]
pub struct MetricsRunConfig {
    pub run: RunConfig,
    pub metrics_per_request: usize,
}

impl CommonArgs {
    /// Validate the shared options into a [`RunConfig`], filling the signal's default endpoint.
    fn to_run_config(&self, default_endpoint: &str) -> Result<RunConfig, String> {
        let pacing = match (self.rate, self.saturate) {
            (Some(_), true) => return Err("--rate and --saturate are mutually exclusive".into()),
            (None, false) => {
                return Err("specify exactly one of --rate <units/sec> or --saturate".into())
            }
            (Some(0), _) => return Err("--rate must be > 0".into()),
            (Some(r), false) => Pacing::Rate(r),
            (None, true) => Pacing::Saturate,
        };
        if self.concurrency == 0 {
            return Err("--concurrency must be > 0".into());
        }
        if self.services == 0 {
            return Err("--services must be > 0".into());
        }
        Ok(RunConfig {
            endpoint: self
                .endpoint
                .clone()
                .unwrap_or_else(|| default_endpoint.to_string()),
            token: self.token.clone(),
            pacing,
            concurrency: self.concurrency,
            services: self.services,
            duration: self.duration.map(Duration::from_secs),
            total: self.total,
            report_interval: Duration::from_secs(self.report_interval.max(1)),
        })
    }
}

impl LogsArgs {
    pub fn validate(self) -> Result<LogsConfig, String> {
        let run = self.common.to_run_config(DEFAULT_LOGS_ENDPOINT)?;
        if self.batch == 0 {
            return Err("--batch must be > 0".into());
        }
        Ok(LogsConfig {
            run,
            batch: self.batch,
        })
    }
}

impl TraceArgs {
    pub fn validate(self) -> Result<TraceConfig, String> {
        let run = self.common.to_run_config(DEFAULT_TRACES_ENDPOINT)?;
        if self.traces_per_request == 0 {
            return Err("--traces-per-request must be > 0".into());
        }
        if self.spans_per_trace.min == 0 {
            return Err("--spans-per-trace minimum must be > 0".into());
        }
        if self.spans_per_trace.min > self.spans_per_trace.max {
            return Err("--spans-per-trace minimum must be <= maximum".into());
        }
        Ok(TraceConfig {
            run,
            traces_per_request: self.traces_per_request,
            spans_per_trace: self.spans_per_trace,
        })
    }
}

impl MetricsArgs {
    pub fn validate(self) -> Result<MetricsRunConfig, String> {
        let run = self.common.to_run_config(DEFAULT_METRICS_ENDPOINT)?;
        if self.metrics_per_request == 0 {
            return Err("--metrics-per-request must be > 0".into());
        }
        Ok(MetricsRunConfig {
            run,
            metrics_per_request: self.metrics_per_request,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logs_args(argv: &[&str]) -> LogsArgs {
        match Cli::parse_from(argv).command {
            Command::Logs(a) => a,
            other => panic!("expected logs subcommand, got {other:?}"),
        }
    }

    fn trace_args(argv: &[&str]) -> TraceArgs {
        match Cli::parse_from(argv).command {
            Command::Traces(a) => a,
            other => panic!("expected traces subcommand, got {other:?}"),
        }
    }

    fn metrics_args(argv: &[&str]) -> MetricsArgs {
        match Cli::parse_from(argv).command {
            Command::Metrics(a) => a,
            other => panic!("expected metrics subcommand, got {other:?}"),
        }
    }

    #[test]
    fn rejects_both_and_neither_modes() {
        let both = logs_args(&["lg", "logs", "--rate", "10", "--saturate"]);
        assert!(both.validate().is_err());

        let neither = logs_args(&["lg", "logs"]);
        assert!(neither.validate().is_err());
    }

    #[test]
    fn accepts_exactly_one_mode_per_signal() {
        let rate = logs_args(&["lg", "logs", "--rate", "1000"]);
        assert!(matches!(
            rate.validate().unwrap().run.pacing,
            Pacing::Rate(1000)
        ));

        let sat = trace_args(&["lg", "traces", "--saturate"]);
        assert!(matches!(
            sat.validate().unwrap().run.pacing,
            Pacing::Saturate
        ));
    }

    #[test]
    fn endpoints_default_per_signal() {
        let logs = logs_args(&["lg", "logs", "--saturate"]).validate().unwrap();
        assert_eq!(logs.run.endpoint, DEFAULT_LOGS_ENDPOINT);

        let traces = trace_args(&["lg", "traces", "--saturate"])
            .validate()
            .unwrap();
        assert_eq!(traces.run.endpoint, DEFAULT_TRACES_ENDPOINT);

        let custom = trace_args(&[
            "lg",
            "traces",
            "--saturate",
            "--endpoint",
            "http://x/v1/traces",
        ])
        .validate()
        .unwrap();
        assert_eq!(custom.run.endpoint, "http://x/v1/traces");

        let metrics = metrics_args(&["lg", "metrics", "--saturate"])
            .validate()
            .unwrap();
        assert_eq!(metrics.run.endpoint, DEFAULT_METRICS_ENDPOINT);
    }

    #[test]
    fn rejects_zero_bounds() {
        assert!(logs_args(&["lg", "logs", "--rate", "0"])
            .validate()
            .is_err());
        assert!(
            logs_args(&["lg", "logs", "--saturate", "--concurrency", "0"])
                .validate()
                .is_err()
        );
        assert!(logs_args(&["lg", "logs", "--saturate", "--batch", "0"])
            .validate()
            .is_err());
        assert!(
            trace_args(&["lg", "traces", "--saturate", "--traces-per-request", "0"])
                .validate()
                .is_err()
        );
        assert!(
            metrics_args(&["lg", "metrics", "--saturate", "--metrics-per-request", "0"])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn metrics_defaults_and_custom_per_request() {
        let default = metrics_args(&["lg", "metrics", "--saturate"])
            .validate()
            .unwrap();
        assert_eq!(default.metrics_per_request, 50);

        let custom = metrics_args(&[
            "lg",
            "metrics",
            "--saturate",
            "--metrics-per-request",
            "200",
        ])
        .validate()
        .unwrap();
        assert_eq!(custom.metrics_per_request, 200);
    }

    #[test]
    fn parses_and_validates_span_range() {
        let range = trace_args(&["lg", "traces", "--saturate", "--spans-per-trace", "3..9"])
            .validate()
            .unwrap()
            .spans_per_trace;
        assert_eq!((range.min, range.max), (3, 9));

        // A bare N means N..N.
        let fixed = trace_args(&["lg", "traces", "--saturate", "--spans-per-trace", "7"])
            .validate()
            .unwrap()
            .spans_per_trace;
        assert_eq!((fixed.min, fixed.max), (7, 7));

        // min > max is rejected.
        assert!(
            trace_args(&["lg", "traces", "--saturate", "--spans-per-trace", "9..3"])
                .validate()
                .is_err()
        );
        // zero min is rejected.
        assert!(
            trace_args(&["lg", "traces", "--saturate", "--spans-per-trace", "0..5"])
                .validate()
                .is_err()
        );
    }
}

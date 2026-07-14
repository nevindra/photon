//! The signal-agnostic load driver: given a validated [`RunConfig`], a [`Payload`], and display
//! labels, it spawns the worker pool + reporter, runs until Ctrl-C / `--duration` / `--total`,
//! then drains and prints a final summary. Both the `logs` and `traces` subcommands funnel
//! through [`run`]; only the payload and the labels differ.

use crate::config::{Pacing, RunConfig};
use crate::payload::Payload;
use crate::ratelimit::RateLimiter;
use crate::stats::{Snapshot, Stats};
use crate::worker::{run_worker, WorkerCtx};

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Display labels + banner detail for one signal. Keeps the driver generic while letting each
/// subcommand name its units ("logs" / "traces") and optional secondary column ("spans").
pub struct Report {
    /// Logical-unit label, e.g. `"logs"` or `"traces"`. Also used in the banner header.
    pub unit: &'static str,
    /// Secondary column label, e.g. `Some("spans")` for traces, `None` for logs.
    pub secondary: Option<&'static str>,
    /// Signal-specific knobs to show in the banner, e.g. `"batch=500 services=10"`.
    pub detail: String,
}

/// Run the load until a stop condition fires, then print the summary.
pub async fn run(run: RunConfig, payload: Arc<dyn Payload>, report: Report) {
    let run = Arc::new(run);

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(run.concurrency)
        .timeout(Duration::from_secs(30))
        .build()
        .expect("build http client");

    let cost = payload.cost();
    let limiter = Arc::new(match &run.pacing {
        Pacing::Rate(r) => RateLimiter::per_second(*r as f64, cost),
        Pacing::Saturate => RateLimiter::unlimited(),
    });
    let stats = Arc::new(Stats::new());
    let shutdown = Arc::new(AtomicBool::new(false));

    print_banner(&run, &report);

    let ctx = Arc::new(WorkerCtx {
        client,
        run: run.clone(),
        payload,
        limiter,
        stats: stats.clone(),
        shutdown: shutdown.clone(),
    });

    let started = Instant::now();
    let mut handles = Vec::with_capacity(run.concurrency);
    for id in 0..run.concurrency {
        handles.push(tokio::spawn(run_worker(id as u64, ctx.clone())));
    }
    let reporter = tokio::spawn(reporter_loop(
        stats.clone(),
        shutdown.clone(),
        run.report_interval,
        started,
        report.unit,
        report.secondary,
    ));

    wait_for_shutdown(&run, &stats).await;
    shutdown.store(true, Relaxed);

    for h in handles {
        let _ = h.await;
    }
    let _ = reporter.await;

    print_final(&stats, started.elapsed(), &report);
}

fn print_banner(cfg: &RunConfig, report: &Report) {
    let mode = match &cfg.pacing {
        Pacing::Rate(r) => format!("rate {r} {}/s", report.unit),
        Pacing::Saturate => "saturate".to_string(),
    };
    eprintln!("photon-loadgen [{}] → {}", report.unit, cfg.endpoint);
    eprintln!(
        "  mode={mode}  concurrency={}  {}  duration={}  total={}",
        cfg.concurrency,
        report.detail,
        cfg.duration
            .map(|d| format!("{}s", d.as_secs()))
            .unwrap_or_else(|| "∞".into()),
        cfg.total
            .map(|t| t.to_string())
            .unwrap_or_else(|| "∞".into()),
    );
    eprintln!("  (Ctrl-C to stop)\n");
}

/// Resolve when any stop condition fires: Ctrl-C, elapsed duration, or the total-units cap.
async fn wait_for_shutdown(cfg: &RunConfig, stats: &Stats) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let duration = async {
        match cfg.duration {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending::<()>().await,
        }
    };
    let total = async {
        match cfg.total {
            Some(t) => loop {
                if stats.units() >= t {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            },
            None => std::future::pending::<()>().await,
        }
    };

    tokio::select! {
        _ = ctrl_c => eprintln!("\n[shutdown] ctrl-c received, draining…"),
        _ = duration => eprintln!("\n[shutdown] duration elapsed, draining…"),
        _ = total => eprintln!("\n[shutdown] total target reached, draining…"),
    }
}

async fn reporter_loop(
    stats: Arc<Stats>,
    shutdown: Arc<AtomicBool>,
    interval: Duration,
    started: Instant,
    unit: &'static str,
    secondary: Option<&'static str>,
) {
    let mut prev = stats.snapshot();
    let mut prev_at = started;
    loop {
        tokio::time::sleep(interval).await;
        if shutdown.load(Relaxed) {
            break;
        }
        let now = Instant::now();
        let snap = stats.snapshot();
        let dt = now.duration_since(prev_at).as_secs_f64().max(1e-6);

        let dunits = snap.units.saturating_sub(prev.units) as f64 / dt;
        let dreq = snap.requests.saturating_sub(prev.requests) as f64 / dt;
        let dmb = (snap.bytes.saturating_sub(prev.bytes) as f64 / dt) / (1024.0 * 1024.0);

        let secondary_col = match secondary {
            Some(label) => {
                let dsecondary = snap.spans.saturating_sub(prev.spans) as f64 / dt;
                format!("{dsecondary:>10.0} {label}/s | ")
            }
            None => String::new(),
        };

        println!(
            "[{:>4}s] {:>10.0} {unit}/s | {secondary_col}{:>6.0} req/s | {:>6.1} MB/s | \
             ok {} httperr {} neterr {} | p50 {:>5.1}ms p95 {:>5.1}ms p99 {:>5.1}ms | total {} {unit}",
            now.duration_since(started).as_secs(),
            dunits,
            dreq,
            dmb,
            snap.ok,
            snap.http_errors,
            snap.transport_errors,
            snap.p50_us as f64 / 1000.0,
            snap.p95_us as f64 / 1000.0,
            snap.p99_us as f64 / 1000.0,
            snap.units,
        );

        prev = snap;
        prev_at = now;
    }
}

fn print_final(stats: &Stats, elapsed: Duration, report: &Report) {
    let snap: Snapshot = stats.snapshot();
    let secs = elapsed.as_secs_f64().max(1e-6);
    let mb = snap.bytes as f64 / (1024.0 * 1024.0);

    eprintln!("\n──── summary ────");
    eprintln!("  duration        {secs:.1}s");
    eprintln!(
        "  {} accepted   {} ({:.0}/s avg)",
        report.unit,
        snap.units,
        snap.units as f64 / secs
    );
    if let Some(label) = report.secondary {
        eprintln!(
            "  {label} accepted   {} ({:.0}/s avg)",
            snap.spans,
            snap.spans as f64 / secs
        );
    }
    eprintln!(
        "  requests        {} ({:.0}/s avg)",
        snap.requests,
        snap.requests as f64 / secs
    );
    eprintln!("  data sent       {mb:.1} MB ({:.1} MB/s avg)", mb / secs);
    eprintln!(
        "  ok / http-err / net-err   {} / {} / {}",
        snap.ok, snap.http_errors, snap.transport_errors
    );
    eprintln!(
        "  ack latency     p50 {:.1}ms  p95 {:.1}ms  p99 {:.1}ms",
        snap.p50_us as f64 / 1000.0,
        snap.p95_us as f64 / 1000.0,
        snap.p99_us as f64 / 1000.0
    );
    if !snap.status.is_empty() {
        let codes: Vec<String> = snap
            .status
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect();
        eprintln!("  status codes    {}", codes.join("  "));
    }
}

//! Read-only probe: run one `red_rollup` pass over a hot tier and print what it produced.
//!
//! The rollup pass writes into the metrics WAL, so there is no way to ask "what would this bucket
//! publish?" from the API — by the time you can read it, it is already written. This runs the same
//! query the pass runs and prints the rows instead, which is what you want when a pass reports
//! "done" and nothing appears under the metric name.
//!
//! ```sh
//! cargo run --release -p photon-query --example rollup_probe -- <hot_dir> <start_epoch_secs> [hours]
//! ```

use std::collections::{HashMap, HashSet};

use photon_core::metric_schema::MetricSchema;
use photon_core::span_schema::SpanSchema;
use photon_query::{SpanQueryEngine, SpanQueryRequest, SpanSort};

const NS_PER_SEC: i64 = 1_000_000_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let hot: std::path::PathBuf = args.next().expect("hot dir").into();
    let start: i64 = args.next().expect("start epoch secs").parse::<i64>()? * NS_PER_SEC;
    let hours: i64 = args.next().unwrap_or_else(|| "1".into()).parse()?;
    let end = start + hours * 3600 * NS_PER_SEC;

    // Must match the config the server runs with, or the same Parquet reads differently.
    let promoted: Vec<String> = [
        "service.name",
        "host.name",
        "deployment.environment",
        "tenant",
        "http.response.status_code",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let _ = MetricSchema::new(&promoted);
    let engine = SpanQueryEngine::new(hot, SpanSchema::new(&promoted))?;

    let req = SpanQueryRequest {
        start_ts_nanos: start,
        end_ts_nanos: end - 1,
        query: None,
        sort: SpanSort::Recent,
        limit: 0,
        offset: 0,
        projected_attributes: Vec::new(),
        required_attributes: Vec::new(),
    };
    let t0 = std::time::Instant::now();
    let pass = engine
        .red_rollup(
            req,
            start,
            end,
            60 * NS_PER_SEC,
            &HashMap::new(),
            500,
            50,
            &HashSet::new(),
        )
        .await?;
    println!(
        "{} service rows, {} operation rows in {:.2}s (folded {}, discovery_truncated {})",
        pass.services.len(),
        pass.operations.len(),
        t0.elapsed().as_secs_f64(),
        pass.folded_operations,
        pass.discovery_truncated
    );
    for row in pass.services.iter().take(5) {
        println!(
            "  ts={} service={} env={:?} tenant={:?} count={} duration_count={} buckets={:?}",
            row.ts,
            row.service,
            row.environment,
            row.tenant,
            row.count,
            row.duration_count,
            row.latency_buckets,
        );
    }
    Ok(())
}

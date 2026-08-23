//! Read-only probe: run the real `MetricsQueryEngine` against a hot tier for one metric name.
//!
//! Sits between `photon-index`'s `idx_probe` (what the sidecar claims) and `metric_names` (what a
//! single file holds): this one goes through pruning, the explicit read schema and the type probe,
//! which is where a metric that demonstrably has rows can still come back as an empty series.
//!
//! ```sh
//! cargo run --release -p photon-query --example metric_probe -- <hot_dir> <metric> [agg]
//! ```

use photon_core::metric_agg::Agg;
use photon_core::metric_schema::MetricSchema;
use photon_query::{MetricSeriesRequest, MetricsQueryEngine};

const NS_PER_SEC: i64 = 1_000_000_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let hot: std::path::PathBuf = args.next().expect("hot dir").into();
    let metric = args.next().expect("metric name");
    let agg = args.next().map(|s| Agg::parse(&s).expect("unknown agg"));

    // Must match the server's `[schema].promoted_attributes`.
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
    let engine = MetricsQueryEngine::new(hot, MetricSchema::new(&promoted))?;

    let res = engine
        .query_series(MetricSeriesRequest {
            metric: metric.clone(),
            agg,
            group_by: Vec::new(),
            filter: None,
            start_ts_nanos: 1_786_686_660 * NS_PER_SEC,
            end_ts_nanos: 1_787_500_000 * NS_PER_SEC,
            buckets: 8,
        })
        .await?;
    println!(
        "{metric}: default_agg={:?} chosen={:?} series={} capped={}",
        res.default_agg,
        res.chosen_agg,
        res.series.len(),
        res.capped
    );
    for s in res.series.iter().take(3) {
        let pts: Vec<_> = s.points.iter().filter_map(|p| p.v).take(4).collect();
        println!("  {:?} -> {pts:?}", s.labels);
    }
    Ok(())
}

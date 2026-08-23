//! Read-only probe: which metrics files survive pruning for one metric name, and whether a `LIMIT`
//! plan over them agrees with a `count()`.
//!
//! Written to diagnose a metric that the catalog lists and every chart reports as empty. It mirrors
//! `MetricsQueryEngine::keep_candidate` (manifest time overlap, then the sidecar's token bloom) so
//! the survivor set can be inspected from outside the engine, then reads exactly those files under
//! the SAME explicit schema the engine imposes.
//!
//! The last line is the interesting one. `count()` and `LIMIT 1` over one filtered frame must
//! agree; they do not when Parquet filter pushdown is on and the survivors were written under
//! different `[schema].promoted_attributes`. That is why `session()` keeps pushdown off — see the
//! flag's comment in `lib.rs`. Pass `pushdown` as the third argument to reproduce the failure.
//!
//! ```sh
//! cargo run --release -p photon-query --example metric_survivors -- <hot_dir> <metric> [pushdown]
//! ```

use datafusion::prelude::{col, lit, ParquetReadOptions, SessionConfig, SessionContext};
use datafusion::scalar::ScalarValue;
use photon_core::metric_schema::MetricSchema;
use photon_index::SkipIndex;
use serde_json::Value;

const NS_PER_SEC: i64 = 1_000_000_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let hot: std::path::PathBuf = args.next().expect("hot dir").into();
    let metric = args.next().expect("metric name");
    let pushdown = args.next().is_some_and(|v| v == "pushdown");
    // Wide enough to cover any retention this is pointed at: the subject is the file set, not a
    // particular window.
    let start = 0i64;
    let end = 4_000_000_000i64 * NS_PER_SEC;

    let manifest: Value =
        serde_json::from_slice(&std::fs::read(hot.join("manifest/metrics-manifest.json"))?)?;
    let entries = manifest["entries"].as_array().cloned().unwrap_or_default();

    let mut kept = Vec::new();
    let (mut overlapped, mut no_idx) = (0usize, 0usize);
    for e in &entries {
        let (lo, hi) = (
            e["min_ts_nanos"].as_i64().unwrap_or(i64::MIN),
            e["max_ts_nanos"].as_i64().unwrap_or(i64::MAX),
        );
        if hi < start || lo > end {
            continue;
        }
        overlapped += 1;
        let path = e["path"].as_str().unwrap_or_default();
        let idx = hot.join(path.replace(".parquet", ".idx"));
        match std::fs::read(&idx)
            .ok()
            .and_then(|b| SkipIndex::from_bytes(&b).ok())
        {
            // Same rule as the engine: no readable sidecar means keep the file.
            None => {
                no_idx += 1;
                kept.push(hot.join(path).to_string_lossy().into_owned());
            }
            Some(index) if index.might_contain_token(&metric) => {
                kept.push(hot.join(path).to_string_lossy().into_owned())
            }
            Some(_) => {}
        }
    }
    println!(
        "{} entries, {overlapped} overlap the window, {} survive the bloom ({no_idx} kept for a \
         missing sidecar)",
        entries.len(),
        kept.len()
    );
    if kept.is_empty() {
        return Ok(());
    }

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
    let schema = MetricSchema::new(&promoted);

    let config = SessionConfig::new()
        .set_bool(
            "datafusion.execution.parquet.schema_force_view_types",
            false,
        )
        .set_bool("datafusion.execution.parquet.pushdown_filters", pushdown)
        .set_bool("datafusion.execution.parquet.reorder_filters", pushdown);
    let ctx = SessionContext::new_with_config(config);
    let ts = |v: i64| lit(ScalarValue::TimestampNanosecond(Some(v), None));
    let filtered = ctx
        .read_parquet(
            kept.clone(),
            ParquetReadOptions::default().schema(schema.arrow.as_ref()),
        )
        .await?
        .filter(
            col("metric_name")
                .eq(lit(metric.clone()))
                .and(col("timestamp").gt_eq(ts(start)))
                .and(col("timestamp").lt_eq(ts(end))),
        )?;

    let counted = filtered.clone().count().await?;
    let batches = filtered
        .select(vec![col("metric_type"), col("temporality")])?
        .limit(0, Some(1))?
        .collect()
        .await?;
    let limited: usize = batches.iter().map(|b| b.num_rows()).sum();
    println!("pushdown={pushdown}: count() -> {counted} rows, LIMIT 1 -> {limited} rows");
    if counted > 0 && limited == 0 {
        println!(
            "  ^ the fetch plan lost every row the aggregate found — this is the pushdown bug"
        );
    }
    Ok(())
}

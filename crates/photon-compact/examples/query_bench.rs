//! Dev-only: run the same real queries against two hot tiers and time them.
//!   cargo run --release -p photon-compact --example query_bench -- <tier_a> <tier_b>
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use photon_core::metric_schema::MetricSchema;
use photon_core::schema::LogSchema;
use photon_core::span_schema::SpanSchema;
use photon_query::RedGroup;
use photon_query::{MetricsQueryEngine, QueryEngine, QueryRequest};
use photon_query::{SpanQueryEngine, SpanQueryRequest, SpanSort};

const DAY: i64 = 86_400_000_000_000;

async fn bench(tier: &PathBuf, end: i64) -> Vec<(String, f64, String)> {
    let p: Vec<String> = [
        "service.name",
        "host.name",
        "deployment.environment",
        "tenant",
        "http.response.status_code",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let logs = QueryEngine::new(tier.clone(), LogSchema::new(&p)).unwrap();
    let spans = SpanQueryEngine::new(tier.clone(), SpanSchema::new(&p)).unwrap();
    let _metrics = MetricsQueryEngine::new(tier.clone(), MetricSchema::new(&p)).unwrap();
    let mut out = Vec::new();

    let go = |name: &str, t: Instant, n: String, out: &mut Vec<_>| {
        out.push((name.to_string(), t.elapsed().as_secs_f64() * 1000.0, n));
    };

    for (label, days) in [("24h", 1i64), ("7d", 7)] {
        let req = QueryRequest {
            start_ts_nanos: end - days * DAY,
            end_ts_nanos: end,
            services: vec![],
            severities: vec![],
            text: None,
            query: None,
            limit: 100,
        };
        let t = Instant::now();
        let r = logs.search(req).await.unwrap();
        let n: usize = r.iter().map(|b| b.num_rows()).sum();
        go(
            &format!("logs search {label}, limit 100"),
            t,
            format!("{n} rows"),
            &mut out,
        );
    }

    let req = QueryRequest {
        start_ts_nanos: end - 7 * DAY,
        end_ts_nanos: end,
        services: vec![],
        severities: vec![],
        text: Some("error".to_string()),
        query: None,
        limit: 100,
    };
    let t = Instant::now();
    let r = logs.search(req).await.unwrap();
    let n: usize = r.iter().map(|b| b.num_rows()).sum();
    go(
        "logs search 7d, text=\"error\"",
        t,
        format!("{n} rows"),
        &mut out,
    );

    for (label, days) in [("24h", 1i64), ("7d", 7)] {
        let req = SpanQueryRequest {
            start_ts_nanos: end - days * DAY,
            end_ts_nanos: end,
            query: None,
            sort: SpanSort::Recent,
            limit: 100,
            offset: 0,
            projected_attributes: vec![],
            required_attributes: vec![],
        };
        let t = Instant::now();
        let r = spans.search_spans(req).await.unwrap();
        let n: usize = r.iter().map(|b| b.num_rows()).sum();
        go(
            &format!("span search {label}, Recent, limit 100"),
            t,
            format!("{n} rows"),
            &mut out,
        );
    }

    for (label, days) in [("24h", 1i64), ("7d", 7)] {
        let req = SpanQueryRequest {
            start_ts_nanos: end - days * DAY,
            end_ts_nanos: end,
            query: None,
            sort: SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: vec![],
            required_attributes: vec![],
        };
        let t = Instant::now();
        let rows = spans
            .red_metrics(req, RedGroup::Service, &HashMap::new(), 500)
            .await
            .unwrap();
        go(
            &format!("RED by service {label} (/api/red)"),
            t,
            format!("{} services", rows.len()),
            &mut out,
        );
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let tiers: Vec<PathBuf> = a[1..3].iter().map(PathBuf::from).collect();
    photon_query::set_memory_pool_bytes(2 * 1024 * 1024 * 1024);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    // Newest timestamp in the SOURCE spans manifest, so both tiers use the same window.
    let m = photon_core::manifest::Manifest::from_json(
        &std::fs::read_to_string(tiers[0].join("manifest/spans-manifest.json")).unwrap(),
    )
    .unwrap();
    let end = m.entries().iter().map(|e| e.max_ts_nanos).max().unwrap();

    let mut results: Vec<Vec<(String, f64, String)>> = Vec::new();
    for t in &tiers {
        eprintln!("running {} ...", t.display());
        // Two passes; keep the faster, so page-cache warmth does not decide the winner.
        let a = rt.block_on(bench(t, end));
        let b = rt.block_on(bench(t, end));
        results.push(
            a.into_iter()
                .zip(b)
                .map(|(x, y)| (x.0, x.1.min(y.1), x.2))
                .collect(),
        );
    }

    println!(
        "\n{:<42} {:>12} {:>12} {:>10}   {}",
        "query", "before", "after", "change", "result"
    );
    for i in 0..results[0].len() {
        let (name, ta, na) = &results[0][i];
        let (_, tb, nb) = &results[1][i];
        println!(
            "{:<42} {:>9.0} ms {:>9.0} ms {:>9.1}x   {} / {}",
            name,
            ta,
            tb,
            ta / tb,
            na,
            nb
        );
    }
}

//! End-to-end metrics query correctness over real compacted Parquet. The fixture mirrors
//! crates/photon-compact/tests/metrics_pipeline.rs (WAL → MetricsCompactor → hot dir), then runs
//! the Phase-2 query methods and checks the numbers.

// window helpers: everything in [T0, T0+STEP*2] so 2 buckets of width STEP.
const T0: i64 = 1_000_000_000_000; // 1000s in ns
const STEP: i64 = 30_000_000_000; // 30s in ns
const END: i64 = T0 + STEP * 2;

use std::sync::Arc;

use object_store::local::LocalFileSystem;
use photon_compact::MetricsCompactor;
use photon_core::config::WalConfig;
use photon_core::metric_agg::Agg;
use photon_core::metric_record::MetricPoint;
use photon_core::metric_schema::{metric_type, MetricSchema};
use photon_query::{LabelsResult, MetricSeriesRequest, MetricsQueryEngine};
use photon_storage::{Replicator, Storage};
use photon_wal::DiskWal;

// MetricPoint derives Default (Phase 1) — set only the fields that matter, `..Default::default()`
// the rest. This is the same shape crates/photon-compact/tests/metrics_pipeline.rs uses.
fn svc_attrs(service: &str) -> std::collections::BTreeMap<String, String> {
    let mut a = std::collections::BTreeMap::new();
    a.insert("service.name".to_string(), service.to_string());
    a
}

fn cum_counter(service: &str, ts: i64, value: f64, start_ts: i64) -> MetricPoint {
    MetricPoint {
        metric_name: "http.requests".to_string(),
        metric_type: metric_type::SUM,
        temporality: Some(2), // cumulative
        is_monotonic: Some(true),
        timestamp_nanos: ts,
        start_timestamp_nanos: Some(start_ts),
        value: Some(value),
        attributes: svc_attrs(service),
        ..Default::default()
    }
}

fn gauge(service: &str, ts: i64, value: f64) -> MetricPoint {
    MetricPoint {
        metric_name: "cpu.util".to_string(),
        metric_type: metric_type::GAUGE,
        timestamp_nanos: ts,
        value: Some(value),
        attributes: svc_attrs(service),
        ..Default::default()
    }
}

/// Ingest → compact → engine, copied from metrics_pipeline.rs. WalConfig with
/// `segment_max_bytes: 1` + a second append forces the first segment closed so the compactor
/// drains it. Returns an engine rooted at the compacted hot dir. The `tempfile::TempDir` is
/// leaked into the returned tuple so the dir survives for the test's lifetime.
async fn engine_with(points: Vec<MetricPoint>) -> (MetricsQueryEngine, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let hot_dir = tmp.path().to_path_buf();
    let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);
    let wal_cfg = WalConfig {
        segment_max_bytes: 1,
        segment_max_age_secs: 0,
        group_commit_max_delay_ms: 0,
    };
    let wal = Arc::new(
        DiskWal::open_arrow(hot_dir.join("wal-metrics"), schema.arrow.clone(), wal_cfg)
            .await
            .unwrap(),
    );
    // One batch per point so each tiny (size=1) segment closes and is drained.
    for p in &points {
        let mut b = photon_core::metric_record::MetricBatchBuilder::new(&schema);
        b.append(p);
        wal.append(b.finish().unwrap()).await.unwrap();
        wal.sync().await.unwrap();
    }
    // A trailing append to close the last real data segment. Throwaway metric far before the test
    // window (ts=1) so it can never appear in any query, catalog, or labels result.
    let seal = MetricPoint {
        metric_name: "__seal__".to_string(),
        metric_type: metric_type::GAUGE,
        timestamp_nanos: 1,
        value: Some(0.0),
        attributes: svc_attrs("__seal__"),
        ..Default::default()
    };
    let mut tail = photon_core::metric_record::MetricBatchBuilder::new(&schema);
    tail.append(&seal);
    wal.append(tail.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();

    let storage = Storage {
        hot: Arc::new(LocalFileSystem::new_with_prefix(&hot_dir).unwrap()),
        durable: None,
        hot_dir: Some(hot_dir.clone()),
    };
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = MetricsCompactor::new(wal.clone(), storage, replicator, schema.clone());
    while compactor.run_once().await.unwrap().is_some() {}

    (MetricsQueryEngine::new(hot_dir, schema).unwrap(), tmp)
}

#[tokio::test]
async fn cumulative_rate_is_reset_aware() {
    // One cumulative counter for service=a. Samples (all same start_ts, so reset = value drop):
    //  bucket0 [T0, T0+STEP): ts=T0 → 0, ts=T0+10s → 100   (increase 100)
    //  bucket1 [T0+STEP, END): ts=T0+STEP → 100, then RESET to 5 at +5s, then 25 at +10s
    //     contributions in bucket1: (100-100=0) + (reset → 5) + (25-5=20) = 25
    let ten = 10_000_000_000;
    let points = vec![
        cum_counter("a", T0, 0.0, T0),
        cum_counter("a", T0 + ten, 100.0, T0),
        cum_counter("a", T0 + STEP, 100.0, T0),
        cum_counter("a", T0 + STEP + 5_000_000_000, 5.0, T0),
        cum_counter("a", T0 + STEP + ten, 25.0, T0),
    ];
    let (engine, _tmp) = engine_with(points).await;
    let r = engine
        .query_series(MetricSeriesRequest {
            metric: "http.requests".to_string(),
            agg: Some(Agg::Increase),
            group_by: vec!["service".to_string()],
            filter: None,
            start_ts_nanos: T0,
            end_ts_nanos: END,
            buckets: 2,
        })
        .await
        .unwrap();
    assert_eq!(r.series.len(), 1);
    let pts = &r.series[0].points;
    assert_eq!(pts[0].v, Some(100.0), "bucket0 increase");
    assert_eq!(pts[1].v, Some(25.0), "bucket1 increase (reset-aware)");
    // default agg for a monotonic Sum is rate:
    assert_eq!(r.default_agg, Agg::Rate);
}

#[tokio::test]
async fn gauge_avg_groups_by_service() {
    let ten = 10_000_000_000;
    let points = vec![
        gauge("a", T0 + ten, 10.0),
        gauge("a", T0 + STEP + ten, 30.0),
        gauge("b", T0 + ten, 5.0),
        gauge("b", T0 + STEP + ten, 7.0),
    ];
    let (engine, _tmp) = engine_with(points).await;
    let r = engine
        .query_series(MetricSeriesRequest {
            metric: "cpu.util".to_string(),
            agg: None, // default avg
            group_by: vec!["service".to_string()],
            filter: None,
            start_ts_nanos: T0,
            end_ts_nanos: END,
            buckets: 2,
        })
        .await
        .unwrap();
    assert_eq!(r.default_agg, Agg::Avg);
    assert_eq!(r.series.len(), 2);
    let a = r
        .series
        .iter()
        .find(|s| s.labels.get("service").map(String::as_str) == Some("a"))
        .unwrap();
    assert_eq!(a.points[0].v, Some(10.0));
    assert_eq!(a.points[1].v, Some(30.0));
    let b = r
        .series
        .iter()
        .find(|s| s.labels.get("service").map(String::as_str) == Some("b"))
        .unwrap();
    assert_eq!(b.points[0].v, Some(5.0));
    assert_eq!(b.points[1].v, Some(7.0));
}

#[tokio::test]
async fn catalog_and_metadata_and_labels() {
    let points = vec![
        gauge("a", T0 + 1, 1.0),
        gauge("b", T0 + 2, 2.0),
        cum_counter("a", T0 + 3, 5.0, T0),
    ];
    let (engine, _tmp) = engine_with(points).await;

    let cat = engine.catalog(T0, END, None, None).await.unwrap();
    let names: Vec<&str> = cat.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"cpu.util") && names.contains(&"http.requests"));
    let cpu = cat.iter().find(|c| c.name == "cpu.util").unwrap();
    assert_eq!(
        cpu.metric_type,
        photon_core::metric_schema::metric_type::GAUGE
    );

    assert!(engine
        .metadata("does.not.exist", T0, END)
        .await
        .unwrap()
        .is_none());
    let md = engine.metadata("cpu.util", T0, END).await.unwrap().unwrap();
    assert_eq!(
        md.metric_type,
        photon_core::metric_schema::metric_type::GAUGE
    );
    assert!(md.attribute_keys.iter().any(|k| k == "service"));

    match engine
        .labels("cpu.util", Some("service.name"), T0, END)
        .await
        .unwrap()
    {
        LabelsResult::Values { values, capped } => {
            assert!(values.contains(&"a".to_string()) && values.contains(&"b".to_string()));
            assert!(!capped);
        }
        _ => panic!("expected values"),
    }
}

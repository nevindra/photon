//! Phase-1 end-to-end: metric points → WAL → MetricsCompactor → SQL-queryable Parquet.

use std::sync::Arc;

use object_store::local::LocalFileSystem;
use photon_compact::MetricsCompactor;
use photon_core::config::WalConfig;
use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
use photon_core::metric_schema::{metric_type, MetricSchema};
use photon_query::MetricsQueryEngine;
use photon_storage::{Replicator, Storage};
use photon_wal::DiskWal;

fn point(name: &str, svc: &str, ts: i64, value: f64) -> MetricPoint {
    let mut attributes = std::collections::BTreeMap::new();
    attributes.insert("service.name".to_string(), svc.to_string());
    MetricPoint {
        metric_name: name.to_string(),
        metric_type: metric_type::GAUGE,
        timestamp_nanos: ts,
        value: Some(value),
        attributes,
        ..Default::default()
    }
}

#[tokio::test]
async fn metrics_are_stored_and_sql_queryable() {
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

    let mut b = MetricBatchBuilder::new(&schema);
    b.append(&point("cpu.usage", "checkout", 100, 0.73));
    b.append(&point("cpu.usage", "cart", 200, 0.41));
    wal.append(b.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();
    // second tiny append forces the first (size=1) segment closed
    let mut b2 = MetricBatchBuilder::new(&schema);
    b2.append(&point("http.rps", "checkout", 300, 12.0));
    wal.append(b2.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();

    let storage = Storage {
        hot: Arc::new(LocalFileSystem::new_with_prefix(&hot_dir).unwrap()),
        durable: None,
        hot_dir: Some(hot_dir.clone()),
    };
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = MetricsCompactor::new(wal.clone(), storage, replicator, schema.clone());
    while compactor.run_once().await.unwrap().is_some() {}

    let engine = MetricsQueryEngine::new(hot_dir.clone(), schema).unwrap();
    let batches = engine
        .sql("SELECT metric_name, \"service.name\" AS svc, value FROM metrics WHERE metric_name = 'cpu.usage' ORDER BY \"service.name\"")
        .await
        .unwrap();

    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, 2, "both cpu.usage points are stored and queryable");
}

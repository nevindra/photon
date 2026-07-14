//! Phase-1 end-to-end: spans → WAL → SpanCompactor → SQL-queryable Parquet.

use std::collections::BTreeMap;
use std::sync::Arc;

use object_store::local::LocalFileSystem;
use photon_compact::SpanCompactor;
use photon_core::config::WalConfig;
use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
use photon_core::span_schema::SpanSchema;
use photon_query::SpanQueryEngine;
use photon_storage::{Replicator, Storage};
use photon_wal::DiskWal;

fn span(trace: &str, span_id: &str, svc: &str, name: &str, start: i64, dur: i64) -> SpanRecord {
    let mut attributes = BTreeMap::new();
    attributes.insert("service.name".to_string(), svc.to_string());
    SpanRecord {
        trace_id: trace.into(),
        span_id: span_id.into(),
        name: Some(name.into()),
        start_time_nanos: start,
        end_time_nanos: Some(start + dur),
        duration_nanos: Some(dur),
        status_code: Some(2),
        status_text: Some("ERROR".into()),
        attributes,
        ..Default::default()
    }
}

#[tokio::test]
async fn spans_are_stored_and_sql_queryable() {
    let tmp = tempfile::tempdir().unwrap();
    let hot_dir = tmp.path().to_path_buf();
    let schema = SpanSchema::new(&["service.name".to_string()]);

    // 1. Append two spans of one trace to a real spans WAL, then sync + let it rotate by
    //    forcing a closed segment via a short size bound.
    let wal_cfg = WalConfig {
        segment_max_bytes: 1, // rotate aggressively so the segment closes for the compactor
        segment_max_age_secs: 0,
        group_commit_max_delay_ms: 0,
    };
    let wal = Arc::new(
        DiskWal::open_arrow(hot_dir.join("wal-traces"), schema.arrow.clone(), wal_cfg)
            .await
            .unwrap(),
    );

    let mut b = SpanBatchBuilder::new(&schema);
    b.append(&span(
        "t1",
        "root",
        "checkout",
        "POST /checkout",
        100,
        1_840,
    ));
    b.append(&span("t1", "child", "payments", "charge.card", 200, 1_500));
    wal.append(b.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();

    // A second tiny append forces the (size=1) first segment to have rotated/closed.
    let mut b2 = SpanBatchBuilder::new(&schema);
    b2.append(&span("t2", "root", "cart", "GET /cart", 300, 88));
    wal.append(b2.finish().unwrap()).await.unwrap();
    wal.sync().await.unwrap();

    // 2. Compact every closed segment.
    let storage = Storage {
        hot: Arc::new(LocalFileSystem::new_with_prefix(&hot_dir).unwrap()),
        durable: None,
        hot_dir: Some(hot_dir.clone()),
    };
    let replicator = Arc::new(Replicator::new(storage.clone()));
    let compactor = SpanCompactor::new(wal.clone(), storage, replicator, schema.clone());
    while compactor.run_once().await.unwrap().is_some() {}

    // 3. Query the spans via SQL.
    let engine = SpanQueryEngine::new(hot_dir.clone(), schema).unwrap();
    let batches = engine
        .sql("SELECT trace_id, \"service.name\" AS svc, name FROM spans WHERE trace_id = 't1' ORDER BY start_time_nanos")
        .await
        .unwrap();

    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, 2, "both spans of trace t1 are stored and queryable");
}

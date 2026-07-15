//! Layer-1 WAL bench: sequential `append` (each = one group-commit fsync round, since appends
//! are awaited one at a time). Isolates F6/F7 (per-round writer overhead). Run on tmpfs to see
//! the CPU/framing ceiling; on real disk to see fsync cost. Backing dir = $PHOTON_BENCH_DIR
//! (default: the OS tempdir via `tempfile`).

use arrow::record_batch::RecordBatch;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use photon_core::config::WalConfig;
use photon_core::record::{LogRecord, RecordBatchBuilder};
use photon_core::schema::LogSchema;
use photon_wal::DiskWal;
use std::collections::BTreeMap;

fn schema() -> LogSchema {
    LogSchema::new(&["service.name".to_string(), "host.name".to_string()])
}

fn wal_config() -> WalConfig {
    WalConfig {
        segment_max_bytes: 134_217_728,
        segment_max_age_secs: 3600, // don't rotate by age mid-bench
        group_commit_max_delay_ms: 5,
    }
}

/// Build one RecordBatch of `rows` synthetic log rows, matching `schema`.
fn make_batch(schema: &LogSchema, rows: usize) -> RecordBatch {
    let mut builder = RecordBatchBuilder::with_capacity(schema, rows);
    for r in 0..rows {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), "checkout".to_string());
        attributes.insert("host.name".to_string(), "node-42".to_string());
        attributes.insert("http.method".to_string(), "GET".to_string());
        builder.append(&LogRecord {
            timestamp_nanos: 1_700_000_000_000_000_000 + r as i64,
            body: Some(format!("request {r} completed")),
            attributes,
            ..Default::default()
        });
    }
    builder.finish().unwrap()
}

fn bench_wal_append(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let schema = schema();

    let mut g = c.benchmark_group("wal_append");
    for &rows in &[500usize, 5_000] {
        // Fresh WAL per shape. PHOTON_BENCH_DIR lets Task 6 point this at tmpfs vs real disk.
        let base = std::env::var("PHOTON_BENCH_DIR").ok();
        let dir = match &base {
            Some(b) => {
                let p = std::path::Path::new(b).join(format!("wal-bench-{rows}"));
                let _ = std::fs::remove_dir_all(&p);
                std::fs::create_dir_all(&p).unwrap();
                p
            }
            None => tempfile::tempdir().unwrap().keep(),
        };
        let wal = rt
            .block_on(DiskWal::open(dir.clone(), schema.clone(), wal_config()))
            .unwrap();
        let batch = make_batch(&schema, rows);

        g.throughput(Throughput::Elements(rows as u64));
        g.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, _| {
            b.iter_batched(
                || batch.clone(),
                |batch| rt.block_on(wal.append(batch)).unwrap(),
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

criterion_group!(benches, bench_wal_append);
criterion_main!(benches);

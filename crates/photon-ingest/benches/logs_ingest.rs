//! Layer-1 CPU/allocation micro-benches for the logs write path. Isolates the three hot
//! stages so a win maps to a specific audit finding:
//!   - `map`               -> otlp_logs_to_records (F1: per-record BTreeMap churn)
//!   - `build`             -> RecordBatchBuilder    (F2: zero-capacity vs with_capacity)
//!   - `decode_map_build`  -> the full per-request ingest CPU (prost decode + map + build)

mod fixture;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use photon_core::record::RecordBatchBuilder;
use photon_core::schema::LogSchema;
use photon_ingest::otlp_logs_to_records;

// (rows, resource_attrs, attrs_per_record) shapes: a small hot batch and a large one.
const SHAPES: &[(usize, usize, usize)] = &[(500, 4, 8), (10_000, 4, 8)];

fn schema() -> LogSchema {
    LogSchema::new(&["service.name".to_string(), "host.name".to_string()])
}

fn bench_map(c: &mut Criterion) {
    let mut g = c.benchmark_group("map");
    for &(rows, ra, ar) in SHAPES {
        g.throughput(Throughput::Elements(rows as u64));
        g.bench_with_input(
            BenchmarkId::from_parameter(rows),
            &(rows, ra, ar),
            |b, &(rows, ra, ar)| {
                b.iter_batched(
                    || fixture::logs_request(rows, ra, ar),
                    |req| otlp_logs_to_records(std::hint::black_box(req)),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    g.finish();
}

fn bench_build(c: &mut Criterion) {
    let schema = schema();
    let mut g = c.benchmark_group("build");
    for &(rows, ra, ar) in SHAPES {
        let records = otlp_logs_to_records(fixture::logs_request(rows, ra, ar));
        g.throughput(Throughput::Elements(rows as u64));
        g.bench_with_input(BenchmarkId::from_parameter(rows), &records, |b, records| {
            b.iter(|| {
                let mut builder = RecordBatchBuilder::new(&schema);
                for r in records {
                    builder.append(r);
                }
                std::hint::black_box(builder.finish().unwrap())
            });
        });
    }
    g.finish();
}

fn bench_decode_map_build(c: &mut Criterion) {
    let schema = schema();
    let mut g = c.benchmark_group("decode_map_build");
    for &(rows, ra, ar) in SHAPES {
        let bytes = fixture::logs_request_bytes(rows, ra, ar);
        g.throughput(Throughput::Elements(rows as u64));
        g.bench_with_input(BenchmarkId::from_parameter(rows), &bytes, |b, bytes| {
            b.iter(|| {
                let req: ExportLogsServiceRequest = prost::Message::decode(&bytes[..]).unwrap();
                let records = otlp_logs_to_records(req);
                let mut builder = RecordBatchBuilder::new(&schema);
                for r in &records {
                    builder.append(r);
                }
                std::hint::black_box(builder.finish().unwrap())
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench_map, bench_build, bench_decode_map_build);
criterion_main!(benches);

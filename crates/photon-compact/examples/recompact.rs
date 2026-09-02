//! Dev-only OFFLINE recompaction: rewrite a hot tier into a new directory with every storage
//! improvement applied at once, so the win can be measured on a whole real corpus instead of a
//! sample. Reads the source read-only; writes a complete, queryable hot tier (Parquet + `.idx`
//! sidecars + manifests) that `photon-server` can be pointed at unchanged.
//!
//! What it does, per signal:
//!   1. merges the pile of one-minute files into `--rows`-row files (the merge that is supposed
//!      to happen online),
//!   2. HOISTS every `[schema].promoted_attributes` key out of the long-tail `attributes` map
//!      into its own column (the online path only does this for newly ingested rows; `align_to`
//!      leaves the column null and lets the read path fall back to the map),
//!   3. sorts with `tenant` as the LEADING key, so Parquet row-group statistics can prune a
//!      per-tenant query,
//!   4. caps every attribute value at `--attr-cap` bytes,
//!   5. rounds log `timestamp` / `observed_timestamp` to 1 ms,
//!   6. re-encodes at `--zstd`.
//!
//!   cargo run --release -p photon-compact --example recompact -- <src_hot> <dst_hot> \
//!       [rows=1000000] [zstd=6] [attr_cap=512] [ms_logs=1] [tenant_first=1] [threads=4]
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arrow::array::{
    Array, ArrayBuilder, ArrayRef, MapArray, StringArray, StringBuilder, StructArray,
    TimestampNanosecondArray,
};
use arrow::buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow::compute::{cast, concat_batches, lexsort_to_indices, take_record_batch, SortColumn};
use arrow::datatypes::{DataType, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::WriterProperties;
use parquet::schema::types::ColumnPath;

use photon_core::manifest::{
    FileEntry, Manifest, MANIFEST_OBJECT_PATH, METRICS_MANIFEST_OBJECT_PATH,
    SPANS_MANIFEST_OBJECT_PATH,
};
use photon_core::metric_schema::MetricSchema;
use photon_core::schema::LogSchema;
use photon_core::segment::SegmentId;
use photon_core::span_schema::SpanSchema;
use photon_index::SkipIndex;

const RG: usize = 131_072;

#[derive(Clone, Copy)]
struct Opts {
    rows: usize,
    zstd: i32,
    attr_cap: usize,
    ms_logs: bool,
    tenant_first: bool,
}

fn is_hex_id(n: &str) -> bool {
    n == photon_core::schema::TRACE_ID
        || n == photon_core::schema::SPAN_ID
        || n == photon_core::span_schema::PARENT_SPAN_ID
}

/// Same rule as `stream::writer_properties`; `delta_time` is the signal's TimeEncoding.
fn props(schema: &Schema, zstd: i32, delta_time: bool) -> WriterProperties {
    let mut b = WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            parquet::basic::ZstdLevel::try_new(zstd).unwrap(),
        ))
        .set_max_row_group_size(RG);
    for f in schema.fields() {
        let enc = match f.data_type() {
            DataType::Timestamp(_, _) | DataType::Int64 if delta_time => {
                Encoding::DELTA_BINARY_PACKED
            }
            DataType::Utf8 if is_hex_id(f.name()) => Encoding::DELTA_BYTE_ARRAY,
            _ => continue,
        };
        let p = ColumnPath::from(f.name().as_str());
        b = b
            .set_column_dictionary_enabled(p.clone(), false)
            .set_column_encoding(p, enc);
    }
    b.build()
}

/// Rebuild a Map column from parts, forced to the target schema's exact Map type.
fn map_from_parts(
    keys: StringArray,
    vals: StringArray,
    offsets: Vec<i32>,
    valid: Vec<bool>,
    target: &DataType,
) -> ArrayRef {
    let DataType::Map(entry_field, ordered) = target else {
        panic!("attributes is not a Map")
    };
    let DataType::Struct(fields) = entry_field.data_type() else {
        panic!("map entry is not a Struct")
    };
    let entries = StructArray::try_new(
        fields.clone(),
        vec![Arc::new(keys) as ArrayRef, Arc::new(vals) as ArrayRef],
        None,
    )
    .unwrap();
    let nulls = if valid.iter().all(|v| *v) {
        None
    } else {
        Some(NullBuffer::from(valid))
    };
    Arc::new(
        MapArray::try_new(
            entry_field.clone(),
            OffsetBuffer::new(ScalarBuffer::from(offsets)),
            entries,
            nulls,
            *ordered,
        )
        .unwrap(),
    )
}

/// Hoist promoted keys out of the map into their own columns, cap value lengths, round times,
/// and lay the columns out in `target`'s order.
fn reshape(
    batch: &RecordBatch,
    target: &SchemaRef,
    promoted: &[String],
    opts: Opts,
    round_ns: &[&str],
) -> RecordBatch {
    let n = batch.num_rows();
    let attrs_name = photon_core::schema::ATTRIBUTES;
    let map = batch
        .column_by_name(attrs_name)
        .expect("attributes")
        .as_any()
        .downcast_ref::<MapArray>()
        .unwrap();
    let mkeys = map.keys().as_any().downcast_ref::<StringArray>().unwrap();
    let mvals = map.values().as_any().downcast_ref::<StringArray>().unwrap();
    let offs = map.offsets();

    // Which promoted keys still need hoisting? (missing column, or a column that is entirely null)
    let hoist: Vec<&String> = promoted
        .iter()
        .filter(|k| match batch.column_by_name(k) {
            None => true,
            Some(c) => c.null_count() == c.len(),
        })
        .collect();

    let mut hoisted: HashMap<&str, StringBuilder> = hoist
        .iter()
        .map(|k| (k.as_str(), StringBuilder::new()))
        .collect();
    let hoist_set: BTreeSet<&str> = hoist.iter().map(|k| k.as_str()).collect();

    let mut kb = StringBuilder::new();
    let mut vb = StringBuilder::new();
    let mut offsets: Vec<i32> = Vec::with_capacity(n + 1);
    offsets.push(0);
    let mut valid: Vec<bool> = Vec::with_capacity(n);
    let mut seen: HashMap<&str, Option<&str>> = HashMap::new();

    for r in 0..n {
        seen.clear();
        if map.is_null(r) {
            valid.push(false);
        } else {
            valid.push(true);
            for i in offs[r] as usize..offs[r + 1] as usize {
                let k = mkeys.value(i);
                let v = if mvals.is_null(i) {
                    None
                } else {
                    Some(mvals.value(i))
                };
                if hoist_set.contains(k) {
                    seen.entry(k).or_insert(v);
                    continue; // hoisted out of the map
                }
                kb.append_value(k);
                match v {
                    None => vb.append_null(),
                    Some(v) if v.len() > opts.attr_cap => {
                        let mut e = opts.attr_cap;
                        while !v.is_char_boundary(e) {
                            e -= 1;
                        }
                        vb.append_value(&v[..e])
                    }
                    Some(v) => vb.append_value(v),
                }
            }
        }
        offsets.push(ArrayBuilder::len(&kb) as i32);
        for k in &hoist {
            let b = hoisted.get_mut(k.as_str()).unwrap();
            match seen.get(k.as_str()).copied().flatten() {
                Some(v) => b.append_value(v),
                None => b.append_null(),
            }
        }
    }

    let new_map = map_from_parts(
        kb.finish(),
        vb.finish(),
        offsets,
        valid,
        target.field_with_name(attrs_name).unwrap().data_type(),
    );
    let mut hoisted_arrays: HashMap<&str, ArrayRef> = hoisted
        .into_iter()
        .map(|(k, mut b)| (k, Arc::new(b.finish()) as ArrayRef))
        .collect();

    let mut cols: Vec<ArrayRef> = Vec::with_capacity(target.fields().len());
    for f in target.fields() {
        let name = f.name().as_str();
        if name == attrs_name {
            cols.push(new_map.clone());
            continue;
        }
        if let Some(a) = hoisted_arrays.remove(name) {
            cols.push(a);
            continue;
        }
        let c = match batch.column_by_name(name) {
            Some(c) if c.data_type() == f.data_type() => Arc::clone(c),
            Some(c) => cast(c, f.data_type()).unwrap(),
            None => arrow::array::new_null_array(f.data_type(), n),
        };
        // Round the nanosecond wall-clock columns this signal asked for.
        if round_ns.contains(&name) {
            if let Some(t) = c.as_any().downcast_ref::<TimestampNanosecondArray>() {
                let r: TimestampNanosecondArray = t
                    .iter()
                    .map(|v| v.map(|v| v / 1_000_000 * 1_000_000))
                    .collect();
                cols.push(Arc::new(r));
                continue;
            }
        }
        cols.push(c);
    }
    RecordBatch::try_new(target.clone(), cols).unwrap()
}

fn attribute_keys(batch: &RecordBatch) -> Vec<String> {
    let map = batch
        .column_by_name(photon_core::schema::ATTRIBUTES)
        .unwrap()
        .as_any()
        .downcast_ref::<MapArray>()
        .unwrap();
    let keys = map.keys().as_any().downcast_ref::<StringArray>().unwrap();
    let mut set: BTreeSet<&str> = BTreeSet::new();
    for i in 0..keys.len() {
        set.insert(keys.value(i));
    }
    set.into_iter().map(|s| s.to_string()).collect()
}

struct SignalPlan {
    label: &'static str,
    manifest_obj: &'static str,
    dir: &'static str,
    target: SchemaRef,
    promoted: Vec<String>,
    sort_keys: Vec<String>,
    delta_time: bool,
    round_ns: Vec<&'static str>,
    build_index: fn(&RecordBatch, &LogSchema) -> SkipIndex,
    log_schema: LogSchema,
}

fn dur(t: std::time::Instant) -> String {
    format!("{:.0}s", t.elapsed().as_secs_f64())
}

fn run(src: &Path, dst: &Path, plan: &SignalPlan, opts: Opts, threads: usize) {
    let t0 = std::time::Instant::now();
    let m = Manifest::from_json(&std::fs::read_to_string(src.join(plan.manifest_obj)).unwrap())
        .unwrap();
    let mut entries: Vec<FileEntry> = m.entries().to_vec();
    entries.sort_by_key(|e| e.min_ts_nanos);
    let src_bytes: u64 = entries
        .iter()
        .map(|e| {
            std::fs::metadata(src.join(&e.path))
                .map(|x| x.len())
                .unwrap_or(0)
        })
        .sum();
    let src_idx: u64 = entries
        .iter()
        .map(|e| {
            std::fs::metadata(src.join(&e.path).with_extension("idx"))
                .map(|x| x.len())
                .unwrap_or(0)
        })
        .sum();
    let src_rows: u64 = entries.iter().map(|e| e.row_count).sum();
    let src_files = entries.len();

    // Group time-adjacent files up to `opts.rows` rows.
    let mut groups: Vec<Vec<FileEntry>> = Vec::new();
    let mut cur: Vec<FileEntry> = Vec::new();
    let mut cur_rows = 0u64;
    for e in entries {
        cur_rows += e.row_count;
        cur.push(e);
        if cur_rows >= opts.rows as u64 {
            groups.push(std::mem::take(&mut cur));
            cur_rows = 0;
        }
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    std::fs::create_dir_all(dst.join(plan.dir)).unwrap();
    std::fs::create_dir_all(dst.join("manifest")).unwrap();

    let out: Mutex<Vec<FileEntry>> = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let total = groups.len();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let g = next.fetch_add(1, Ordering::Relaxed);
                if g >= total {
                    break;
                }
                let group = &groups[g];
                let mut seg = SegmentId::first_merged();
                for _ in 0..g {
                    seg = seg.next();
                }
                let mut batches = Vec::new();
                for e in group {
                    let f = File::open(src.join(&e.path)).unwrap();
                    for b in ParquetRecordBatchReaderBuilder::try_new(f)
                        .unwrap()
                        .build()
                        .unwrap()
                    {
                        let b = b.unwrap();
                        batches.push(reshape(
                            &b,
                            &plan.target,
                            &plan.promoted,
                            opts,
                            &plan.round_ns,
                        ));
                    }
                }
                let cat = concat_batches(&plan.target, &batches).unwrap();
                drop(batches);
                let sc: Vec<SortColumn> = plan
                    .sort_keys
                    .iter()
                    .filter_map(|k| cat.column_by_name(k))
                    .map(|c| SortColumn {
                        values: c.clone(),
                        options: None,
                    })
                    .collect();
                let idx = lexsort_to_indices(&sc, None).unwrap();
                let sorted = take_record_batch(&cat, &idx).unwrap();
                drop(cat);

                let name = format!("seg-{:016x}", seg.0);
                let pq = dst.join(plan.dir).join(format!("{name}.parquet"));
                let f = File::create(&pq).unwrap();
                let mut w = ArrowWriter::try_new(
                    f,
                    sorted.schema(),
                    Some(props(&plan.target, opts.zstd, plan.delta_time)),
                )
                .unwrap();
                w.write(&sorted).unwrap();
                w.close().unwrap();
                let bytes = std::fs::metadata(&pq).unwrap().len();

                let index = (plan.build_index)(&sorted, &plan.log_schema);
                std::fs::write(
                    dst.join(plan.dir).join(format!("{name}.idx")),
                    index.to_bytes(),
                )
                .unwrap();
                let (min_ts, max_ts) = index.timestamp_range().unwrap_or((0, 0));
                let (min_service, max_service) = index.service_range().unwrap_or_default();
                out.lock().unwrap().push(FileEntry {
                    path: format!("{}/{name}.parquet", plan.dir),
                    segment_id: seg,
                    min_ts_nanos: min_ts,
                    max_ts_nanos: max_ts,
                    min_service,
                    max_service,
                    row_count: sorted.num_rows() as u64,
                    durable: false,
                    attribute_keys: attribute_keys(&sorted),
                    bytes,
                    tenant_range: index.tenant_range(),
                });
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                if d % 10 == 0 || d == total {
                    eprintln!("  {} {d}/{total} groups", plan.label);
                }
            });
        }
    });

    let mut entries = out.into_inner().unwrap();
    entries.sort_by_key(|e| e.min_ts_nanos);
    let dst_rows: u64 = entries.iter().map(|e| e.row_count).sum();
    let dst_bytes: u64 = entries.iter().map(|e| e.bytes).sum();
    let dst_idx: u64 = entries
        .iter()
        .map(|e| {
            std::fs::metadata(dst.join(&e.path).with_extension("idx"))
                .map(|x| x.len())
                .unwrap_or(0)
        })
        .sum();
    let n_files = entries.len();
    let mut nm = Manifest::new();
    for e in entries {
        nm.add(e);
    }
    let json = nm.to_json().unwrap();
    let mjson = json.len() as u64;
    std::fs::write(dst.join(plan.manifest_obj), json).unwrap();

    assert_eq!(src_rows, dst_rows, "{} lost rows!", plan.label);
    let src_m = std::fs::metadata(src.join(plan.manifest_obj))
        .unwrap()
        .len();
    println!(
        "{:<8} {:>13} rows   files {:>6} -> {:<5}  parquet {:>8.2} -> {:<8} ({:+.1}%)  idx {:>7.1} -> {:<7}  manifest {:>6.1} -> {:<7}  [{}]",
        plan.label,
        dst_rows,
        src_files,
        n_files,
        src_bytes as f64 / 1073741824.0,
        format!("{:.2} GiB", dst_bytes as f64 / 1073741824.0),
        (dst_bytes as f64 - src_bytes as f64) / src_bytes as f64 * 100.0,
        format!("{:.1} MiB", src_idx as f64 / 1048576.0),
        format!("{:.1} MiB", dst_idx as f64 / 1048576.0),
        format!("{:.1} MiB", src_m as f64 / 1048576.0),
        format!("{:.1} MiB", mjson as f64 / 1048576.0),
        dur(t0)
    );
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let src = PathBuf::from(&a[1]);
    let dst = PathBuf::from(&a[2]);
    let g = |i: usize, d: &str| a.get(i).cloned().unwrap_or_else(|| d.to_string());
    let opts = Opts {
        rows: g(3, "1000000").parse().unwrap(),
        zstd: g(4, "6").parse().unwrap(),
        attr_cap: g(5, "512").parse().unwrap(),
        ms_logs: g(6, "1") == "1",
        tenant_first: g(7, "1") == "1",
    };
    let threads: usize = g(8, "4").parse().unwrap();

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

    println!(
        "src {} -> dst {}\nrows/file {}, zstd {}, attr cap {} B, ms log timestamps {}, tenant-first sort {}, {} threads\npromoted: {:?}\n",
        src.display(), dst.display(), opts.rows, opts.zstd, opts.attr_cap, opts.ms_logs, opts.tenant_first, threads, promoted
    );

    let logs = LogSchema::new(&promoted);
    let spans = SpanSchema::new(&promoted);
    let metrics = MetricSchema::new(&promoted);

    let tk = |base: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if opts.tenant_first {
            v.push("tenant".to_string());
        }
        v.extend(base.iter().map(|s| s.to_string()));
        v
    };

    let plans = vec![
        SignalPlan {
            label: "logs",
            manifest_obj: MANIFEST_OBJECT_PATH,
            dir: "data",
            target: logs.arrow.clone(),
            promoted: promoted.clone(),
            sort_keys: tk(&["service.name", "timestamp"]),
            delta_time: true,
            round_ns: if opts.ms_logs {
                vec!["timestamp", "observed_timestamp"]
            } else {
                vec![]
            },
            build_index: |b, s| SkipIndex::build(b, s).unwrap(),
            log_schema: logs.clone(),
        },
        SignalPlan {
            label: "spans",
            manifest_obj: SPANS_MANIFEST_OBJECT_PATH,
            dir: "data-spans",
            target: spans.arrow.clone(),
            promoted: promoted.clone(),
            sort_keys: tk(&["service.name", "start_time_nanos"]),
            delta_time: true,
            round_ns: vec![],
            build_index: |b, _| SkipIndex::build_spans(b).unwrap(),
            log_schema: logs.clone(),
        },
        SignalPlan {
            label: "metrics",
            manifest_obj: METRICS_MANIFEST_OBJECT_PATH,
            dir: "data-metrics",
            target: metrics.arrow.clone(),
            promoted: promoted.clone(),
            sort_keys: tk(&["metric_name", "service.name", "host.name", "timestamp"]),
            delta_time: false,
            round_ns: vec![],
            build_index: |b, _| SkipIndex::build_metrics(b).unwrap(),
            log_schema: logs.clone(),
        },
    ];

    for p in &plans {
        run(&src, &dst, p, opts, threads);
    }
}

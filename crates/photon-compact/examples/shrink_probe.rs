//! Dev-only: measure what each candidate storage change is actually worth, on real data.
//!   cargo run --release -p photon-compact --example shrink_probe -- <dir> <sort_keys> <n_files>
use std::collections::HashSet;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Int64Array, MapArray, MapBuilder, StringArray, StringBuilder,
    TimestampNanosecondArray,
};
use arrow::compute::{concat_batches, lexsort_to_indices, take_record_batch, SortColumn};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::WriterProperties;
use parquet::schema::types::ColumnPath;

fn is_hex_id(n: &str) -> bool {
    n == "trace_id" || n == "span_id" || n == "parent_span_id"
}

fn props(schema: &Schema, zstd: i32, rg: usize) -> WriterProperties {
    let mut b = WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            parquet::basic::ZstdLevel::try_new(zstd).unwrap(),
        ))
        .set_max_row_group_size(rg);
    for f in schema.fields() {
        let enc = match f.data_type() {
            DataType::Timestamp(_, _) | DataType::Int64 => Encoding::DELTA_BINARY_PACKED,
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

fn size_of(batch: &RecordBatch, zstd: i32, rg: usize) -> u64 {
    let tmp = std::env::temp_dir().join("photon_shrink.parquet");
    let f = File::create(&tmp).unwrap();
    let mut w =
        ArrowWriter::try_new(f, batch.schema(), Some(props(&batch.schema(), zstd, rg))).unwrap();
    w.write(batch).unwrap();
    w.close().unwrap();
    let s = std::fs::metadata(&tmp).unwrap().len();
    let _ = std::fs::remove_file(&tmp);
    s
}

/// Rebuild the `attributes` map with `drop` keys removed.
fn strip_keys(batch: &RecordBatch, drop: &HashSet<&str>) -> RecordBatch {
    let idx = batch.schema().index_of("attributes").unwrap();
    let m = batch
        .column(idx)
        .as_any()
        .downcast_ref::<MapArray>()
        .unwrap();
    let keys = m.keys().as_any().downcast_ref::<StringArray>().unwrap();
    let vals = m.values().as_any().downcast_ref::<StringArray>().unwrap();
    let offs = m.offsets();
    let mut b = MapBuilder::new(None, StringBuilder::new(), StringBuilder::new());
    for r in 0..m.len() {
        if m.is_null(r) {
            b.append(false).unwrap();
            continue;
        }
        for i in offs[r] as usize..offs[r + 1] as usize {
            let k = keys.value(i);
            if drop.contains(k) {
                continue;
            }
            b.keys().append_value(k);
            if vals.is_null(i) {
                b.values().append_null()
            } else {
                b.values().append_value(vals.value(i))
            }
        }
        b.append(true).unwrap();
    }
    let new_map: ArrayRef = Arc::new(b.finish());
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    fields[idx] = Field::new("attributes", new_map.data_type().clone(), true);
    let mut cols: Vec<ArrayRef> = batch.columns().to_vec();
    cols[idx] = new_map;
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).unwrap()
}

/// Cap one attribute's value length (keep the key, truncate the value).
fn truncate_key(batch: &RecordBatch, key: &str, max: usize) -> RecordBatch {
    let idx = batch.schema().index_of("attributes").unwrap();
    let m = batch
        .column(idx)
        .as_any()
        .downcast_ref::<MapArray>()
        .unwrap();
    let keys = m.keys().as_any().downcast_ref::<StringArray>().unwrap();
    let vals = m.values().as_any().downcast_ref::<StringArray>().unwrap();
    let offs = m.offsets();
    let mut b = MapBuilder::new(None, StringBuilder::new(), StringBuilder::new());
    for r in 0..m.len() {
        if m.is_null(r) {
            b.append(false).unwrap();
            continue;
        }
        for i in offs[r] as usize..offs[r + 1] as usize {
            let k = keys.value(i);
            b.keys().append_value(k);
            if vals.is_null(i) {
                b.values().append_null()
            } else {
                let v = vals.value(i);
                if k == key && v.len() > max {
                    b.values().append_value({
                        let mut e = max;
                        while !v.is_char_boundary(e) {
                            e -= 1;
                        }
                        &v[..e]
                    })
                } else {
                    b.values().append_value(v)
                }
            }
        }
        b.append(true).unwrap();
    }
    let new_map: ArrayRef = Arc::new(b.finish());
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    fields[idx] = Field::new("attributes", new_map.data_type().clone(), true);
    let mut cols: Vec<ArrayRef> = batch.columns().to_vec();
    cols[idx] = new_map;
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).unwrap()
}

fn drop_col(batch: &RecordBatch, name: &str) -> RecordBatch {
    let Ok(idx) = batch.schema().index_of(name) else {
        return batch.clone();
    };
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    fields.remove(idx);
    let mut cols = batch.columns().to_vec();
    cols.remove(idx);
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).unwrap()
}

/// Round an Int64 / Timestamp(ns) column down to `div` nanoseconds.
fn coarsen(batch: &RecordBatch, name: &str, div: i64) -> RecordBatch {
    let Ok(idx) = batch.schema().index_of(name) else {
        return batch.clone();
    };
    let c = batch.column(idx);
    let new: ArrayRef = if let Some(a) = c.as_any().downcast_ref::<Int64Array>() {
        Arc::new(
            a.iter()
                .map(|v| v.map(|v| v / div * div))
                .collect::<Int64Array>(),
        )
    } else if let Some(a) = c.as_any().downcast_ref::<TimestampNanosecondArray>() {
        Arc::new(
            a.iter()
                .map(|v| v.map(|v| v / div * div))
                .collect::<TimestampNanosecondArray>(),
        )
    } else {
        return batch.clone();
    };
    let mut cols = batch.columns().to_vec();
    cols[idx] = new;
    RecordBatch::try_new(batch.schema(), cols).unwrap()
}

/// Replace `name` with (name - base), i.e. store the offset instead of the absolute stamp.
fn as_delta(batch: &RecordBatch, name: &str, base: &str) -> RecordBatch {
    let (Ok(i), Ok(j)) = (batch.schema().index_of(name), batch.schema().index_of(base)) else {
        return batch.clone();
    };
    let get = |k: usize| -> Vec<Option<i64>> {
        let c = batch.column(k);
        if let Some(a) = c.as_any().downcast_ref::<TimestampNanosecondArray>() {
            a.iter().collect()
        } else {
            c.as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .iter()
                .collect()
        }
    };
    let (a, bb) = (get(i), get(j));
    let d: Int64Array = a
        .iter()
        .zip(bb.iter())
        .map(|(x, y)| match (x, y) {
            (Some(x), Some(y)) => Some(x - y),
            _ => *x,
        })
        .collect();
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    fields[i] = Field::new(name, DataType::Int64, true);
    let mut cols = batch.columns().to_vec();
    cols[i] = Arc::new(d);
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).unwrap()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&a[1]);
    let keys: Vec<String> = a[2].split(',').map(|s| s.to_string()).collect();
    let n: usize = a[3].parse().unwrap();

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "parquet").unwrap_or(false))
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("seg-0")
        })
        .collect();
    files.sort();
    let files: Vec<PathBuf> = files.into_iter().take(n).collect();
    let mut in_bytes = 0u64;
    let mut batches = Vec::new();
    for p in &files {
        in_bytes += std::fs::metadata(p).unwrap().len();
        let f = File::open(p).unwrap();
        for b in ParquetRecordBatchReaderBuilder::try_new(f)
            .unwrap()
            .build()
            .unwrap()
        {
            batches.push(b.unwrap());
        }
    }
    let schema = batches[0].schema();
    let merged = concat_batches(&schema, &batches).unwrap();
    drop(batches);
    let sc: Vec<SortColumn> = keys
        .iter()
        .map(|k| SortColumn {
            values: merged.column_by_name(k).expect(k).clone(),
            options: None,
        })
        .collect();
    let idx = lexsort_to_indices(&sc, None).unwrap();
    let base = take_record_batch(&merged, &idx).unwrap();
    drop(merged);
    let rows = base.num_rows();

    let b0 = size_of(&base, 6, 1_048_576);
    println!(
        "{} files, {} rows.  on disk today (many small files, zstd 1): {:.2} MiB",
        files.len(),
        rows,
        in_bytes as f64 / 1048576.0
    );
    println!(
        "BASELINE  merged, zstd 6, rg 1M: {:.2} MiB ({:.2} B/row)\n",
        b0 as f64 / 1048576.0,
        b0 as f64 / rows as f64
    );

    let report = |label: &str, b: &RecordBatch| {
        let s = size_of(b, 6, 1_048_576);
        println!(
            "  {:<52} {:>9.2} MiB  {:>7.2} B/row  {:>+6.1}%",
            label,
            s as f64 / 1048576.0,
            s as f64 / rows as f64,
            (s as f64 - b0 as f64) / b0 as f64 * 100.0
        );
    };

    // Constant RESOURCE attributes: OTLP sends these once per batch, we copy them onto every row.
    let resource: HashSet<&str> = [
        "os.description",
        "os.type",
        "process.command_args",
        "process.runtime.description",
        "process.runtime.version",
        "process.runtime.name",
        "process.executable.path",
        "process.executable.name",
        "process.owner",
        "process.pid",
        "service.version",
        "host.id",
        "host.arch",
        "telemetry.sdk.name",
        "telemetry.sdk.version",
        "telemetry.sdk.language",
    ]
    .into_iter()
    .collect();
    report(
        "− constant resource attributes (16 keys)",
        &strip_keys(&base, &resource),
    );
    report(
        "− db.query.text",
        &strip_keys(&base, &["db.query.text"].into_iter().collect()),
    );
    report(
        "− end_time_nanos (derivable: start + duration)",
        &drop_col(&base, "end_time_nanos"),
    );
    report("− events column", &drop_col(&base, "events"));
    report(
        "duration_nanos rounded to 1 µs",
        &coarsen(&base, "duration_nanos", 1_000),
    );
    report(
        "start/end/duration rounded to 1 µs",
        &coarsen(
            &coarsen(
                &coarsen(&base, "duration_nanos", 1_000),
                "start_time_nanos",
                1_000,
            ),
            "end_time_nanos",
            1_000,
        ),
    );
    report(
        "observed_timestamp stored as delta from timestamp",
        &as_delta(&base, "observed_timestamp", "timestamp"),
    );
    report(
        "− observed_timestamp",
        &drop_col(&base, "observed_timestamp"),
    );

    for cap in [8192usize, 4096, 2048, 1024, 512] {
        let mut b = base.clone();
        // cap EVERY attribute value, which is what an ingest-side knob would do
        for k in [
            "db.query.text",
            "os.description",
            "process.command_args",
            "db.query.summary",
            "mandor.bundle",
            "err",
            "error",
        ] {
            b = truncate_key(&b, k, cap);
        }
        report(&format!("all attribute values capped at {cap} B"), &b);
    }
    report(
        "db.query.text truncated to 512 chars",
        &truncate_key(&base, "db.query.text", 512),
    );
    report(
        "db.query.text truncated to 256 chars",
        &truncate_key(&base, "db.query.text", 256),
    );
    report(
        "db.query.text truncated to 64 chars",
        &truncate_key(&base, "db.query.text", 64),
    );
    report(
        "timestamp rounded to 1 ms",
        &coarsen(&base, "timestamp", 1_000_000),
    );
    report(
        "timestamp + observed rounded to 1 ms",
        &coarsen(
            &coarsen(&base, "timestamp", 1_000_000),
            "observed_timestamp",
            1_000_000,
        ),
    );
    report(
        "observed as delta from timestamp, rounded to 1 ms",
        &coarsen(
            &as_delta(&base, "observed_timestamp", "timestamp"),
            "observed_timestamp",
            1_000_000,
        ),
    );
    report(
        "start_time rounded to 1 ms",
        &coarsen(&base, "start_time_nanos", 1_000_000),
    );

    // Spans: everything that is free or near-free, together.
    let combo = coarsen(
        &drop_col(
            &truncate_key(&strip_keys(&base, &resource), "db.query.text", 256),
            "end_time_nanos",
        ),
        "start_time_nanos",
        1_000_000,
    );
    report(
        "SPANS COMBO: −resource, query.text@256, −end_time, ms start",
        &combo,
    );
    // Logs: everything that is free, together.
    let lcombo = coarsen(
        &coarsen(&strip_keys(&base, &resource), "timestamp", 1_000_000),
        "observed_timestamp",
        1_000_000,
    );
    report("LOGS COMBO: −resource, ms timestamps", &lcombo);

    // Does a higher zstd level cost the READ path anything?
    for lvl in [1i32, 6, 12] {
        let tmp = std::env::temp_dir().join(format!("photon_dec_{lvl}.parquet"));
        let f = File::create(&tmp).unwrap();
        let mut w = ArrowWriter::try_new(
            f,
            base.schema(),
            Some(props(&base.schema(), lvl, 1_048_576)),
        )
        .unwrap();
        w.write(&base).unwrap();
        w.close().unwrap();
        let sz = std::fs::metadata(&tmp).unwrap().len();
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let f = File::open(&tmp).unwrap();
            let mut nr = 0usize;
            for b in ParquetRecordBatchReaderBuilder::try_new(f)
                .unwrap()
                .build()
                .unwrap()
            {
                nr += b.unwrap().num_rows();
            }
            assert_eq!(nr, rows);
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "  decode ALL columns, zstd {:<2}: {:>8.2} MiB, {:>7.0} ms",
            lvl,
            sz as f64 / 1048576.0,
            best
        );
        let _ = std::fs::remove_file(&tmp);
    }

    let all = drop_col(
        &coarsen(
            &as_delta(
                &strip_keys(&base, &resource),
                "observed_timestamp",
                "timestamp",
            ),
            "duration_nanos",
            1_000,
        ),
        "end_time_nanos",
    );
    report("ALL of the above combined", &all);
}

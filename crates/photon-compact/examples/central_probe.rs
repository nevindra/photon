//! Dev-only: what a federation CENTRAL costs, measured on real data.
//!
//!   central_probe tenants <spans_dir> <n_files> <n_tenants>   layout of the tenant dimension
//!   central_probe rollup  <metrics_dir> <n_files>             how much of metrics is the rollup
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, MapArray, MapBuilder, StringArray, StringBuilder};
use arrow::compute::{concat_batches, lexsort_to_indices, take_record_batch, SortColumn};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::WriterProperties;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::file::statistics::Statistics;
use parquet::schema::types::ColumnPath;

const RG: usize = 131_072; // MAX_ROW_GROUP_SIZE, as shipped
const ZSTD: i32 = 6;

fn is_hex_id(n: &str) -> bool {
    n == "trace_id" || n == "span_id" || n == "parent_span_id"
}

fn props(schema: &Schema) -> WriterProperties {
    let mut b = WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            parquet::basic::ZstdLevel::try_new(ZSTD).unwrap(),
        ))
        .set_max_row_group_size(RG);
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

fn load(dir: &PathBuf, n: usize) -> (Vec<RecordBatch>, u64) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
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
    let mut bytes = 0;
    let mut out = Vec::new();
    let sel: Vec<&PathBuf> = if std::env::var("NEWEST").is_ok() {
        files.iter().rev().take(n).collect()
    } else {
        files.iter().take(n).collect()
    };
    for p in sel {
        bytes += std::fs::metadata(p).unwrap().len();
        let f = File::open(p).unwrap();
        for b in ParquetRecordBatchReaderBuilder::try_new(f)
            .unwrap()
            .build()
            .unwrap()
        {
            out.push(b.unwrap());
        }
    }
    (out, bytes)
}

/// Overwrite / insert `tenant` inside the attributes map (today's central layout).
fn tenant_in_map(batch: &RecordBatch, tenants: &[String]) -> RecordBatch {
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
        let mut done = false;
        for i in offs[r] as usize..offs[r + 1] as usize {
            let k = keys.value(i);
            b.keys().append_value(k);
            if k == "tenant" {
                b.values().append_value(&tenants[r]);
                done = true;
            } else if vals.is_null(i) {
                b.values().append_null()
            } else {
                b.values().append_value(vals.value(i))
            }
        }
        if !done {
            b.keys().append_value("tenant");
            b.values().append_value(&tenants[r]);
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
    let mut cols = batch.columns().to_vec();
    cols[idx] = new_map;
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).unwrap()
}

/// Promote `tenant` to its own top-level column and drop it from the map.
fn tenant_promoted(batch: &RecordBatch, tenants: &[String]) -> RecordBatch {
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
            if k == "tenant" {
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
    let mut cols = batch.columns().to_vec();
    cols[idx] = new_map;
    fields.insert(0, Field::new("tenant", DataType::Utf8, false));
    cols.insert(
        0,
        Arc::new(
            tenants
                .iter()
                .map(|s| Some(s.as_str()))
                .collect::<StringArray>(),
        ),
    );
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).unwrap()
}

fn sort_by(batch: &RecordBatch, keys: &[&str]) -> RecordBatch {
    let sc: Vec<SortColumn> = keys
        .iter()
        .map(|k| SortColumn {
            values: batch.column_by_name(k).expect(k).clone(),
            options: None,
        })
        .collect();
    let idx = lexsort_to_indices(&sc, None).unwrap();
    take_record_batch(batch, &idx).unwrap()
}

/// Write, then report size + how much a `tenant:<pick>` query must decode after Parquet
/// row-group statistics pruning.
fn measure(label: &str, batch: &RecordBatch, pick: &str, rows: usize) {
    let tmp = std::env::temp_dir().join("photon_central.parquet");
    let f = File::create(&tmp).unwrap();
    let mut w = ArrowWriter::try_new(f, batch.schema(), Some(props(&batch.schema()))).unwrap();
    w.write(batch).unwrap();
    w.close().unwrap();
    let total = std::fs::metadata(&tmp).unwrap().len();

    let r = SerializedFileReader::new(File::open(&tmp).unwrap()).unwrap();
    let md = r.metadata();
    let mut kept_bytes = 0i64;
    let mut kept_rgs = 0usize;
    let mut all_bytes = 0i64;
    for i in 0..md.num_row_groups() {
        let rg = md.row_group(i);
        all_bytes += rg.compressed_size();
        // Does this row group's `tenant` (column or map value) range possibly contain `pick`?
        let mut keep = true;
        for c in rg.columns() {
            let p = c.column_path().string();
            if p != "tenant" && p != "attributes.entries.values" {
                continue;
            }
            if let Some(Statistics::ByteArray(s)) = c.statistics() {
                if let (Some(lo), Some(hi)) = (s.min_opt(), s.max_opt()) {
                    let (lo, hi) = (lo.as_utf8().unwrap_or(""), hi.as_utf8().unwrap_or(""));
                    if p == "tenant" {
                        keep = pick >= lo && pick <= hi;
                    }
                }
            }
        }
        if keep {
            kept_bytes += rg.compressed_size();
            kept_rgs += 1;
        }
    }
    println!(
        "  {:<50} {:>8.2} MiB ({:>6.2} B/row)   {:>2}/{:<2} row groups, {:>5.1}% of bytes scanned for one tenant",
        label, total as f64 / 1048576.0, total as f64 / rows as f64,
        kept_rgs, md.num_row_groups(), kept_bytes as f64 / all_bytes.max(1) as f64 * 100.0
    );
    let _ = std::fs::remove_file(&tmp);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "tenants" => {
            let dir = PathBuf::from(&a[2]);
            let n: usize = a[3].parse().unwrap();
            let nt: usize = a[4].parse().unwrap();
            let (batches, on_disk) = load(&dir, n);
            let schema = batches[0].schema();
            let base = concat_batches(&schema, &batches).unwrap();
            drop(batches);
            let rows = base.num_rows();
            // Batch-level interleaving: central's WAL sees ~200-row batches arriving from all
            // Sites at once, so every segment holds every tenant.
            let tenants: Vec<String> = (0..rows)
                .map(|i| format!("site-{:02}", (i / 200) % nt))
                .collect();
            let pick = format!("site-{:02}", nt / 2);
            println!(
                "{} rows from {} files ({:.2} MiB on disk today), simulating {} Sites, row group {}\n",
                rows, n, on_disk as f64 / 1048576.0, nt, RG
            );
            let in_map = tenant_in_map(&base, &tenants);
            measure(
                "A. tenant in attributes map, sort (service, time)  [TODAY]",
                &sort_by(&in_map, &["service.name", "start_time_nanos"]),
                &pick,
                rows,
            );
            drop(in_map);
            let prom = tenant_promoted(&base, &tenants);
            drop(base);
            measure(
                "B. tenant promoted column, sort (service, time)",
                &sort_by(&prom, &["service.name", "start_time_nanos"]),
                &pick,
                rows,
            );
            measure(
                "C. tenant promoted column, sort (tenant, service, time)",
                &sort_by(&prom, &["tenant", "service.name", "start_time_nanos"]),
                &pick,
                rows,
            );
        }
        "rollup" => {
            let dir = PathBuf::from(&a[2]);
            let n: usize = a[3].parse().unwrap();
            let (batches, on_disk) = load(&dir, n);
            let mut by: std::collections::BTreeMap<String, u64> = Default::default();
            let mut rows = 0u64;
            for b in &batches {
                rows += b.num_rows() as u64;
                let mn = b
                    .column_by_name("metric_name")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                for i in 0..mn.len() {
                    let v = mn.value(i);
                    let bucket =
                        if v.starts_with("photon.service.") || v.starts_with("photon.operation.") {
                            "rollup (photon.service/operation.*)"
                        } else if v.starts_with("photon.federation.") {
                            "federation summary"
                        } else if v.starts_with("system.") {
                            "agent (system.*)"
                        } else {
                            "everything else"
                        };
                    *by.entry(bucket.to_string()).or_default() += 1;
                }
            }
            println!(
                "{} metric rows from {} files, {:.2} MiB on disk",
                rows,
                n,
                on_disk as f64 / 1048576.0
            );
            for (k, v) in &by {
                println!(
                    "  {:<40} {:>10} rows  {:>5.1}%  ~{:.1} MiB",
                    k,
                    v,
                    *v as f64 / rows as f64 * 100.0,
                    on_disk as f64 / 1048576.0 * (*v as f64 / rows as f64)
                );
            }
        }
        _ => panic!("mode?"),
    }
}

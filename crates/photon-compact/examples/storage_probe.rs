//! Dev-only storage probe: measure what small-file merging and Parquet knob changes are worth
//! on a real corpus, without touching the corpus.
//!
//! Usage:
//!   cargo run --release -p photon-compact --example storage_probe -- \
//!       <dir> <sort_col_a> <sort_col_b> <n_files> [skip]
//!
//! Reads `n_files` Parquet files from `dir` (sorted by name, i.e. by segment id ~ time), then:
//!   1. reports the inputs' bytes/rows and per-column footprint,
//!   2. concats + lexsorts them the way the compactor does,
//!   3. re-encodes under several writer-property variants and reports each one's size.
use std::fs::File;
use std::path::PathBuf;

use arrow::compute::{concat_batches, lexsort_to_indices, take_record_batch, SortColumn};
use arrow::datatypes::{DataType, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::WriterProperties;
use parquet::file::reader::FileReader;
use parquet::file::serialized_reader::SerializedFileReader;
use parquet::schema::types::ColumnPath;

fn is_hex_id(name: &str) -> bool {
    name == "trace_id" || name == "span_id" || name == "parent_span_id"
}

/// Mirror of `stream::writer_properties`, plus the knobs we want to sweep.
fn props(
    schema: &Schema,
    zstd: i32,
    row_group: usize,
    delta_time: bool,
    dict_attrs: bool,
) -> WriterProperties {
    let level = parquet::basic::ZstdLevel::try_new(zstd).unwrap();
    let mut b = WriterProperties::builder()
        .set_compression(Compression::ZSTD(level))
        .set_max_row_group_size(row_group);
    if !dict_attrs {
        b = b.set_dictionary_enabled(true);
    }
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

fn read(path: &PathBuf) -> Vec<RecordBatch> {
    let f = File::open(path).unwrap();
    let rb = ParquetRecordBatchReaderBuilder::try_new(f)
        .unwrap()
        .with_batch_size(8192)
        .build()
        .unwrap();
    rb.map(|b| b.unwrap()).collect()
}

/// Per-leaf-column compressed + uncompressed bytes across all row groups.
fn column_footprint(path: &PathBuf) -> Vec<(String, i64, i64)> {
    let f = File::open(path).unwrap();
    let r = SerializedFileReader::new(f).unwrap();
    let md = r.metadata();
    let mut acc: std::collections::BTreeMap<String, (i64, i64)> = Default::default();
    for rg in 0..md.num_row_groups() {
        for c in md.row_group(rg).columns() {
            let e = acc.entry(c.column_path().string()).or_default();
            e.0 += c.compressed_size();
            e.1 += c.uncompressed_size();
        }
    }
    acc.into_iter().map(|(k, (c, u))| (k, c, u)).collect()
}

fn write_variant(label: &str, batch: &RecordBatch, p: WriterProperties, rows: usize, base: u64) {
    let tmp = std::env::temp_dir().join(format!(
        "photon_probe_{}.parquet",
        label.replace(['/', ' ', ','], "_")
    ));
    let f = File::create(&tmp).unwrap();
    let mut w = ArrowWriter::try_new(f, batch.schema(), Some(p)).unwrap();
    w.write(batch).unwrap();
    w.close().unwrap();
    let sz = std::fs::metadata(&tmp).unwrap().len();
    println!(
        "  {:<44} {:>10.2} MiB  {:>7.2} B/row   {:>+7.1}% vs inputs",
        label,
        sz as f64 / 1048576.0,
        sz as f64 / rows as f64,
        (sz as f64 - base as f64) / base as f64 * 100.0
    );
    let _ = std::fs::remove_file(&tmp);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&a[1]);
    let keys: Vec<String> = a[2].split(',').map(|s| s.to_string()).collect();
    let ka = keys.join(", ");
    let n: usize = a[3].parse().unwrap();
    let skip: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "parquet").unwrap_or(false))
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("seg-"))
        .collect();
    files.sort();
    let files: Vec<PathBuf> = files.into_iter().skip(skip).take(n).collect();

    let mut in_bytes = 0u64;
    let mut batches = Vec::new();
    for p in &files {
        in_bytes += std::fs::metadata(p).unwrap().len();
        batches.extend(read(p));
    }
    let schema = batches[0].schema();
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    println!(
        "INPUTS  {} files  {} rows  {:.2} MiB  ({:.2} B/row, {:.0} rows/file)",
        files.len(),
        rows,
        in_bytes as f64 / 1048576.0,
        in_bytes as f64 / rows as f64,
        rows as f64 / files.len() as f64
    );

    println!(
        "\nPER-COLUMN FOOTPRINT of one input file ({}):",
        files[0].file_name().unwrap().to_string_lossy()
    );
    let fp = column_footprint(&files[0]);
    let tot: i64 = fp.iter().map(|x| x.1).sum();
    let totu: i64 = fp.iter().map(|x| x.2).sum();
    let mut fp2 = fp.clone();
    fp2.sort_by_key(|x| -x.1);
    for (name, c, u) in fp2.iter().take(14) {
        println!(
            "  {:<46} {:>9.1} KiB compressed ({:>5.1}%)   {:>9.1} KiB raw ({:>5.1}%)   ratio {:.1}x",
            name,
            *c as f64 / 1024.0,
            *c as f64 / tot as f64 * 100.0,
            *u as f64 / 1024.0,
            *u as f64 / totu as f64 * 100.0,
            *u as f64 / (*c).max(1) as f64
        );
    }
    println!(
        "  {:<46} {:>9.1} KiB compressed              {:>9.1} KiB raw",
        "TOTAL (column chunks only)",
        tot as f64 / 1024.0,
        totu as f64 / 1024.0
    );
    println!(
        "  file size {:.1} KiB -> parquet overhead (footer/metadata) {:.1} KiB ({:.1}%)",
        std::fs::metadata(&files[0]).unwrap().len() as f64 / 1024.0,
        (std::fs::metadata(&files[0]).unwrap().len() as i64 - tot) as f64 / 1024.0,
        (std::fs::metadata(&files[0]).unwrap().len() as i64 - tot) as f64
            / std::fs::metadata(&files[0]).unwrap().len() as f64
            * 100.0
    );

    // Concat + lexsort exactly like compact_segment.
    let merged = concat_batches(&schema, &batches).unwrap();
    drop(batches);
    let sort_columns: Vec<SortColumn> = keys
        .iter()
        .map(|k| SortColumn {
            values: merged.column_by_name(k).expect(k).clone(),
            options: None,
        })
        .collect();
    let idx = lexsort_to_indices(&sort_columns, None).unwrap();
    let sorted = take_record_batch(&merged, &idx).unwrap();
    drop(merged);

    println!("\nRE-ENCODED AS ONE FILE (sorted by {ka}):");
    write_variant(
        "current props (zstd 1, rg 128k)",
        &sorted,
        props(&schema, 1, 131_072, true, true),
        rows,
        in_bytes,
    );
    write_variant(
        "zstd 3",
        &sorted,
        props(&schema, 3, 131_072, true, true),
        rows,
        in_bytes,
    );
    write_variant(
        "zstd 6",
        &sorted,
        props(&schema, 6, 131_072, true, true),
        rows,
        in_bytes,
    );
    write_variant(
        "zstd 9",
        &sorted,
        props(&schema, 9, 131_072, true, true),
        rows,
        in_bytes,
    );
    write_variant(
        "zstd 1, rg 1M",
        &sorted,
        props(&schema, 1, 1_048_576, true, true),
        rows,
        in_bytes,
    );
    write_variant(
        "zstd 6, rg 1M",
        &sorted,
        props(&schema, 6, 1_048_576, true, true),
        rows,
        in_bytes,
    );
    write_variant(
        "zstd 12, rg 1M",
        &sorted,
        props(&schema, 12, 1_048_576, true, true),
        rows,
        in_bytes,
    );

    // Where do the bytes go once merged?
    let tmp = std::env::temp_dir().join("photon_probe_merged.parquet");
    let f = File::create(&tmp).unwrap();
    let mut w = ArrowWriter::try_new(
        f,
        sorted.schema(),
        Some(props(&schema, 6, 1_048_576, true, true)),
    )
    .unwrap();
    w.write(&sorted).unwrap();
    w.close().unwrap();
    println!("\nPER-COLUMN FOOTPRINT of the merged file (zstd 6, rg 1M):");
    let fp = column_footprint(&tmp);
    let tot: i64 = fp.iter().map(|x| x.1).sum();
    let totu: i64 = fp.iter().map(|x| x.2).sum();
    let mut fp2 = fp.clone();
    fp2.sort_by_key(|x| -x.1);
    for (name, c, u) in fp2.iter().take(16) {
        println!(
            "  {:<46} {:>9.1} KiB compressed ({:>5.1}%)   {:>9.1} KiB raw ({:>5.1}%)   ratio {:.1}x",
            name, *c as f64 / 1024.0, *c as f64 / tot as f64 * 100.0,
            *u as f64 / 1024.0, *u as f64 / totu as f64 * 100.0, *u as f64 / (*c).max(1) as f64
        );
    }
    println!(
        "  {:<46} {:>9.1} KiB compressed              {:>9.1} KiB raw",
        "TOTAL",
        tot as f64 / 1024.0,
        totu as f64 / 1024.0
    );
    let _ = std::fs::remove_file(&tmp);
}

//! Dev-only: what the small-file pile costs the READ path.
//!   cargo run --release -p photon-compact --example read_cost -- <hot_dir> <manifest.json> <parquet_dir> <n>
use std::fs::File;
use std::path::PathBuf;
use std::time::Instant;

use parquet::file::reader::{FileReader, SerializedFileReader};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let manifest = PathBuf::from(&a[1]);
    let dir = PathBuf::from(&a[2]);
    let n: usize = a[3].parse().unwrap();

    let t = Instant::now();
    let bytes = std::fs::read(&manifest).unwrap();
    let read_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let parse_ms = t.elapsed().as_secs_f64() * 1000.0;
    let entries = v["entries"].as_array().unwrap().len();
    println!(
        "manifest {:.1} MiB / {} entries: read {:.0} ms, serde_json parse {:.0} ms",
        bytes.len() as f64 / 1048576.0,
        entries,
        read_ms,
        parse_ms
    );

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "parquet").unwrap_or(false))
        .collect();
    files.sort();
    let files: Vec<PathBuf> = files.into_iter().take(n).collect();

    // Cold-ish: we cannot drop the page cache without root, so report warm numbers — the
    // per-file constant (open + footer decode) is what matters, and it is CPU-bound.
    for _ in 0..2 {
        let t = Instant::now();
        let mut rgs = 0usize;
        let mut rows = 0i64;
        for p in &files {
            let f = File::open(p).unwrap();
            let r = SerializedFileReader::new(f).unwrap();
            let md = r.metadata();
            rgs += md.num_row_groups();
            rows += md.file_metadata().num_rows();
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "open+footer of {} files ({} row groups, {} rows): {:.0} ms  = {:.2} ms/file",
            files.len(),
            rgs,
            rows,
            ms,
            ms / files.len() as f64
        );
    }
    // Same for the .idx skip-index sidecars the pruner reads before any Parquet.
    let idx: Vec<PathBuf> = files
        .iter()
        .map(|p| p.with_extension("idx"))
        .filter(|p| p.exists())
        .collect();
    let t = Instant::now();
    let mut b = 0u64;
    for p in &idx {
        b += std::fs::read(p).unwrap().len() as u64;
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    // DECODE CHECK: fully decode the oldest N files — a single corrupt input would wedge
    // merge_once forever, since it always re-selects the OLDEST small files.
    let mut bad = 0;
    for p in &files {
        let f = File::open(p).unwrap();
        match parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f) {
            Err(e) => {
                bad += 1;
                println!("  CORRUPT (open) {}: {e}", p.display());
            }
            Ok(b) => {
                let r = b.build().unwrap();
                for batch in r {
                    if let Err(e) = batch {
                        bad += 1;
                        println!("  CORRUPT (decode) {}: {e}", p.display());
                        break;
                    }
                }
            }
        }
    }
    println!("decode check: {} of {} files unreadable", bad, files.len());

    println!(
        "read {} .idx sidecars ({:.1} MiB): {:.0} ms = {:.2} ms/file",
        idx.len(),
        b as f64 / 1048576.0,
        ms,
        ms / idx.len().max(1) as f64
    );
}

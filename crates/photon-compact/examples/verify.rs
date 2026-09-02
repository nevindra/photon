//! Dev-only: check a recompacted tier is intact and that each transform actually landed.
//!   verify <tier> [signal_dir]
use std::fs::File;
use std::path::PathBuf;

use arrow::array::{Array, MapArray, StringArray, TimestampNanosecondArray};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let tier = PathBuf::from(&a[1]);
    for dir in ["data", "data-spans", "data-metrics"] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(tier.join(dir))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|e| e == "parquet").unwrap_or(false))
            .collect();
        files.sort();
        let (mut rows, mut bad, mut over_cap, mut max_len, mut ns_ts) =
            (0u64, 0u32, 0u64, 0usize, 0u64);
        let (mut tenant_col_rows, mut tenant_col_nonnull, mut tenant_in_map) = (0u64, 0u64, 0u64);
        for p in &files {
            let f = File::open(p).unwrap();
            let rdr = match ParquetRecordBatchReaderBuilder::try_new(f) {
                Ok(b) => b.build().unwrap(),
                Err(e) => {
                    bad += 1;
                    eprintln!("  UNREADABLE {}: {e}", p.display());
                    continue;
                }
            };
            for b in rdr {
                let Ok(b) = b else {
                    bad += 1;
                    break;
                };
                rows += b.num_rows() as u64;
                if let Some(c) = b.column_by_name("tenant") {
                    tenant_col_rows += c.len() as u64;
                    tenant_col_nonnull += (c.len() - c.null_count()) as u64;
                }
                if let Some(t) = b
                    .column_by_name("timestamp")
                    .and_then(|c| c.as_any().downcast_ref::<TimestampNanosecondArray>())
                {
                    for i in 0..t.len() {
                        if !t.is_null(i) && t.value(i) % 1_000_000 != 0 {
                            ns_ts += 1;
                        }
                    }
                }
                if let Some(m) = b
                    .column_by_name("attributes")
                    .and_then(|c| c.as_any().downcast_ref::<MapArray>())
                {
                    let k = m.keys().as_any().downcast_ref::<StringArray>().unwrap();
                    let v = m.values().as_any().downcast_ref::<StringArray>().unwrap();
                    for i in 0..v.len() {
                        if k.value(i) == "tenant" {
                            tenant_in_map += 1;
                        }
                        if v.is_null(i) {
                            continue;
                        }
                        let l = v.value(i).len();
                        max_len = max_len.max(l);
                        if l > 512 {
                            over_cap += 1;
                        }
                    }
                }
            }
        }
        println!(
            "{dir:<14} {files:>4} files  {rows:>12} rows  unreadable={bad}  attr values >512B={over_cap} (longest {max_len})  tenant col: {tenant_col_nonnull}/{tenant_col_rows} filled, {tenant_in_map} still in map  ns-precision timestamps={ns_ts}",
            files = files.len()
        );
    }
}

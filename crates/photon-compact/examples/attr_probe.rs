//! Dev-only: which attribute keys actually cost bytes, and how redundant the time columns are.
//!
//!   cargo run --release -p photon-compact --example attr_probe -- <dir> <n_files>
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

use arrow::array::{Array, MapArray, StringArray, TimestampNanosecondArray};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&a[1]);
    let n: usize = a[2].parse().unwrap();

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "parquet").unwrap_or(false))
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("seg-"))
        .collect();
    files.sort();

    // key -> (occurrences, total value bytes, distinct values seen (capped sample))
    let mut stats: HashMap<String, (u64, u64, std::collections::HashSet<String>)> = HashMap::new();
    let mut rows = 0u64;
    let mut key_bytes = 0u64;
    let mut ts_eq = 0u64;
    let mut ts_seen = 0u64;
    let mut ts_delta_ms: Vec<i64> = Vec::new();

    let sel: Vec<&PathBuf> = if std::env::var("NEWEST").is_ok() {
        files.iter().rev().take(n).collect()
    } else {
        files.iter().take(n).collect()
    };
    for p in sel {
        let f = File::open(p).unwrap();
        let rdr = ParquetRecordBatchReaderBuilder::try_new(f)
            .unwrap()
            .build()
            .unwrap();
        for b in rdr {
            let b = b.unwrap();
            rows += b.num_rows() as u64;
            if let (Some(t), Some(o)) = (
                b.column_by_name("timestamp"),
                b.column_by_name("observed_timestamp"),
            ) {
                let t = t
                    .as_any()
                    .downcast_ref::<TimestampNanosecondArray>()
                    .unwrap();
                let o = o
                    .as_any()
                    .downcast_ref::<TimestampNanosecondArray>()
                    .unwrap();
                for i in 0..t.len() {
                    if t.is_null(i) || o.is_null(i) {
                        continue;
                    }
                    ts_seen += 1;
                    let d = o.value(i) - t.value(i);
                    if d == 0 {
                        ts_eq += 1;
                    }
                    if ts_delta_ms.len() < 2_000_000 {
                        ts_delta_ms.push(d / 1_000_000);
                    }
                }
            }
            let Some(m) = b.column_by_name("attributes") else {
                continue;
            };
            let m = m.as_any().downcast_ref::<MapArray>().unwrap();
            let keys = m.keys().as_any().downcast_ref::<StringArray>().unwrap();
            let vals = m.values().as_any().downcast_ref::<StringArray>().unwrap();
            for i in 0..keys.len() {
                let k = keys.value(i);
                key_bytes += k.len() as u64;
                let v = if vals.is_null(i) { "" } else { vals.value(i) };
                let e = stats
                    .entry(k.to_string())
                    .or_insert((0, 0, Default::default()));
                e.0 += 1;
                e.1 += v.len() as u64;
                if e.2.len() < 5000 {
                    e.2.insert(v.to_string());
                }
            }
        }
    }

    let total_v: u64 = stats.values().map(|s| s.1).sum();
    let total_n: u64 = stats.values().map(|s| s.0).sum();
    println!(
        "{} rows, {} map entries ({:.1} per row), raw value bytes {:.1} MiB, raw key bytes {:.1} MiB, {} distinct keys",
        rows, total_n, total_n as f64 / rows as f64,
        total_v as f64 / 1048576.0, key_bytes as f64 / 1048576.0, stats.len()
    );
    let mut v: Vec<_> = stats.into_iter().collect();
    v.sort_by_key(|(_, s)| std::cmp::Reverse(s.1));
    println!(
        "\n{:<42} {:>7} {:>10} {:>8} {:>10} {:>9}",
        "attribute key", "% bytes", "MiB(raw)", "avg len", "rows w/ it", "distinct*"
    );
    let mut cum = 0.0;
    for (k, s) in v.iter().take(30) {
        let pct = s.1 as f64 / total_v as f64 * 100.0;
        cum += pct;
        println!(
            "{:<42} {:>6.1}% {:>10.2} {:>8.0} {:>9.0}% {:>9} (cum {:.0}%)",
            k,
            pct,
            s.1 as f64 / 1048576.0,
            s.1 as f64 / s.0.max(1) as f64,
            s.0 as f64 / rows as f64 * 100.0,
            if s.2.len() >= 5000 {
                ">5000".to_string()
            } else {
                s.2.len().to_string()
            },
            cum
        );
    }

    if ts_seen > 0 {
        ts_delta_ms.sort_unstable();
        let p = |q: f64| ts_delta_ms[((ts_delta_ms.len() as f64 - 1.0) * q) as usize];
        println!(
            "\nobserved_timestamp - timestamp: identical on {:.1}% of {} rows; delta ms p50={} p90={} p99={} max={}",
            ts_eq as f64 / ts_seen as f64 * 100.0, ts_seen, p(0.5), p(0.9), p(0.99), ts_delta_ms.last().unwrap()
        );
    }
}

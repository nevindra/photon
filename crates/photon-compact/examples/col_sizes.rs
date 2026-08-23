//! Scratch: per-column compressed/uncompressed bytes for a Parquet file, so we can see which
//! column dominates a scan. Delete after use.
use std::fs::File;

use parquet::file::reader::{FileReader, SerializedFileReader};

fn main() {
    for path in std::env::args().skip(1) {
        let f = File::open(&path).expect("open");
        let r = SerializedFileReader::new(f).expect("parquet");
        let meta = r.metadata();
        let n_rows = meta.file_metadata().num_rows();
        let schema = meta.file_metadata().schema_descr();

        let mut comp = vec![0i64; schema.num_columns()];
        let mut uncomp = vec![0i64; schema.num_columns()];
        for rg in 0..meta.num_row_groups() {
            let g = meta.row_group(rg);
            for c in 0..g.num_columns() {
                comp[c] += g.column(c).compressed_size();
                uncomp[c] += g.column(c).uncompressed_size();
            }
        }
        let total: i64 = comp.iter().sum();
        println!(
            "== {path}  rows={n_rows}  total={:.1} MB",
            total as f64 / 1e6
        );
        let mut rows: Vec<_> = (0..schema.num_columns())
            .map(|c| (schema.column(c).path().string(), comp[c], uncomp[c]))
            .collect();
        rows.sort_by_key(|r| -r.1);
        for (name, c, u) in rows {
            println!(
                "   {:<44} {:>9.2} MB comp  {:>9.2} MB raw  {:>5.1}%",
                name,
                c as f64 / 1e6,
                u as f64 / 1e6,
                100.0 * c as f64 / total as f64
            );
        }
        println!();
    }
}

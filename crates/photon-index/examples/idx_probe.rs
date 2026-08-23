//! Read-only probe: what does a skip-index sidecar say about a token?
//!
//! Pruning can only ever add work, never drop a real result — so when a query returns nothing in
//! three milliseconds over files that demonstrably hold the rows, the sidecar is the first thing
//! to ask. This decodes one and answers `might_contain_token` for each token given.
//!
//! ```sh
//! cargo run --release -p photon-index --example idx_probe -- <file.idx> <token>...
//! ```

use photon_index::SkipIndex;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("path to a .idx sidecar");
    let tokens: Vec<String> = args.collect();
    let bytes = std::fs::read(&path)?;
    let index = SkipIndex::from_bytes(&bytes)?;
    println!("{path} ({} bytes)", bytes.len());
    println!("  host_range: {:?}", index.host_range());
    for t in &tokens {
        println!(
            "  might_contain_token({t:?}) = {}",
            index.might_contain_token(t)
        );
    }
    Ok(())
}

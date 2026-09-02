//! Process-wide cap on how long ONE attribute value may be.
//!
//! Applied where OTLP is mapped into Arrow, which is the single choke point every ingest door
//! funnels through (OTLP HTTP + gRPC for all three signals, the RUM beacon, Prometheus
//! remote-write). Capping *there* and not later is the whole point: an oversized value is cut
//! before it reaches the WAL, so it costs nothing to `fsync`, nothing to compact, nothing to
//! replicate, and nothing to decode on every later query.
//!
//! Measured on a production span corpus (182k spans, one merged Parquet file, zstd 6), capping
//! **every** attribute value:
//!
//! | cap    | file size |
//! |--------|-----------|
//! | 8 KiB  |    −0.2%  |
//! | 4 KiB  |    −2.9%  |
//! | 2 KiB  |   −10.8%  |
//! | 1 KiB  |   −20.6%  |
//! | 512 B  |   −30.8%  |
//!
//! One key carried nearly all of it: `db.query.text`, averaging 1,618 bytes and present on 16% of
//! spans. The same corpus also had a single log attribute averaging 52,133 bytes, which is the
//! shape this exists to stop.
//!
//! **Truncation is not reversible**, which is why this is a config knob and not a constant: an
//! operator who needs whole SQL statements raises it, one who does not takes the 30%.

use std::sync::atomic::{AtomicUsize, Ordering};

/// `0` means no cap. Set once at startup from `[ingest].max_attribute_value_bytes`; read by each
/// batch builder at construction, so a builder's limit never changes underneath it mid-batch.
static MAX_ATTRIBUTE_VALUE_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Install the process-wide cap. `photon-server` calls this once from config before any receiver
/// is mounted; everything else (tests, benches, the offline tools) leaves it at "no cap" and opts
/// in per builder instead.
pub fn set_max_attribute_value_bytes(bytes: usize) {
    MAX_ATTRIBUTE_VALUE_BYTES.store(bytes, Ordering::Relaxed);
}

/// The current cap, `0` meaning unlimited.
pub fn max_attribute_value_bytes() -> usize {
    MAX_ATTRIBUTE_VALUE_BYTES.load(Ordering::Relaxed)
}

/// `v` shortened to at most `max` **bytes**, never splitting a UTF-8 character. `max == 0` is
/// "no cap" and returns `v` untouched.
///
/// The cap is in bytes rather than characters because bytes are what the WAL, the Parquet file
/// and the page cache actually pay for; a multi-byte character straddling the boundary is dropped
/// whole rather than corrupted.
pub fn cap_value(v: &str, max: usize) -> &str {
    if max == 0 || v.len() <= max {
        return v;
    }
    let mut end = max;
    while end > 0 && !v.is_char_boundary(end) {
        end -= 1;
    }
    &v[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_means_no_cap() {
        assert_eq!(cap_value("hello", 0), "hello");
    }

    #[test]
    fn a_short_value_is_untouched() {
        assert_eq!(cap_value("hello", 16), "hello");
        assert_eq!(cap_value("hello", 5), "hello");
    }

    #[test]
    fn a_long_value_is_cut_to_the_cap() {
        assert_eq!(cap_value("hello world", 5), "hello");
    }

    #[test]
    fn a_multibyte_character_is_dropped_whole_rather_than_split() {
        // "héllo": h=1, é=2, l=1, l=1, o=1 — a cap of 2 lands inside `é`.
        let v = "héllo";
        assert_eq!(cap_value(v, 2), "h");
        assert_eq!(cap_value(v, 3), "hé");
        // Every result is still valid UTF-8 for every cap, which is what the Arrow builder needs.
        for max in 0..=v.len() + 2 {
            let out = cap_value(v, max);
            assert!(v.starts_with(out));
        }
    }

    #[test]
    fn the_global_default_is_no_cap() {
        // Nothing in the library sets it; only `photon-server` does, from config.
        assert_eq!(cap_value("x", max_attribute_value_bytes()), "x");
    }
}

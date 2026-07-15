//! Hand-rolled bloom filter — no external bloom crate. A bit vector plus `k` hash probes
//! via double hashing (Kirsch-Mitzenmacher): one 64-bit `DefaultHasher` output is split
//! into two 32-bit halves `(h1, h2)`, and probe `i` lands at `h1 + i * h2 (mod m)`.

use photon_core::PhotonError;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Target false-positive rate used to size the bit array from the distinct-item count.
const TARGET_FALSE_POSITIVE_RATE: f64 = 0.01;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Bloom {
    /// Bit-packed membership bits, 8 bits per byte, little-endian within a byte.
    bits: Vec<u8>,
    num_bits: usize,
    num_hashes: u32,
}

impl Bloom {
    /// Size a filter for `expected_items` distinct entries at ~1% false-positive rate.
    /// Standard bloom-filter sizing:
    ///   m = ceil(-(n * ln(p)) / ln(2)^2)
    ///   k = round((m / n) * ln(2))
    pub(crate) fn new(expected_items: usize) -> Bloom {
        let n = expected_items.max(1) as f64;
        let ln2 = std::f64::consts::LN_2;

        let num_bits = (-(n * TARGET_FALSE_POSITIVE_RATE.ln()) / (ln2 * ln2))
            .ceil()
            .max(8.0) as usize;
        let num_hashes = ((num_bits as f64 / n) * ln2).round().clamp(1.0, 32.0) as u32;

        Bloom {
            bits: vec![0u8; num_bits.div_ceil(8)],
            num_bits,
            num_hashes,
        }
    }

    pub(crate) fn insert(&mut self, item: &str) {
        let (h1, h2) = Self::hash_pair(item);
        for i in 0..self.num_hashes {
            let idx = self.index_for(h1, h2, i);
            self.bits[idx / 8] |= 1 << (idx % 8);
        }
    }

    /// false = item DEFINITELY absent; true = possibly present.
    pub(crate) fn might_contain(&self, item: &str) -> bool {
        let (h1, h2) = Self::hash_pair(item);
        (0..self.num_hashes).all(|i| {
            let idx = self.index_for(h1, h2, i);
            (self.bits[idx / 8] >> (idx % 8)) & 1 == 1
        })
    }

    /// Raw parts for the binary `.idx` encoder: `(num_bits, num_hashes, packed bit vector)`.
    /// Read-only view; reconstruct via `from_raw_parts`.
    pub(crate) fn raw_parts(&self) -> (usize, u32, &[u8]) {
        (self.num_bits, self.num_hashes, &self.bits)
    }

    /// Reconstruct a `Bloom` from parts previously produced by `raw_parts` (used by the
    /// binary `.idx` decoder). No validation here: every caller MUST call `validate()` on the
    /// result before using it, so the `% num_bits` in `index_for` and the `bits[idx / 8]` in
    /// `might_contain` can never divide-by-zero or index out of bounds on a decoded bloom.
    pub(crate) fn from_raw_parts(num_bits: usize, num_hashes: u32, bits: Vec<u8>) -> Bloom {
        Bloom {
            bits,
            num_bits,
            num_hashes,
        }
    }

    /// Validate the framing invariants a decoded bloom must satisfy before any membership
    /// probe touches it: `num_bits` must be non-zero (it's a `%` divisor in `index_for`), and
    /// `bits` must be exactly the byte length `num_bits` implies (it's indexed as `bits[idx / 8]`
    /// in `might_contain`). Both the binary `.idx` decoder and the legacy `serde_json` decoder
    /// deserialize a `Bloom`'s fields from untrusted bytes, so BOTH must call this before
    /// returning a `SkipIndex` — a bloom that fails this check would otherwise panic (divide-by-
    /// zero or out-of-bounds index) on the first query-time membership probe instead of
    /// surfacing as a clean decode error.
    pub(crate) fn validate(&self) -> Result<(), PhotonError> {
        if self.num_bits == 0 || self.bits.len() != self.num_bits.div_ceil(8) {
            return Err(PhotonError::Index(format!(
                "skip index: corrupt bloom framing (num_bits={}, bits_len={})",
                self.num_bits,
                self.bits.len()
            )));
        }
        Ok(())
    }

    fn index_for(&self, h1: u32, h2: u32, i: u32) -> usize {
        // Force the step odd (hence non-zero) so successive probes don't collapse onto
        // the same bucket when h2 happens to hash to 0.
        let step = (h2 | 1) as u64;
        ((h1 as u64).wrapping_add((i as u64).wrapping_mul(step)) % self.num_bits as u64) as usize
    }

    fn hash_pair(item: &str) -> (u32, u32) {
        let mut hasher = DefaultHasher::new();
        item.hash(&mut hasher);
        let h = hasher.finish();
        ((h >> 32) as u32, h as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserted_items_are_always_reported_present() {
        let mut bloom = Bloom::new(5);
        for item in ["alpha", "beta", "gamma", "delta", "epsilon"] {
            bloom.insert(item);
        }
        for item in ["alpha", "beta", "gamma", "delta", "epsilon"] {
            assert!(bloom.might_contain(item));
        }
    }

    #[test]
    fn nothing_inserted_means_nothing_reported_present() {
        let bloom = Bloom::new(1);
        assert!(!bloom.might_contain("anything"));
    }

    #[test]
    fn sizing_grows_with_expected_item_count() {
        let small = Bloom::new(1);
        let large = Bloom::new(10_000);
        assert!(large.num_bits > small.num_bits);
    }
}

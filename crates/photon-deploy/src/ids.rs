//! Identifiers and digests. Ids are short and prefixed; tokens are OS-random (uuid v4 draws
//! from the OS RNG); the payload digest is what the audit log keeps instead of the payload
//! (R5.9).

use sha2::{Digest, Sha256};

/// A random id with a short prefix (`st_…`, `ro_…`, `ac_…`, `tv_…`): 16 hex chars of OS entropy.
pub fn new_id(prefix: &str) -> String {
    let u = uuid::Uuid::new_v4();
    let s = u.simple().to_string();
    format!("{prefix}_{}", &s[..16])
}

/// The per-host agent token (R1.4): `ak_` + 32 hex chars of OS entropy. Shown once; only its
/// hash is stored.
pub fn mint_token() -> String {
    format!("ak_{}", uuid::Uuid::new_v4().simple())
}

/// sha256 hex — how tokens are stored and how payloads are summarized for the audit log.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// `sha256:` + the first 12 hex chars: the short form the UI shows beside an audit row.
pub fn payload_digest(bytes: &[u8]) -> String {
    let full = sha256_hex(bytes);
    format!("sha256:{}", &full[..12])
}

/// Constant-time equality for two digests of equal length.
pub fn digest_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_prefixed() {
        let a = new_id("st");
        let b = new_id("st");
        assert!(a.starts_with("st_") && a.len() == 3 + 16);
        assert_ne!(a, b);
        let t = mint_token();
        assert!(t.starts_with("ak_") && t.len() == 3 + 32, "{t}");
    }

    #[test]
    fn digests() {
        assert_eq!(sha256_hex(b"").len(), 64);
        assert!(payload_digest(b"x").starts_with("sha256:"));
        assert!(digest_eq("abc", "abc"));
        assert!(!digest_eq("abc", "abd"));
        assert!(!digest_eq("ab", "abc"));
    }
}

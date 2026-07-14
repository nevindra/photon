use crate::PhotonError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SegmentId(pub u64);

/// Merged compaction outputs use the top bit of the id space so their object path and
/// manifest key can never collide with a WAL-allocated segment id (which are small and
/// sequential). See doc-04 / the B2 merge-id-collision fix.
pub const MERGE_ID_BIT: u64 = 1 << 63;

impl SegmentId {
    pub fn next(self) -> SegmentId {
        SegmentId(self.0 + 1)
    }

    /// True when this id was allocated from the merged (high-bit) namespace rather than by the
    /// WAL. Merged ids never enter the WAL, so their `.wal` filename is never materialized.
    pub fn is_merged(self) -> bool {
        self.0 & MERGE_ID_BIT != 0
    }

    /// The first merged id (used when no merged segment exists yet).
    pub fn first_merged() -> SegmentId {
        SegmentId(MERGE_ID_BIT)
    }

    pub fn filename(self) -> String {
        format!("seg-{:016x}.wal", self.0)
    }

    pub fn parse_filename(name: &str) -> Result<SegmentId, PhotonError> {
        let hex = name
            .strip_prefix("seg-")
            .and_then(|s| s.strip_suffix(".wal"))
            .ok_or_else(|| PhotonError::Config(format!("not a segment filename: {name}")))?;
        let n = u64::from_str_radix(hex, 16)
            .map_err(|e| PhotonError::Config(format!("bad segment id in {name}: {e}")))?;
        Ok(SegmentId(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_and_increments() {
        let a = SegmentId(1);
        let b = a.next();
        assert_eq!(b, SegmentId(2));
        assert!(a < b);
    }

    #[test]
    fn filename_roundtrips_and_zero_pads() {
        let id = SegmentId(255);
        assert_eq!(id.filename(), "seg-00000000000000ff.wal");
        assert_eq!(
            SegmentId::parse_filename("seg-00000000000000ff.wal").unwrap(),
            id
        );
    }

    #[test]
    fn rejects_bad_filename() {
        assert!(SegmentId::parse_filename("nope.txt").is_err());
    }

    #[test]
    fn merged_namespace_is_disjoint_from_wal_ids() {
        // WAL ids are small and sequential — never the top bit.
        assert!(!SegmentId(0).is_merged());
        assert!(!SegmentId(2).is_merged());
        assert!(!SegmentId(u64::MAX >> 1).is_merged());

        // Merged ids carry the top bit and are strictly greater than any WAL id.
        let first = SegmentId::first_merged();
        assert!(first.is_merged());
        assert_eq!(first, SegmentId(MERGE_ID_BIT));
        assert!(first > SegmentId(u64::MAX >> 1));
        // Merged ids stay in the high half as they increment.
        assert!(first.next().is_merged());
        assert!(first.next() > first);
    }
}

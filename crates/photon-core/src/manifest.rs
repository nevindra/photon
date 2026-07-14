use crate::segment::SegmentId;
use crate::PhotonError;
use serde::{Deserialize, Serialize};

/// Object path (in the hot store) where the serialized manifest lives. The compactor is
/// the sole writer; the query engine reads it. Both go through `Manifest::to_json`/`from_json`.
pub const MANIFEST_OBJECT_PATH: &str = "manifest/manifest.json";

/// Object path for the spans manifest — separate object from the logs manifest so the two
/// signals never write-race. Sole writer: the spans compactor.
pub const SPANS_MANIFEST_OBJECT_PATH: &str = "manifest/spans-manifest.json";

/// Metrics manifest — a separate object from the logs and spans manifests so the three
/// signals never write-race.
pub const METRICS_MANIFEST_OBJECT_PATH: &str = "manifest/metrics-manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub path: String,
    pub segment_id: SegmentId,
    pub min_ts_nanos: i64,
    pub max_ts_nanos: i64,
    pub min_service: String,
    pub max_service: String,
    pub row_count: u64,
    pub durable: bool,
    /// Sorted, deduped union of long-tail (non-promoted) attribute keys observed in this
    /// segment. Enables `/api/fields` to be metadata, not a scan. Empty for legacy segments.
    #[serde(default)]
    pub attribute_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    entries: Vec<FileEntry>,
}

impl Manifest {
    pub fn new() -> Manifest {
        Manifest {
            entries: Vec::new(),
        }
    }

    /// Insert `entry`, or replace an existing entry with the same `segment_id`. `segment_id`
    /// is the natural idempotency key (compaction reuses ids on retry), so this makes the
    /// WAL→Parquet handoff crash-idempotent: a re-run after a crash/`remove_segment` error
    /// updates the entry in place instead of duplicating it (doc-04 Finding 1, P0).
    pub fn add(&mut self, entry: FileEntry) {
        match self
            .entries
            .iter_mut()
            .find(|e| e.segment_id == entry.segment_id)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }

    /// All entries, in insertion order.
    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    /// Replace all entries (used by the compactor's merge pass).
    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
    }

    pub fn candidates(&self, start_ts_nanos: i64, end_ts_nanos: i64) -> Vec<&FileEntry> {
        self.entries
            .iter()
            .filter(|e| e.min_ts_nanos <= end_ts_nanos && e.max_ts_nanos >= start_ts_nanos)
            .collect()
    }

    pub fn to_json(&self) -> Result<String, PhotonError> {
        serde_json::to_string(self).map_err(|e| PhotonError::Serde(e.to_string()))
    }

    pub fn from_json(s: &str) -> Result<Manifest, PhotonError> {
        serde_json::from_str(s).map_err(|e| PhotonError::Serde(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, seg: u64, min_ts: i64, max_ts: i64) -> FileEntry {
        FileEntry {
            path: path.into(),
            segment_id: SegmentId(seg),
            min_ts_nanos: min_ts,
            max_ts_nanos: max_ts,
            min_service: "api".into(),
            max_service: "web".into(),
            row_count: 10,
            durable: false,
            attribute_keys: Vec::new(),
        }
    }

    #[test]
    fn candidates_returns_only_time_overlapping() {
        let mut m = Manifest::new();
        m.add(entry("a.parquet", 1, 0, 100));
        m.add(entry("b.parquet", 2, 200, 300));
        m.add(entry("c.parquet", 3, 90, 210));

        let hits: Vec<_> = m
            .candidates(95, 105)
            .iter()
            .map(|e| e.path.clone())
            .collect();
        assert_eq!(hits, vec!["a.parquet".to_string(), "c.parquet".to_string()]);
    }

    #[test]
    fn json_roundtrips() {
        let mut m = Manifest::new();
        m.add(entry("a.parquet", 1, 0, 100));
        let json = m.to_json().unwrap();
        let back = Manifest::from_json(&json).unwrap();
        assert_eq!(back.candidates(0, 100).len(), 1);
    }

    #[test]
    fn attribute_keys_default_and_roundtrip() {
        // Legacy JSON with no attribute_keys still loads (serde default → empty).
        let legacy = r#"{"entries":[{"path":"a.parquet","segment_id":1,"min_ts_nanos":0,
"max_ts_nanos":100,"min_service":"api","max_service":"web","row_count":10,"durable":false}]}"#;
        let m = Manifest::from_json(legacy).unwrap();
        assert!(m.candidates(0, 100)[0].attribute_keys.is_empty());

        // New field round-trips.
        let mut m2 = Manifest::new();
        let mut e = entry("b.parquet", 2, 0, 100);
        e.attribute_keys = vec!["host.name".into(), "region".into()];
        m2.add(e);
        let back = Manifest::from_json(&m2.to_json().unwrap()).unwrap();
        assert_eq!(
            back.candidates(0, 100)[0].attribute_keys,
            vec!["host.name", "region"]
        );
    }

    #[test]
    fn add_is_idempotent_by_segment_id() {
        let mut m = Manifest::new();
        m.add(entry("a.parquet", 1, 0, 100));
        m.add(entry("a.parquet", 1, 0, 100)); // retry of the same segment
        assert_eq!(
            m.candidates(i64::MIN, i64::MAX).len(),
            1,
            "same segment_id must not duplicate"
        );

        // A new entry for the same id replaces the old (e.g. re-encoded file).
        let mut updated = entry("a2.parquet", 1, 0, 200);
        updated.row_count = 99;
        m.add(updated);
        let all = m.candidates(i64::MIN, i64::MAX);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].row_count, 99);
        assert_eq!(all[0].path, "a2.parquet");
    }
}

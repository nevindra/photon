//! `fields`: the field catalog for a time window — fixed columns + configured promoted
//! attributes + the union of long-tail attribute keys recorded per segment in the manifest.
//! Metadata only: it reads the manifest, never the Parquet data.
use std::collections::BTreeSet;

use photon_core::schema;
use photon_core::PhotonError;

use crate::QueryEngine;

/// How a field is stored, so the UI can hint its type/behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Fixed,
    Promoted,
    Attribute,
}

/// One catalog entry: field name + how it is stored.
pub struct FieldInfo {
    pub name: String,
    pub kind: FieldKind,
}

impl QueryEngine {
    /// Every field that could appear in the `[start, end]` window.
    pub fn fields(
        &self,
        start_ts_nanos: i64,
        end_ts_nanos: i64,
    ) -> Result<Vec<FieldInfo>, PhotonError> {
        let manifest = self.load_manifest()?;
        let mut attrs: BTreeSet<String> = BTreeSet::new();
        for entry in manifest.candidates(start_ts_nanos, end_ts_nanos) {
            for k in &entry.attribute_keys {
                attrs.insert(k.clone());
            }
        }

        let mut out = Vec::new();
        for name in schema::FIXED_COLUMNS {
            out.push(FieldInfo {
                name: (*name).to_string(),
                kind: FieldKind::Fixed,
            });
        }
        for name in self.promoted_attributes() {
            out.push(FieldInfo {
                name: name.clone(),
                kind: FieldKind::Promoted,
            });
            // A promoted attribute is its own column, not a long-tail Map key — never double-count.
            attrs.remove(name);
        }
        for name in attrs {
            out.push(FieldInfo {
                name,
                kind: FieldKind::Attribute,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photon_core::manifest::{FileEntry, Manifest};
    use photon_core::schema::LogSchema;
    use photon_core::segment::SegmentId;

    fn entry(seg: u64, min: i64, max: i64, keys: &[&str]) -> FileEntry {
        FileEntry {
            path: format!("seg-{seg}.parquet"),
            segment_id: SegmentId(seg),
            min_ts_nanos: min,
            max_ts_nanos: max,
            min_service: "api".into(),
            max_service: "web".into(),
            row_count: 1,
            durable: false,
            attribute_keys: keys.iter().map(|s| s.to_string()).collect(),
            bytes: 0,
        }
    }

    #[test]
    fn unions_attribute_keys_over_window_plus_fixed_and_promoted() {
        let tmp = tempfile::tempdir().unwrap();
        let mdir = tmp.path().join("manifest");
        std::fs::create_dir_all(&mdir).unwrap();
        let mut m = Manifest::new();
        m.add(entry(1, 0, 100, &["region"]));
        m.add(entry(2, 50, 150, &["tier"]));
        m.add(entry(3, 1000, 2000, &["only_outside"])); // outside the query window
        std::fs::write(mdir.join("manifest.json"), m.to_json().unwrap()).unwrap();

        let schema = LogSchema::new(&["service.name".into(), "host.name".into()]);
        let engine = QueryEngine::new(tmp.path().to_path_buf(), schema).unwrap();
        let fields = engine.fields(0, 200).unwrap();

        let has =
            |name: &str, kind: FieldKind| fields.iter().any(|f| f.name == name && f.kind == kind);
        assert!(has("region", FieldKind::Attribute));
        assert!(has("tier", FieldKind::Attribute));
        assert!(!fields.iter().any(|f| f.name == "only_outside")); // pruned by window
        assert!(has("service.name", FieldKind::Promoted));
        assert!(has("host.name", FieldKind::Promoted));
        assert!(has("body", FieldKind::Fixed));
        assert!(has("severity_text", FieldKind::Fixed));
    }
}

//! `fields` for spans: the field catalog for a time window — fixed columns + configured promoted
//! attributes + the union of long-tail attribute keys recorded per segment in the spans manifest.
//! Metadata only: it reads the spans manifest, never the Parquet data. Mirrors `crate::fields`
//! (the logs field catalog) for the spans dataset.
use std::collections::BTreeSet;

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::{FieldInfo, FieldKind, SpanQueryEngine};

impl SpanQueryEngine {
    /// Every span field that could appear in the `[start, end]` window.
    pub fn fields(
        &self,
        start_ts_nanos: i64,
        end_ts_nanos: i64,
    ) -> Result<Vec<FieldInfo>, PhotonError> {
        let manifest = self.load_spans_manifest()?;
        let mut attrs: BTreeSet<String> = BTreeSet::new();
        for entry in manifest.candidates(start_ts_nanos, end_ts_nanos) {
            for k in &entry.attribute_keys {
                attrs.insert(k.clone());
            }
        }

        let mut out = Vec::new();
        for name in span_schema::SPAN_FIXED_COLUMNS {
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
    use photon_core::manifest::{FileEntry, Manifest, SPANS_MANIFEST_OBJECT_PATH};
    use photon_core::segment::SegmentId;
    use photon_core::span_schema::SpanSchema;

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
        std::fs::write(
            tmp.path().join(SPANS_MANIFEST_OBJECT_PATH),
            m.to_json().unwrap(),
        )
        .unwrap();

        let schema = SpanSchema::new(&["service.name".into(), "http.status_code".into()]);
        let engine = SpanQueryEngine::new(tmp.path().to_path_buf(), schema).unwrap();
        let fields = engine.fields(0, 200).unwrap();

        let has =
            |name: &str, kind: FieldKind| fields.iter().any(|f| f.name == name && f.kind == kind);
        assert!(has("region", FieldKind::Attribute));
        assert!(has("tier", FieldKind::Attribute));
        assert!(!fields.iter().any(|f| f.name == "only_outside")); // pruned by window
        assert!(has("service.name", FieldKind::Promoted));
        assert!(has("http.status_code", FieldKind::Promoted));
        assert!(has("trace_id", FieldKind::Fixed));
        assert!(has("name", FieldKind::Fixed));
        assert!(has("duration_nanos", FieldKind::Fixed));
    }
}

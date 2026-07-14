//! Per-file skip index: a bloom filter over tokenized `body` values plus min/max ranges
//! for `timestamp` and the promoted `service.name` column (the compactor's sort key).
//! Serializable to a sidecar `.idx` blob via `to_bytes`/`from_bytes` — a hand-rolled binary
//! layout (see those methods), with a fallback decode of the older serde_json format so
//! `.idx` files written by previous builds keep working.

use crate::bloom::Bloom;
use crate::tokenize::tokenize_dedup_into;
use arrow::array::{Array, StringArray, TimestampNanosecondArray};
use arrow::record_batch::RecordBatch;
use photon_core::schema::{self, LogSchema};
use photon_core::PhotonError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Names of the promoted attribute columns this index ranges over, when present. Match
/// the metrics compactor's sort key `(metric_name, service.name, host.name, timestamp)`.
const SERVICE_NAME_COLUMN: &str = "service.name";
const HOST_NAME_COLUMN: &str = "host.name";

// `Serialize` is kept only so `from_bytes`'s legacy-format fallback (and its test) can
// round-trip through `serde_json` exactly like the pre-binary-format encoder did; new
// writes always go through the binary `to_bytes` below.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipIndex {
    bloom: Bloom,
    timestamp_range: Option<(i64, i64)>,
    service_range: Option<(String, String)>,
    /// Inclusive min/max of the promoted `host.name` column (metrics only). `#[serde(default)]`
    /// so legacy serde_json `.idx` sidecars — and v1 binary sidecars — decode to `None`.
    #[serde(default)]
    host_range: Option<(String, String)>,
}

impl SkipIndex {
    /// Build from a batch. `body` is tokenized into the bloom; min/max captured for
    /// `timestamp` and the promoted `service.name` column (the sort key).
    pub fn build(batch: &RecordBatch, schema: &LogSchema) -> Result<SkipIndex, PhotonError> {
        let distinct = body_distinct_tokens(batch)?;

        let mut bloom = Bloom::new(distinct.len());
        for token in &distinct {
            bloom.insert(token);
        }

        let timestamp_range = timestamp_min_max(batch)?;
        let service_range = service_min_max(batch, schema)?;

        Ok(SkipIndex {
            bloom,
            timestamp_range,
            service_range,
            // Logs do not range over host.name.
            host_range: None,
        })
    }

    /// Build a spans skip index: bloom over `name` tokens + full `trace_id` values (so
    /// trace-by-id lookups prune to the few files that may hold the trace); min/max captured
    /// for `start_time_nanos` (stored in `timestamp_range`) and `service.name`.
    pub fn build_spans(batch: &RecordBatch) -> Result<SkipIndex, PhotonError> {
        use photon_core::span_schema;

        let mut distinct: HashSet<String> = HashSet::new();

        // `name` tokens for free-text operation search.
        if let Some(col) = batch.column_by_name(span_schema::NAME) {
            let names = col
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PhotonError::Index("name column is not Utf8".into()))?;
            for i in 0..names.len() {
                if names.is_valid(i) {
                    tokenize_dedup_into(names.value(i), &mut distinct);
                }
            }
        }

        // Full `trace_id` values so `get_trace` can prune by trace id.
        let tid = batch
            .column_by_name(span_schema::TRACE_ID)
            .ok_or_else(|| PhotonError::Index("batch is missing the trace_id column".into()))?
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| PhotonError::Index("trace_id column is not Utf8".into()))?;
        for i in 0..tid.len() {
            if tid.is_valid(i) {
                let v = tid.value(i);
                if !distinct.contains(v) {
                    distinct.insert(v.to_string());
                }
            }
        }

        let mut bloom = Bloom::new(distinct.len());
        for token in &distinct {
            bloom.insert(token);
        }

        let timestamp_range = start_time_min_max(batch)?;
        let service_range = span_service_min_max(batch)?;

        Ok(SkipIndex {
            bloom,
            timestamp_range,
            service_range,
            // Spans do not range over host.name.
            host_range: None,
        })
    }

    /// Build a metrics skip index: bloom over whole `metric_name` values (exact-match
    /// membership, like spans' `trace_id` — NOT tokenized), plus min/max captured for
    /// `timestamp` (the metrics timestamp column shares the logs column name, so
    /// `timestamp_min_max` works unchanged) and the promoted `service.name` column (reusing
    /// `span_service_min_max`, which already reads that literal column with no schema arg).
    pub fn build_metrics(batch: &RecordBatch) -> Result<SkipIndex, PhotonError> {
        use photon_core::metric_schema;

        let mut distinct: HashSet<String> = HashSet::new();
        let names = batch
            .column_by_name(metric_schema::METRIC_NAME)
            .ok_or_else(|| PhotonError::Index("batch is missing the metric_name column".into()))?
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| PhotonError::Index("metric_name column is not Utf8".into()))?;
        for i in 0..names.len() {
            if names.is_valid(i) {
                let v = names.value(i);
                if !distinct.contains(v) {
                    distinct.insert(v.to_string());
                }
            }
        }

        let mut bloom = Bloom::new(distinct.len());
        for token in &distinct {
            bloom.insert(token);
        }

        let timestamp_range = timestamp_min_max(batch)?;
        let service_range = span_service_min_max(batch)?;
        let host_range = column_string_min_max(batch, HOST_NAME_COLUMN)?;

        Ok(SkipIndex {
            bloom,
            timestamp_range,
            service_range,
            host_range,
        })
    }

    /// Bloom membership: false = token DEFINITELY absent; true = possibly present.
    pub fn might_contain_token(&self, token: &str) -> bool {
        self.bloom.might_contain(token)
    }

    /// All tokens possibly present (AND). Empty slice -> true.
    pub fn might_contain_all(&self, tokens: &[String]) -> bool {
        tokens.iter().all(|t| self.might_contain_token(t))
    }

    /// Inclusive min/max nanos for the timestamp column, if known.
    pub fn timestamp_range(&self) -> Option<(i64, i64)> {
        self.timestamp_range
    }

    /// Inclusive min/max for service.name, if known.
    pub fn service_range(&self) -> Option<(String, String)> {
        self.service_range.clone()
    }

    /// Inclusive min/max for host.name, if known (metrics only; `None` for logs/spans and for
    /// v1 sidecars written before the host block existed).
    pub fn host_range(&self) -> Option<(String, String)> {
        self.host_range.clone()
    }

    /// Hand-rolled binary encoding (see the module doc and the `idx_binary` helpers below
    /// for the exact layout). No external serialization crate on this path — it's a hot,
    /// small, self-describing format we fully control.
    pub fn to_bytes(&self) -> Vec<u8> {
        idx_binary::encode(self)
    }

    /// Decode a `.idx` blob. Tries the binary format first (magic-byte gated); falls back
    /// to the older serde_json format so `.idx` sidecars written by pre-binary-format
    /// builds keep loading. Any other input is a genuine error — the query side already
    /// treats an unreadable `.idx` as "keep the file" (conservative pruning).
    pub fn from_bytes(b: &[u8]) -> Result<SkipIndex, PhotonError> {
        if idx_binary::has_magic(b) {
            return idx_binary::decode(b);
        }
        serde_json::from_slice(b).map_err(|e| PhotonError::Index(e.to_string()))
    }
}

fn body_distinct_tokens(batch: &RecordBatch) -> Result<HashSet<String>, PhotonError> {
    let column = batch
        .column_by_name(schema::BODY)
        .ok_or_else(|| PhotonError::Index("batch is missing the body column".into()))?;
    let values = column
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PhotonError::Index("body column is not Utf8".into()))?;

    let mut distinct = HashSet::new();
    for i in 0..values.len() {
        if values.is_valid(i) {
            tokenize_dedup_into(values.value(i), &mut distinct);
        }
    }
    Ok(distinct)
}

/// Binary `.idx` codec. Layout (all integers little-endian):
///
/// ```text
/// magic:            4 bytes, b"PXSK"
/// version:          1 byte,  currently 2 (v1 sidecars still decode; they stop after has_service)
/// bloom.num_bits:   8 bytes (u64)
/// bloom.num_hashes: 4 bytes (u32)
/// bloom.bits_len:   8 bytes (u64)  -- byte length of the packed bit vector
/// bloom.bits:       bits_len bytes, raw
/// has_timestamp:    1 byte  (0 | 1)
///   [timestamp_lo:  8 bytes (i64)]
///   [timestamp_hi:  8 bytes (i64)]
/// has_service:      1 byte  (0 | 1)
///   [service_lo:    4-byte length (u32) + UTF-8 bytes]
///   [service_hi:    4-byte length (u32) + UTF-8 bytes]
/// has_host:         1 byte  (0 | 1)   -- v2+ only; absent in v1 sidecars → host_range = None
///   [host_lo:       4-byte length (u32) + UTF-8 bytes]
///   [host_hi:       4-byte length (u32) + UTF-8 bytes]
/// ```
///
/// Chosen over serde_json specifically because `Bloom::bits` (the bulk of the payload) was
/// being encoded as a JSON array of decimal integers — ~3-4x the raw size, and parsed
/// number-by-number on every query-side read.
mod idx_binary {
    use super::{Bloom, PhotonError, SkipIndex};

    const MAGIC: [u8; 4] = *b"PXSK";
    const VERSION: u8 = 2;

    pub(super) fn has_magic(b: &[u8]) -> bool {
        b.len() >= MAGIC.len() && b[..MAGIC.len()] == MAGIC
    }

    pub(super) fn encode(index: &SkipIndex) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);

        let (num_bits, num_hashes, bits) = index.bloom.raw_parts();
        out.extend_from_slice(&(num_bits as u64).to_le_bytes());
        out.extend_from_slice(&num_hashes.to_le_bytes());
        out.extend_from_slice(&(bits.len() as u64).to_le_bytes());
        out.extend_from_slice(bits);

        match index.timestamp_range {
            Some((lo, hi)) => {
                out.push(1);
                out.extend_from_slice(&lo.to_le_bytes());
                out.extend_from_slice(&hi.to_le_bytes());
            }
            None => out.push(0),
        }

        match &index.service_range {
            Some((lo, hi)) => {
                out.push(1);
                write_string(&mut out, lo);
                write_string(&mut out, hi);
            }
            None => out.push(0),
        }

        // v2: has_host + optional (host_lo, host_hi), mirroring the service block.
        match &index.host_range {
            Some((lo, hi)) => {
                out.push(1);
                write_string(&mut out, lo);
                write_string(&mut out, hi);
            }
            None => out.push(0),
        }

        out
    }

    pub(super) fn decode(b: &[u8]) -> Result<SkipIndex, PhotonError> {
        let mut cur = Cursor { buf: b, pos: 0 };

        let magic = cur.take(MAGIC.len())?;
        if magic != MAGIC {
            return Err(PhotonError::Index("skip index: bad magic".into()));
        }
        let version = cur.u8()?;
        // Accept every released version up to the current one (v1 sidecars stop after the service
        // block → host_range = None). Reject 0 and anything newer than we understand.
        if version == 0 || version > VERSION {
            return Err(PhotonError::Index(format!(
                "skip index: unsupported binary format version {version}"
            )));
        }

        let num_bits = cur.u64()? as usize;
        let num_hashes = cur.u32()?;
        let bits_len = cur.u64()? as usize;
        let bits = cur.take(bits_len)?.to_vec();
        let bloom = Bloom::from_raw_parts(num_bits, num_hashes, bits);

        let timestamp_range = if cur.u8()? == 1 {
            Some((cur.i64()?, cur.i64()?))
        } else {
            None
        };

        let service_range = if cur.u8()? == 1 {
            Some((cur.string()?, cur.string()?))
        } else {
            None
        };

        // v2+ carries a host block after the service block; v1 sidecars end here → None.
        let host_range = if version >= 2 {
            if cur.u8()? == 1 {
                Some((cur.string()?, cur.string()?))
            } else {
                None
            }
        } else {
            None
        };

        Ok(SkipIndex {
            bloom,
            timestamp_range,
            service_range,
            host_range,
        })
    }

    fn write_string(out: &mut Vec<u8>, s: &str) {
        let bytes = s.as_bytes();
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }

    /// Minimal forward-only byte cursor; every read is bounds-checked so a truncated or
    /// corrupt `.idx` produces a `PhotonError::Index` instead of a panic (the query side
    /// treats an unreadable `.idx` as "keep the file").
    struct Cursor<'a> {
        buf: &'a [u8],
        pos: usize,
    }

    impl<'a> Cursor<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], PhotonError> {
            let end = self
                .pos
                .checked_add(n)
                .ok_or_else(|| PhotonError::Index("skip index: length overflow".into()))?;
            let slice = self
                .buf
                .get(self.pos..end)
                .ok_or_else(|| PhotonError::Index("skip index: truncated binary .idx".into()))?;
            self.pos = end;
            Ok(slice)
        }

        fn u8(&mut self) -> Result<u8, PhotonError> {
            Ok(self.take(1)?[0])
        }

        fn u32(&mut self) -> Result<u32, PhotonError> {
            let bytes = self.take(4)?;
            Ok(u32::from_le_bytes(
                bytes.try_into().expect("take(4) yields a 4-byte slice"),
            ))
        }

        fn u64(&mut self) -> Result<u64, PhotonError> {
            let bytes = self.take(8)?;
            Ok(u64::from_le_bytes(
                bytes.try_into().expect("take(8) yields an 8-byte slice"),
            ))
        }

        fn i64(&mut self) -> Result<i64, PhotonError> {
            let bytes = self.take(8)?;
            Ok(i64::from_le_bytes(
                bytes.try_into().expect("take(8) yields an 8-byte slice"),
            ))
        }

        fn string(&mut self) -> Result<String, PhotonError> {
            let len = self.u32()? as usize;
            let bytes = self.take(len)?;
            String::from_utf8(bytes.to_vec()).map_err(|e| PhotonError::Index(e.to_string()))
        }
    }
}

fn timestamp_min_max(batch: &RecordBatch) -> Result<Option<(i64, i64)>, PhotonError> {
    let column = batch
        .column_by_name(schema::TIMESTAMP)
        .ok_or_else(|| PhotonError::Index("batch is missing the timestamp column".into()))?;
    let values = column
        .as_any()
        .downcast_ref::<TimestampNanosecondArray>()
        .ok_or_else(|| PhotonError::Index("timestamp column is not TimestampNanosecond".into()))?;

    let mut range: Option<(i64, i64)> = None;
    for i in 0..values.len() {
        if values.is_valid(i) {
            let v = values.value(i);
            range = Some(match range {
                None => (v, v),
                Some((lo, hi)) => (lo.min(v), hi.max(v)),
            });
        }
    }
    Ok(range)
}

fn service_min_max(
    batch: &RecordBatch,
    schema: &LogSchema,
) -> Result<Option<(String, String)>, PhotonError> {
    if !schema.promoted.iter().any(|p| p == SERVICE_NAME_COLUMN) {
        return Ok(None);
    }
    let Some(column) = batch.column_by_name(SERVICE_NAME_COLUMN) else {
        return Ok(None);
    };
    let values = column
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PhotonError::Index("service.name column is not Utf8".into()))?;

    let mut range: Option<(String, String)> = None;
    for i in 0..values.len() {
        if values.is_valid(i) {
            let v = values.value(i);
            range = Some(match range {
                None => (v.to_string(), v.to_string()),
                Some((lo, hi)) => {
                    let new_lo = if v < lo.as_str() { v.to_string() } else { lo };
                    let new_hi = if v > hi.as_str() { v.to_string() } else { hi };
                    (new_lo, new_hi)
                }
            });
        }
    }
    Ok(range)
}

fn start_time_min_max(batch: &RecordBatch) -> Result<Option<(i64, i64)>, PhotonError> {
    use photon_core::span_schema;
    let column = batch
        .column_by_name(span_schema::START_TIME)
        .ok_or_else(|| PhotonError::Index("batch is missing the start_time_nanos column".into()))?;
    let values = column
        .as_any()
        .downcast_ref::<TimestampNanosecondArray>()
        .ok_or_else(|| PhotonError::Index("start_time_nanos is not TimestampNanosecond".into()))?;
    let mut range: Option<(i64, i64)> = None;
    for i in 0..values.len() {
        if values.is_valid(i) {
            let v = values.value(i);
            range = Some(match range {
                None => (v, v),
                Some((lo, hi)) => (lo.min(v), hi.max(v)),
            });
        }
    }
    Ok(range)
}

fn span_service_min_max(batch: &RecordBatch) -> Result<Option<(String, String)>, PhotonError> {
    // Spans always promote service.name; if absent, keep-everything (None).
    let Some(column) = batch.column_by_name(SERVICE_NAME_COLUMN) else {
        return Ok(None);
    };
    let values = column
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PhotonError::Index("service.name column is not Utf8".into()))?;
    let mut range: Option<(String, String)> = None;
    for i in 0..values.len() {
        if values.is_valid(i) {
            let v = values.value(i);
            range = Some(match range {
                None => (v.to_string(), v.to_string()),
                Some((lo, hi)) => {
                    let new_lo = if v < lo.as_str() { v.to_string() } else { lo };
                    let new_hi = if v > hi.as_str() { v.to_string() } else { hi };
                    (new_lo, new_hi)
                }
            });
        }
    }
    Ok(range)
}

/// Inclusive min/max of a Utf8 column by name; `None` if the column is absent or all-null.
/// Used for the metrics `host.name` range (a missing/unknown range keeps the file at query time).
fn column_string_min_max(
    batch: &RecordBatch,
    col: &str,
) -> Result<Option<(String, String)>, PhotonError> {
    let Some(column) = batch.column_by_name(col) else {
        return Ok(None);
    };
    let values = column
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PhotonError::Index(format!("{col} column is not Utf8")))?;
    let mut range: Option<(String, String)> = None;
    for i in 0..values.len() {
        if values.is_valid(i) {
            let v = values.value(i);
            range = Some(match range {
                None => (v.to_string(), v.to_string()),
                Some((lo, hi)) => {
                    let new_lo = if v < lo.as_str() { v.to_string() } else { lo };
                    let new_hi = if v > hi.as_str() { v.to_string() } else { hi };
                    (new_lo, new_hi)
                }
            });
        }
    }
    Ok(range)
}

#[cfg(test)]
impl SkipIndex {
    /// Test-only: drop the captured host range so an encode produces a host-less blob (used to
    /// synthesize a legacy v1 sidecar for the backward-compat decode test).
    pub(crate) fn clear_host_range_for_test(&mut self) {
        self.host_range = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenize::tokenize;
    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    fn record(ts: i64, service: &str, body: &str) -> LogRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), service.to_string());
        LogRecord {
            timestamp_nanos: ts,
            body: Some(body.to_string()),
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn timestamp_and_service_range_capture_min_and_max() {
        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut builder = RecordBatchBuilder::new(&schema);
        builder.append(&record(300, "web", "hello"));
        builder.append(&record(100, "api", "world"));
        builder.append(&record(200, "auth", "foo"));
        let batch = builder.finish().unwrap();

        let index = SkipIndex::build(&batch, &schema).unwrap();

        assert_eq!(index.timestamp_range(), Some((100, 300)));
        assert_eq!(
            index.service_range(),
            Some(("api".to_string(), "web".to_string()))
        );
    }

    #[test]
    fn service_range_is_none_when_service_name_is_not_promoted() {
        let schema = LogSchema::new(&[]);
        let mut builder = RecordBatchBuilder::new(&schema);
        builder.append(&LogRecord {
            timestamp_nanos: 1,
            body: Some("hello".into()),
            ..Default::default()
        });
        let batch = builder.finish().unwrap();

        let index = SkipIndex::build(&batch, &schema).unwrap();
        assert_eq!(index.service_range(), None);
    }

    #[test]
    fn might_contain_all_is_true_for_empty_token_slice() {
        let schema = LogSchema::new(&[]);
        let mut builder = RecordBatchBuilder::new(&schema);
        builder.append(&LogRecord {
            timestamp_nanos: 1,
            body: Some("hello".into()),
            ..Default::default()
        });
        let batch = builder.finish().unwrap();

        let index = SkipIndex::build(&batch, &schema).unwrap();
        assert!(index.might_contain_all(&[]));
    }

    #[test]
    fn to_bytes_from_bytes_round_trips_membership_and_ranges() {
        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut builder = RecordBatchBuilder::new(&schema);
        builder.append(&record(100, "api", "hello world"));
        builder.append(&record(200, "web", "goodbye moon"));
        let batch = builder.finish().unwrap();
        let index = SkipIndex::build(&batch, &schema).unwrap();

        let bytes = index.to_bytes();
        let restored = SkipIndex::from_bytes(&bytes).unwrap();

        assert_eq!(index.timestamp_range(), restored.timestamp_range());
        assert_eq!(index.service_range(), restored.service_range());
        for token in ["hello", "world", "goodbye", "moon"] {
            assert!(restored.might_contain_token(token));
        }
    }

    #[test]
    fn to_bytes_round_trips_when_ranges_are_none() {
        // No promoted service.name and an index built with no rows at all exercises the
        // `has_timestamp == 0` / `has_service == 0` branches of the binary decoder.
        let schema = LogSchema::new(&[]);
        let builder = RecordBatchBuilder::new(&schema);
        let batch = builder.finish().unwrap();
        let index = SkipIndex::build(&batch, &schema).unwrap();
        assert_eq!(index.timestamp_range(), None);
        assert_eq!(index.service_range(), None);

        let restored = SkipIndex::from_bytes(&index.to_bytes()).unwrap();
        assert_eq!(restored.timestamp_range(), None);
        assert_eq!(restored.service_range(), None);
    }

    #[test]
    fn to_bytes_uses_the_binary_format_and_is_smaller_than_json() {
        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut builder = RecordBatchBuilder::new(&schema);
        for i in 0..200 {
            builder.append(&record(
                i,
                "api",
                "hello world goodbye moon repeated tokens",
            ));
        }
        let batch = builder.finish().unwrap();
        let index = SkipIndex::build(&batch, &schema).unwrap();

        let binary = index.to_bytes();
        assert_eq!(&binary[..4], b"PXSK");

        let json = serde_json::to_vec(&index).unwrap();
        assert!(
            binary.len() < json.len(),
            "binary encoding ({} bytes) should be smaller than JSON ({} bytes)",
            binary.len(),
            json.len()
        );
    }

    #[test]
    fn from_bytes_decodes_legacy_json_format() {
        // Simulates an `.idx` sidecar written by a pre-binary-format build: plain
        // `serde_json::to_vec`, no magic bytes. `from_bytes` must still load it.
        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut builder = RecordBatchBuilder::new(&schema);
        builder.append(&record(100, "api", "hello world"));
        builder.append(&record(200, "web", "goodbye moon"));
        let batch = builder.finish().unwrap();
        let index = SkipIndex::build(&batch, &schema).unwrap();

        let legacy_bytes = serde_json::to_vec(&index).unwrap();
        assert_ne!(&legacy_bytes[..4.min(legacy_bytes.len())], b"PXSK");

        let restored = SkipIndex::from_bytes(&legacy_bytes).unwrap();
        assert_eq!(index.timestamp_range(), restored.timestamp_range());
        assert_eq!(index.service_range(), restored.service_range());
        for token in ["hello", "world", "goodbye", "moon"] {
            assert!(restored.might_contain_token(token));
        }
    }

    #[test]
    fn from_bytes_rejects_garbage_input() {
        let err = SkipIndex::from_bytes(b"not json").unwrap_err();
        assert!(matches!(err, PhotonError::Index(_)));
    }

    #[test]
    fn from_bytes_rejects_truncated_binary_input() {
        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut builder = RecordBatchBuilder::new(&schema);
        builder.append(&record(100, "api", "hello world"));
        let batch = builder.finish().unwrap();
        let index = SkipIndex::build(&batch, &schema).unwrap();

        let mut bytes = index.to_bytes();
        bytes.truncate(bytes.len() / 2); // still has the magic, but is cut off mid-payload
        let err = SkipIndex::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, PhotonError::Index(_)));
    }

    #[test]
    fn from_bytes_rejects_unsupported_binary_version() {
        let schema = LogSchema::new(&[]);
        let mut builder = RecordBatchBuilder::new(&schema);
        builder.append(&record(100, "api", "hello"));
        let batch = builder.finish().unwrap();
        let index = SkipIndex::build(&batch, &schema).unwrap();

        let mut bytes = index.to_bytes();
        bytes[4] = 99; // corrupt the version byte (magic stays intact)
        let err = SkipIndex::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, PhotonError::Index(_)));
    }

    proptest! {
        /// Mandatory property test: for any generated set of log bodies, every token
        /// actually present in a body value returns `might_contain_token == true`. The
        /// bloom filter must NEVER produce a false negative, or query-time pruning would
        /// silently drop real results.
        #[test]
        fn bloom_never_reports_a_false_negative(bodies in proptest::collection::vec(".{0,60}", 0..25)) {
            let schema = LogSchema::new(&[]);
            let mut builder = RecordBatchBuilder::new(&schema);
            for body in &bodies {
                builder.append(&LogRecord {
                    timestamp_nanos: 0,
                    body: Some(body.clone()),
                    ..Default::default()
                });
            }
            let batch = builder.finish().unwrap();
            let index = SkipIndex::build(&batch, &schema).unwrap();

            for body in &bodies {
                for token in tokenize(body) {
                    prop_assert!(index.might_contain_token(&token));
                }
            }
        }
    }

    fn span(trace: &str, svc: &str, name: &str, start: i64) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        SpanRecord {
            trace_id: trace.to_string(),
            span_id: "s".into(),
            name: Some(name.to_string()),
            start_time_nanos: start,
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn build_spans_indexes_trace_id_name_and_ranges() {
        let schema = SpanSchema::new(&["service.name".to_string()]);
        let mut b = SpanBatchBuilder::new(&schema);
        b.append(&span("trace-abc", "checkout", "POST /checkout", 300));
        b.append(&span("trace-xyz", "payments", "charge card", 100));
        let batch = b.finish().unwrap();

        let idx = SkipIndex::build_spans(&batch).unwrap();
        // trace_id values are members (full-value bloom) → get_trace can find them.
        assert!(idx.might_contain_token("trace-abc"));
        assert!(idx.might_contain_token("trace-xyz"));
        // name tokens are members.
        assert!(idx.might_contain_token("checkout"));
        assert!(idx.might_contain_token("charge"));
        // ranges are over start_time + service.
        assert_eq!(idx.timestamp_range(), Some((100, 300)));
        assert_eq!(
            idx.service_range(),
            Some(("checkout".into(), "payments".into()))
        );
    }

    proptest! {
        #[test]
        fn spans_bloom_never_false_negative_for_trace_ids(
            ids in proptest::collection::vec("[a-f0-9]{4,16}", 1..20)
        ) {
            let schema = SpanSchema::new(&["service.name".to_string()]);
            let mut b = SpanBatchBuilder::new(&schema);
            for (i, id) in ids.iter().enumerate() {
                b.append(&span(id, "svc", "op", i as i64));
            }
            let batch = b.finish().unwrap();
            let idx = SkipIndex::build_spans(&batch).unwrap();
            for id in &ids {
                prop_assert!(idx.might_contain_token(id));
            }
        }
    }

    #[test]
    fn metrics_index_captures_metric_name_membership_and_ranges() {
        use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
        use photon_core::metric_schema::MetricSchema;
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        let mk = |name: &str, svc: &str, ts: i64| {
            let mut attrs = std::collections::BTreeMap::new();
            attrs.insert("service.name".to_string(), svc.to_string());
            MetricPoint {
                metric_name: name.into(),
                timestamp_nanos: ts,
                attributes: attrs,
                ..Default::default()
            }
        };
        b.append(&mk("cpu.usage", "web", 300));
        b.append(&mk("http.duration", "api", 100));
        let batch = b.finish().unwrap();
        let index = SkipIndex::build_metrics(&batch).unwrap();

        assert!(index.might_contain_token("cpu.usage"));
        assert!(index.might_contain_token("http.duration"));
        assert_eq!(index.timestamp_range(), Some((100, 300)));
        assert_eq!(
            index.service_range(),
            Some(("api".to_string(), "web".to_string()))
        );
    }

    /// Build a metrics batch with the promoted `service.name` + `host.name` columns set, nulling
    /// everything else — the minimal fixture for the host-range skip-index tests.
    fn metrics_batch(
        schema: &photon_core::metric_schema::MetricSchema,
        names: &[&str],
        services: &[&str],
        hosts: &[&str],
        ts: &[i64],
    ) -> RecordBatch {
        use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
        let mut b = MetricBatchBuilder::new(schema);
        for (((name, svc), host), t) in names.iter().zip(services).zip(hosts).zip(ts) {
            let mut attrs = BTreeMap::new();
            attrs.insert("service.name".to_string(), svc.to_string());
            attrs.insert("host.name".to_string(), host.to_string());
            b.append(&MetricPoint {
                metric_name: name.to_string(),
                timestamp_nanos: *t,
                attributes: attrs,
                ..Default::default()
            });
        }
        b.finish().unwrap()
    }

    #[test]
    fn metrics_index_captures_host_range_and_roundtrips_binary() {
        let schema = photon_core::metric_schema::MetricSchema::new(&[
            "service.name".to_string(),
            "host.name".to_string(),
        ]);
        let batch = metrics_batch(
            &schema,
            &["m", "m"],
            &["svc", "svc"],
            &["web-2", "web-1"],
            &[100i64, 200],
        );
        let idx = SkipIndex::build_metrics(&batch).unwrap();
        assert_eq!(
            idx.host_range(),
            Some(("web-1".to_string(), "web-2".to_string()))
        );

        // binary v2 round-trips the host range
        let bytes = idx.to_bytes();
        let back = SkipIndex::from_bytes(&bytes).unwrap();
        assert_eq!(
            back.host_range(),
            Some(("web-1".to_string(), "web-2".to_string()))
        );
    }

    #[test]
    fn legacy_v1_index_decodes_with_no_host_range() {
        // A v1-format index (service only, no host block) must still decode; host_range → None.
        let mut idx = SkipIndex::build_metrics(&metrics_batch(
            &photon_core::metric_schema::MetricSchema::new(&[
                "service.name".to_string(),
                "host.name".to_string(),
            ]),
            &["m"],
            &["svc"],
            &["h"],
            &[1i64],
        ))
        .unwrap();
        // Clear the host range, then downgrade the v2 blob to a genuine v1 sidecar: flip the
        // version byte to 1 and drop the trailing (has_host = 0) host block. `decode` must accept
        // the older version and default host_range to None.
        idx.clear_host_range_for_test();
        let mut v1 = idx.to_bytes();
        v1[4] = 1; // version byte (after the 4-byte magic)
        v1.pop(); // strip the has_host = 0 byte that only exists in v2
        let back = SkipIndex::from_bytes(&v1).unwrap();
        assert_eq!(back.host_range(), None);
    }

    proptest! {
        /// Mandatory: the metrics bloom never false-negatives for metric_name — else query-time
        /// pruning could silently drop a real metric's files.
        #[test]
        fn metrics_bloom_never_false_negative_for_metric_names(
            names in proptest::collection::vec("[a-z][a-z0-9._]{0,30}", 0..25)
        ) {
            use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
            use photon_core::metric_schema::MetricSchema;
            let schema = MetricSchema::new(&["service.name".to_string()]);
            let mut b = MetricBatchBuilder::new(&schema);
            for name in &names {
                b.append(&MetricPoint {
                    metric_name: name.clone(),
                    timestamp_nanos: 0,
                    ..Default::default()
                });
            }
            let batch = b.finish().unwrap();
            let index = SkipIndex::build_metrics(&batch).unwrap();
            for name in &names {
                prop_assert!(index.might_contain_token(name));
            }
        }
    }
}

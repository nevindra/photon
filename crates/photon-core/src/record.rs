use crate::schema::LogSchema;
use crate::PhotonError;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct LogRecord {
    pub timestamp_nanos: i64,
    pub observed_timestamp_nanos: Option<i64>,
    pub severity_number: Option<i32>,
    pub severity_text: Option<String>,
    pub body: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub scope_name: Option<String>,
    /// ALL attributes (promoted and long-tail). The builder routes each
    /// key to its promoted column, or into the attributes map if not promoted.
    pub attributes: BTreeMap<String, String>,
}

/// Borrowed view of a log row's non-attribute columns, for the streaming append path.
/// All owned data stays in the caller (the freshly-decoded OTLP request); nothing is cloned.
#[derive(Debug, Default, Clone)]
pub struct LogFixed<'a> {
    pub timestamp_nanos: i64,
    pub observed_timestamp_nanos: Option<i64>,
    pub severity_number: Option<i32>,
    pub severity_text: Option<&'a str>,
    pub body: Option<&'a str>,
    pub trace_id: Option<&'a str>,
    pub span_id: Option<&'a str>,
    pub scope_name: Option<&'a str>,
}

use arrow::array::{ArrayRef, Int32Builder, MapBuilder, StringBuilder, TimestampNanosecondBuilder};
use arrow::record_batch::RecordBatch;
use std::collections::HashSet;
use std::sync::Arc;

pub struct RecordBatchBuilder {
    schema: LogSchema,
    /// `schema.promoted` as a set, so attribute routing is an O(1) lookup
    /// instead of an O(promoted) linear scan per attribute.
    promoted_set: HashSet<String>,
    timestamp: TimestampNanosecondBuilder,
    observed_timestamp: TimestampNanosecondBuilder,
    severity_number: Int32Builder,
    severity_text: StringBuilder,
    body: StringBuilder,
    trace_id: StringBuilder,
    span_id: StringBuilder,
    scope_name: StringBuilder,
    promoted: Vec<StringBuilder>,
    attributes: MapBuilder<StringBuilder, StringBuilder>,
}

impl RecordBatchBuilder {
    pub fn new(schema: &LogSchema) -> RecordBatchBuilder {
        RecordBatchBuilder {
            schema: schema.clone(),
            promoted_set: schema.promoted.iter().cloned().collect(),
            timestamp: TimestampNanosecondBuilder::new(),
            observed_timestamp: TimestampNanosecondBuilder::new(),
            severity_number: Int32Builder::new(),
            severity_text: StringBuilder::new(),
            body: StringBuilder::new(),
            trace_id: StringBuilder::new(),
            span_id: StringBuilder::new(),
            scope_name: StringBuilder::new(),
            promoted: schema
                .promoted
                .iter()
                .map(|_| StringBuilder::new())
                .collect(),
            attributes: MapBuilder::new(None, StringBuilder::new(), StringBuilder::new()),
        }
    }

    /// Same as [`Self::new`], but pre-sizes every column builder for an
    /// expected row count so large batches don't pay for geometric
    /// reallocation. The byte capacities are rough per-column averages
    /// (short ids, longer bodies); builders still grow past them for
    /// oversized values, this just avoids most of the doubling.
    pub fn with_capacity(schema: &LogSchema, rows: usize) -> RecordBatchBuilder {
        RecordBatchBuilder {
            schema: schema.clone(),
            promoted_set: schema.promoted.iter().cloned().collect(),
            timestamp: TimestampNanosecondBuilder::with_capacity(rows),
            observed_timestamp: TimestampNanosecondBuilder::with_capacity(rows),
            severity_number: Int32Builder::with_capacity(rows),
            severity_text: StringBuilder::with_capacity(rows, rows * 8),
            body: StringBuilder::with_capacity(rows, rows * 128),
            trace_id: StringBuilder::with_capacity(rows, rows * 32),
            span_id: StringBuilder::with_capacity(rows, rows * 16),
            scope_name: StringBuilder::with_capacity(rows, rows * 24),
            promoted: schema
                .promoted
                .iter()
                .map(|_| StringBuilder::with_capacity(rows, rows * 24))
                .collect(),
            attributes: MapBuilder::with_capacity(
                None,
                StringBuilder::with_capacity(rows, rows * 16),
                StringBuilder::with_capacity(rows, rows * 32),
                rows,
            ),
        }
    }

    pub fn append(&mut self, record: &LogRecord) {
        self.timestamp.append_value(record.timestamp_nanos);
        self.observed_timestamp
            .append_option(record.observed_timestamp_nanos);
        self.severity_number.append_option(record.severity_number);
        self.severity_text
            .append_option(record.severity_text.as_ref());
        self.body.append_option(record.body.as_ref());
        self.trace_id.append_option(record.trace_id.as_ref());
        self.span_id.append_option(record.span_id.as_ref());
        self.scope_name.append_option(record.scope_name.as_ref());

        // route attributes: promoted keys -> their column, rest -> map
        for (i, name) in self.schema.promoted.iter().enumerate() {
            self.promoted[i].append_option(record.attributes.get(name));
        }
        for (k, v) in &record.attributes {
            if self.promoted_set.contains(k) {
                continue;
            }
            self.attributes.keys().append_value(k);
            self.attributes.values().append_value(v);
        }
        // close this row's map entry (append_option-style boolean = valid row)
        self.attributes.append(true).expect("map row append");
    }

    /// Append one row straight from borrowed OTLP data — no `LogRecord`, no per-record
    /// `BTreeMap`. `attrs` yields the row's merged (resource + record) attributes as
    /// borrowed key/value pairs. Promoted keys route to their column; the rest go to the map.
    pub fn append_streaming<'a, I>(&mut self, fixed: LogFixed<'a>, attrs: I)
    where
        I: Iterator<Item = (&'a str, &'a str)> + Clone,
    {
        self.timestamp.append_value(fixed.timestamp_nanos);
        self.observed_timestamp
            .append_option(fixed.observed_timestamp_nanos);
        self.severity_number.append_option(fixed.severity_number);
        self.severity_text.append_option(fixed.severity_text);
        self.body.append_option(fixed.body);
        self.trace_id.append_option(fixed.trace_id);
        self.span_id.append_option(fixed.span_id);
        self.scope_name.append_option(fixed.scope_name);

        // Promoted columns: for each promoted name, find its value among the attrs (a handful
        // of keys; linear scan of a short per-row list, same cost class as the old map .get).
        for (i, name) in self.schema.promoted.iter().enumerate() {
            let v = attrs.clone().find(|(k, _)| k == name).map(|(_, v)| v);
            self.promoted[i].append_option(v);
        }
        // Long-tail attrs → the map (skip promoted keys).
        for (k, v) in attrs {
            if self.promoted_set.contains(k) {
                continue;
            }
            self.attributes.keys().append_value(k);
            self.attributes.values().append_value(v);
        }
        self.attributes.append(true).expect("map row append");
    }

    pub fn finish(mut self) -> Result<RecordBatch, PhotonError> {
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(self.timestamp.finish()),
            Arc::new(self.observed_timestamp.finish()),
            Arc::new(self.severity_number.finish()),
            Arc::new(self.severity_text.finish()),
            Arc::new(self.body.finish()),
            Arc::new(self.trace_id.finish()),
            Arc::new(self.span_id.finish()),
            Arc::new(self.scope_name.finish()),
        ];
        for mut b in self.promoted {
            columns.push(Arc::new(b.finish()));
        }
        columns.push(Arc::new(self.attributes.finish()));

        RecordBatch::try_new(self.schema.arrow.clone(), columns)
            .map_err(|e| PhotonError::Arrow(e.to_string()))
    }
}

use arrow::array::{Array, Int32Array, MapArray, StringArray, TimestampNanosecondArray};

/// Decode one row of a log `RecordBatch` back into a `LogRecord` for in-memory
/// predicate evaluation on the streaming path. Every promoted string column
/// (INCLUDING `service.name`) AND the long-tail `attributes` Map column are folded
/// into `attributes`, matching what `FieldRef::Attr`/`MapAttr` read (they both look
/// up `record.attributes.get(name)`). This deliberately differs from
/// `photon-api::search::row_to_json`, which breaks `service.name` out separately.
pub fn log_record_from_batch(batch: &RecordBatch, row: usize) -> LogRecord {
    use crate::schema;
    let s = |name: &str| -> Option<String> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .filter(|c| !c.is_null(row))
            .map(|c| c.value(row).to_string())
    };
    let timestamp_nanos = batch
        .column_by_name(schema::TIMESTAMP)
        .and_then(|c| c.as_any().downcast_ref::<TimestampNanosecondArray>())
        .filter(|c| !c.is_null(row))
        .map(|c| c.value(row))
        .unwrap_or(0);
    let observed_timestamp_nanos = batch
        .column_by_name(schema::OBSERVED_TIMESTAMP)
        .and_then(|c| c.as_any().downcast_ref::<TimestampNanosecondArray>())
        .filter(|c| !c.is_null(row))
        .map(|c| c.value(row));
    let severity_number = batch
        .column_by_name(schema::SEVERITY_NUMBER)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .filter(|c| !c.is_null(row))
        .map(|c| c.value(row));

    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    // Long-tail attributes Map column.
    if let Some(map) = batch
        .column_by_name(schema::ATTRIBUTES)
        .and_then(|c| c.as_any().downcast_ref::<MapArray>())
    {
        if !map.is_null(row) {
            let offsets = map.value_offsets();
            let entries = map.entries();
            if let (Some(keys), Some(values)) = (
                entries.column(0).as_any().downcast_ref::<StringArray>(),
                entries.column(1).as_any().downcast_ref::<StringArray>(),
            ) {
                for i in offsets[row] as usize..offsets[row + 1] as usize {
                    if !values.is_null(i) {
                        attributes.insert(keys.value(i).to_string(), values.value(i).to_string());
                    }
                }
            }
        }
    }
    // Promoted string columns (anything that is neither a fixed column nor the map) —
    // INCLUDING service.name.
    for field in batch.schema().fields() {
        let name = field.name();
        if schema::FIXED_COLUMNS.contains(&name.as_str()) || name == schema::ATTRIBUTES {
            continue;
        }
        if let Some(v) = s(name) {
            attributes.insert(name.clone(), v);
        }
    }

    LogRecord {
        timestamp_nanos,
        observed_timestamp_nanos,
        severity_number,
        severity_text: s(schema::SEVERITY_TEXT),
        body: s(schema::BODY),
        trace_id: s(schema::TRACE_ID),
        span_id: s(schema::SPAN_ID),
        scope_name: s(schema::SCOPE_NAME),
        attributes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, StringArray, TimestampNanosecondArray};

    fn rec(ts: i64, service: &str, extra: &[(&str, &str)]) -> LogRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), service.to_string());
        for (k, v) in extra {
            attributes.insert(k.to_string(), v.to_string());
        }
        LogRecord {
            timestamp_nanos: ts,
            body: Some("hello".into()),
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn builds_batch_routing_promoted_vs_map() {
        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut b = RecordBatchBuilder::new(&schema);
        b.append(&rec(100, "api", &[("region", "us")]));
        b.append(&rec(200, "web", &[]));
        let batch = b.finish().unwrap();

        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema(), schema.arrow);

        let ts = batch
            .column_by_name("timestamp")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        assert_eq!(ts.value(0), 100);
        assert_eq!(ts.value(1), 200);

        // service.name landed in its promoted column, not the map
        let svc = batch
            .column_by_name("service.name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(svc.value(0), "api");
        assert_eq!(svc.value(1), "web");
    }

    #[test]
    fn decode_roundtrips_for_predicate() {
        use crate::query::{parse, FieldResolver};

        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut b = RecordBatchBuilder::with_capacity(&schema, 1);
        let mut attrs = BTreeMap::new();
        attrs.insert("service.name".to_string(), "checkout".to_string());
        attrs.insert("http.method".to_string(), "GET".to_string()); // long-tail
        b.append(&LogRecord {
            timestamp_nanos: 42,
            severity_number: Some(17), // ERROR range
            body: Some("boom happened".to_string()),
            attributes: attrs,
            ..Default::default()
        });
        let batch = b.finish().unwrap();

        let rec = log_record_from_batch(&batch, 0);
        assert_eq!(rec.timestamp_nanos, 42);
        assert_eq!(
            rec.attributes.get("service.name").map(String::as_str),
            Some("checkout")
        );
        assert_eq!(
            rec.attributes.get("http.method").map(String::as_str),
            Some("GET")
        );

        let resolver = FieldResolver::new(&["service.name".to_string()]);
        let q = resolver
            .resolve(&parse("service:checkout http.method:GET boom").unwrap())
            .unwrap();
        assert!(
            q.matches(&rec),
            "decoded record must satisfy the grammar predicate"
        );

        let q2 = resolver.resolve(&parse("service:other").unwrap()).unwrap();
        assert!(!q2.matches(&rec));
    }

    #[test]
    fn append_streaming_routes_promoted_and_map_without_a_logrecord() {
        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut b = RecordBatchBuilder::with_capacity(&schema, 2);
        b.append_streaming(
            LogFixed {
                timestamp_nanos: 100,
                ..LogFixed::default()
            },
            [("service.name", "api"), ("region", "us")].into_iter(),
        );
        b.append_streaming(
            LogFixed {
                timestamp_nanos: 200,
                ..LogFixed::default()
            },
            [("service.name", "web")].into_iter(),
        );
        let batch = b.finish().unwrap();
        assert_eq!(batch.num_rows(), 2);
        let svc = batch
            .column_by_name("service.name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(svc.value(0), "api");
        assert_eq!(svc.value(1), "web");
        // `region` is a long-tail attr → lands in the map, not a promoted column.
        let ts = batch
            .column_by_name("timestamp")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        assert_eq!(ts.value(0), 100);
    }
}

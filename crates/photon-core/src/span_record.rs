use crate::span_schema::SpanSchema;
use crate::PhotonError;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct SpanRecord {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: Option<String>,
    pub kind: Option<i32>,
    pub kind_text: Option<String>,
    pub start_time_nanos: i64,
    pub end_time_nanos: Option<i64>,
    pub duration_nanos: Option<i64>,
    pub status_code: Option<i32>,
    pub status_text: Option<String>,
    pub status_message: Option<String>,
    pub scope_name: Option<String>,
    /// JSON-encoded array of `{name, time_unix_nano, attributes}`.
    pub events: Option<String>,
    /// JSON-encoded array of `{trace_id, span_id, attributes}`.
    pub links: Option<String>,
    /// ALL attributes (promoted + long-tail); the builder routes each key.
    pub attributes: BTreeMap<String, String>,
}

/// Borrowed view of a span row's non-attribute columns, for the streaming append path.
/// All owned data stays in the caller (the freshly-decoded OTLP request); nothing is cloned.
/// `trace_id`/`span_id`/`start_time_nanos` are required (mirrors `SpanBatchBuilder::append`,
/// which `append_value`s them); everything else is optional.
#[derive(Debug, Default, Clone)]
pub struct SpanFixed<'a> {
    pub trace_id: &'a str,
    pub span_id: &'a str,
    pub parent_span_id: Option<&'a str>,
    pub name: Option<&'a str>,
    pub kind: Option<i32>,
    pub kind_text: Option<&'a str>,
    pub start_time_nanos: i64,
    pub end_time_nanos: Option<i64>,
    pub duration_nanos: Option<i64>,
    pub status_code: Option<i32>,
    pub status_text: Option<&'a str>,
    pub status_message: Option<&'a str>,
    pub scope_name: Option<&'a str>,
    pub events: Option<&'a str>,
    pub links: Option<&'a str>,
}

use arrow::array::{
    ArrayRef, Int32Builder, Int64Builder, MapBuilder, StringBuilder, TimestampNanosecondBuilder,
};
use arrow::record_batch::RecordBatch;
use std::collections::HashSet;
use std::sync::Arc;

pub struct SpanBatchBuilder {
    schema: SpanSchema,
    /// `schema.promoted` as a set, so attribute routing is an O(1) lookup
    /// instead of an O(promoted) linear scan per attribute.
    promoted_set: HashSet<String>,
    trace_id: StringBuilder,
    span_id: StringBuilder,
    parent_span_id: StringBuilder,
    name: StringBuilder,
    kind: Int32Builder,
    kind_text: StringBuilder,
    start_time: TimestampNanosecondBuilder,
    end_time: Int64Builder,
    duration: Int64Builder,
    status_code: Int32Builder,
    status_text: StringBuilder,
    status_message: StringBuilder,
    scope_name: StringBuilder,
    events: StringBuilder,
    links: StringBuilder,
    promoted: Vec<StringBuilder>,
    attributes: MapBuilder<StringBuilder, StringBuilder>,
}

impl SpanBatchBuilder {
    pub fn new(schema: &SpanSchema) -> SpanBatchBuilder {
        SpanBatchBuilder {
            schema: schema.clone(),
            promoted_set: schema.promoted.iter().cloned().collect(),
            trace_id: StringBuilder::new(),
            span_id: StringBuilder::new(),
            parent_span_id: StringBuilder::new(),
            name: StringBuilder::new(),
            kind: Int32Builder::new(),
            kind_text: StringBuilder::new(),
            start_time: TimestampNanosecondBuilder::new(),
            end_time: Int64Builder::new(),
            duration: Int64Builder::new(),
            status_code: Int32Builder::new(),
            status_text: StringBuilder::new(),
            status_message: StringBuilder::new(),
            scope_name: StringBuilder::new(),
            events: StringBuilder::new(),
            links: StringBuilder::new(),
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
    /// (short ids, longer JSON events/links); builders still grow past
    /// them for oversized values, this just avoids most of the doubling.
    pub fn with_capacity(schema: &SpanSchema, rows: usize) -> SpanBatchBuilder {
        SpanBatchBuilder {
            schema: schema.clone(),
            promoted_set: schema.promoted.iter().cloned().collect(),
            trace_id: StringBuilder::with_capacity(rows, rows * 32),
            span_id: StringBuilder::with_capacity(rows, rows * 16),
            parent_span_id: StringBuilder::with_capacity(rows, rows * 16),
            name: StringBuilder::with_capacity(rows, rows * 24),
            kind: Int32Builder::with_capacity(rows),
            kind_text: StringBuilder::with_capacity(rows, rows * 8),
            start_time: TimestampNanosecondBuilder::with_capacity(rows),
            end_time: Int64Builder::with_capacity(rows),
            duration: Int64Builder::with_capacity(rows),
            status_code: Int32Builder::with_capacity(rows),
            status_text: StringBuilder::with_capacity(rows, rows * 8),
            status_message: StringBuilder::with_capacity(rows, rows * 32),
            scope_name: StringBuilder::with_capacity(rows, rows * 24),
            events: StringBuilder::with_capacity(rows, rows * 128),
            links: StringBuilder::with_capacity(rows, rows * 64),
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

    pub fn append(&mut self, record: &SpanRecord) {
        self.trace_id.append_value(&record.trace_id);
        self.span_id.append_value(&record.span_id);
        self.parent_span_id
            .append_option(record.parent_span_id.as_ref());
        self.name.append_option(record.name.as_ref());
        self.kind.append_option(record.kind);
        self.kind_text.append_option(record.kind_text.as_ref());
        self.start_time.append_value(record.start_time_nanos);
        self.end_time.append_option(record.end_time_nanos);
        self.duration.append_option(record.duration_nanos);
        self.status_code.append_option(record.status_code);
        self.status_text.append_option(record.status_text.as_ref());
        self.status_message
            .append_option(record.status_message.as_ref());
        self.scope_name.append_option(record.scope_name.as_ref());
        self.events.append_option(record.events.as_ref());
        self.links.append_option(record.links.as_ref());

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
        self.attributes.append(true).expect("map row append");
    }

    /// Append one row straight from borrowed OTLP data — no `SpanRecord`, no per-span
    /// `BTreeMap`. `attrs` yields the span's merged (resource + span) attributes as
    /// borrowed key/value pairs. Promoted keys route to their column; the rest go to the map.
    pub fn append_streaming<'a, I>(&mut self, fixed: SpanFixed<'a>, attrs: I)
    where
        I: Iterator<Item = (&'a str, &'a str)> + Clone,
    {
        self.trace_id.append_value(fixed.trace_id);
        self.span_id.append_value(fixed.span_id);
        self.parent_span_id.append_option(fixed.parent_span_id);
        self.name.append_option(fixed.name);
        self.kind.append_option(fixed.kind);
        self.kind_text.append_option(fixed.kind_text);
        self.start_time.append_value(fixed.start_time_nanos);
        self.end_time.append_option(fixed.end_time_nanos);
        self.duration.append_option(fixed.duration_nanos);
        self.status_code.append_option(fixed.status_code);
        self.status_text.append_option(fixed.status_text);
        self.status_message.append_option(fixed.status_message);
        self.scope_name.append_option(fixed.scope_name);
        self.events.append_option(fixed.events);
        self.links.append_option(fixed.links);

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
            Arc::new(self.trace_id.finish()),
            Arc::new(self.span_id.finish()),
            Arc::new(self.parent_span_id.finish()),
            Arc::new(self.name.finish()),
            Arc::new(self.kind.finish()),
            Arc::new(self.kind_text.finish()),
            Arc::new(self.start_time.finish()),
            Arc::new(self.end_time.finish()),
            Arc::new(self.duration.finish()),
            Arc::new(self.status_code.finish()),
            Arc::new(self.status_text.finish()),
            Arc::new(self.status_message.finish()),
            Arc::new(self.scope_name.finish()),
            Arc::new(self.events.finish()),
            Arc::new(self.links.finish()),
        ];
        for mut b in self.promoted {
            columns.push(Arc::new(b.finish()));
        }
        columns.push(Arc::new(self.attributes.finish()));

        RecordBatch::try_new(self.schema.arrow.clone(), columns)
            .map_err(|e| PhotonError::Arrow(e.to_string()))
    }
}

use arrow::array::{
    Array, Int32Array, Int64Array, MapArray, StringArray, TimestampNanosecondArray,
};

/// Decode one row of a span `RecordBatch` back into a `SpanRecord` for in-memory
/// predicate evaluation on the streaming path. Every promoted string column
/// (INCLUDING `service.name`) AND the long-tail `attributes` Map column are folded
/// into `attributes`, matching what `SpanFieldRef::Attr`/`MapAttr` read (both look up
/// `record.attributes.get(name)`).
pub fn span_record_from_batch(batch: &RecordBatch, row: usize) -> SpanRecord {
    use crate::span_schema;
    let s = |name: &str| -> Option<String> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .filter(|c| !c.is_null(row))
            .map(|c| c.value(row).to_string())
    };
    let i32c = |name: &str| -> Option<i32> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
            .filter(|c| !c.is_null(row))
            .map(|c| c.value(row))
    };
    let i64c = |name: &str| -> Option<i64> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .filter(|c| !c.is_null(row))
            .map(|c| c.value(row))
    };

    let start_time_nanos = batch
        .column_by_name(span_schema::START_TIME)
        .and_then(|c| c.as_any().downcast_ref::<TimestampNanosecondArray>())
        .filter(|c| !c.is_null(row))
        .map(|c| c.value(row))
        .unwrap_or(0);

    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    // Long-tail attributes Map column.
    if let Some(map) = batch
        .column_by_name(span_schema::ATTRIBUTES)
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
    // Promoted string columns (neither a fixed column nor the map) — INCLUDING service.name.
    for field in batch.schema().fields() {
        let name = field.name();
        if span_schema::SPAN_FIXED_COLUMNS.contains(&name.as_str())
            || name == span_schema::ATTRIBUTES
        {
            continue;
        }
        if let Some(v) = s(name) {
            attributes.insert(name.clone(), v);
        }
    }

    SpanRecord {
        trace_id: s(span_schema::TRACE_ID).unwrap_or_default(),
        span_id: s(span_schema::SPAN_ID).unwrap_or_default(),
        parent_span_id: s(span_schema::PARENT_SPAN_ID),
        name: s(span_schema::NAME),
        kind: i32c(span_schema::KIND),
        kind_text: s(span_schema::KIND_TEXT),
        start_time_nanos,
        end_time_nanos: i64c(span_schema::END_TIME),
        duration_nanos: i64c(span_schema::DURATION),
        status_code: i32c(span_schema::STATUS_CODE),
        status_text: s(span_schema::STATUS_TEXT),
        status_message: s(span_schema::STATUS_MESSAGE),
        scope_name: s(span_schema::SCOPE_NAME),
        events: s(span_schema::EVENTS),
        links: s(span_schema::LINKS),
        attributes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, StringArray, TimestampNanosecondArray};

    fn span(trace: &str, svc: &str, start: i64) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        SpanRecord {
            trace_id: trace.to_string(),
            span_id: "aaaa".to_string(),
            name: Some("op".to_string()),
            start_time_nanos: start,
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn decode_span_roundtrips_for_predicate() {
        use crate::query::{parse, SpanFieldResolver};

        let schema = SpanSchema::new(&["service.name".to_string()]);
        let mut b = SpanBatchBuilder::with_capacity(&schema, 1);
        let mut attrs = BTreeMap::new();
        attrs.insert("service.name".to_string(), "checkout".to_string());
        attrs.insert("http.method".to_string(), "GET".to_string()); // long-tail
        b.append(&SpanRecord {
            trace_id: "abc".to_string(),
            span_id: "def".to_string(),
            name: Some("GET /pay".to_string()),
            status_code: Some(2), // error
            start_time_nanos: 42,
            attributes: attrs,
            ..Default::default()
        });
        let batch = b.finish().unwrap();

        let rec = span_record_from_batch(&batch, 0);
        assert_eq!(rec.trace_id, "abc");
        assert_eq!(rec.span_id, "def");
        assert_eq!(rec.start_time_nanos, 42);
        assert_eq!(rec.status_code, Some(2));
        assert_eq!(
            rec.attributes.get("service.name").map(String::as_str),
            Some("checkout")
        );
        assert_eq!(
            rec.attributes.get("http.method").map(String::as_str),
            Some("GET")
        );

        let resolver = SpanFieldResolver::new(&["service.name".to_string()]);
        let q = resolver
            .resolve(&parse("service:checkout status:error").unwrap())
            .unwrap();
        assert!(q.matches(&rec));

        let q2 = resolver.resolve(&parse("service:other").unwrap()).unwrap();
        assert!(!q2.matches(&rec));
    }

    #[test]
    fn builds_batch_routing_promoted_vs_map() {
        let schema = SpanSchema::new(&["service.name".to_string()]);
        let mut b = SpanBatchBuilder::new(&schema);
        b.append(&span("t1", "checkout", 100));
        b.append(&span("t2", "payments", 200));
        let batch = b.finish().unwrap();

        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema(), schema.arrow);

        let tid = batch
            .column_by_name("trace_id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(tid.value(0), "t1");

        let svc = batch
            .column_by_name("service.name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(svc.value(0), "checkout");

        let start = batch
            .column_by_name("start_time_nanos")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        assert_eq!(start.value(1), 200);
    }

    #[test]
    fn append_streaming_routes_promoted_and_map_without_a_spanrecord() {
        let schema = SpanSchema::new(&["service.name".to_string()]);
        let mut b = SpanBatchBuilder::with_capacity(&schema, 2);
        b.append_streaming(
            SpanFixed {
                trace_id: "t1",
                span_id: "s1",
                start_time_nanos: 100,
                ..SpanFixed::default()
            },
            [("service.name", "api"), ("region", "us")].into_iter(),
        );
        b.append_streaming(
            SpanFixed {
                trace_id: "t2",
                span_id: "s2",
                start_time_nanos: 200,
                ..SpanFixed::default()
            },
            [("service.name", "web")].into_iter(),
        );
        let batch = b.finish().unwrap();
        assert_eq!(batch.num_rows(), 2);
        // `service.name` lands in its promoted column, `region` is long-tail (goes to the map).
        let svc = batch
            .column_by_name("service.name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(svc.value(0), "api");
        assert_eq!(svc.value(1), "web");
        let start = batch
            .column_by_name("start_time_nanos")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap();
        assert_eq!(start.value(0), 100);
        assert_eq!(start.value(1), 200);
    }
}

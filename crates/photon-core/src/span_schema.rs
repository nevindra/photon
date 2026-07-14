use crate::schema::attributes_map_type;
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use std::sync::Arc;

pub const TRACE_ID: &str = "trace_id";
pub const SPAN_ID: &str = "span_id";
pub const PARENT_SPAN_ID: &str = "parent_span_id";
pub const NAME: &str = "name";
pub const KIND: &str = "kind";
pub const KIND_TEXT: &str = "kind_text";
pub const START_TIME: &str = "start_time_nanos";
pub const END_TIME: &str = "end_time_nanos";
pub const DURATION: &str = "duration_nanos";
pub const STATUS_CODE: &str = "status_code";
pub const STATUS_TEXT: &str = "status_text";
pub const STATUS_MESSAGE: &str = "status_message";
pub const SCOPE_NAME: &str = "scope_name";
pub const EVENTS: &str = "events";
pub const LINKS: &str = "links";
pub const ATTRIBUTES: &str = "attributes";

/// All reserved span column names. A promoted attribute must not collide with any of these.
pub const SPAN_FIXED_COLUMNS: &[&str] = &[
    TRACE_ID,
    SPAN_ID,
    PARENT_SPAN_ID,
    NAME,
    KIND,
    KIND_TEXT,
    START_TIME,
    END_TIME,
    DURATION,
    STATUS_CODE,
    STATUS_TEXT,
    STATUS_MESSAGE,
    SCOPE_NAME,
    EVENTS,
    LINKS,
    ATTRIBUTES,
];

/// Columnar schema for a batch of spans. Layout: 15 fixed columns, then one Utf8 column per
/// promoted attribute, then a Map<Utf8,Utf8> for long-tail attributes. Sort key is
/// `(service.name, start_time_nanos)`.
#[derive(Debug, Clone)]
pub struct SpanSchema {
    pub arrow: Arc<Schema>,
    pub promoted: Vec<String>,
}

impl SpanSchema {
    pub fn new(promoted: &[String]) -> SpanSchema {
        let mut fields: Vec<Field> = vec![
            Field::new(TRACE_ID, DataType::Utf8, false),
            Field::new(SPAN_ID, DataType::Utf8, false),
            Field::new(PARENT_SPAN_ID, DataType::Utf8, true),
            Field::new(NAME, DataType::Utf8, true),
            Field::new(KIND, DataType::Int32, true),
            Field::new(KIND_TEXT, DataType::Utf8, true),
            Field::new(
                START_TIME,
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                false,
            ),
            Field::new(END_TIME, DataType::Int64, true),
            Field::new(DURATION, DataType::Int64, true),
            Field::new(STATUS_CODE, DataType::Int32, true),
            Field::new(STATUS_TEXT, DataType::Utf8, true),
            Field::new(STATUS_MESSAGE, DataType::Utf8, true),
            Field::new(SCOPE_NAME, DataType::Utf8, true),
            Field::new(EVENTS, DataType::Utf8, true),
            Field::new(LINKS, DataType::Utf8, true),
        ];
        for name in promoted {
            fields.push(Field::new(name, DataType::Utf8, true));
        }
        fields.push(Field::new(ATTRIBUTES, attributes_map_type(), true));
        SpanSchema {
            arrow: Arc::new(Schema::new(fields)),
            promoted: promoted.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_fixed_promoted_and_map_columns() {
        let s = SpanSchema::new(&["service.name".to_string()]);
        let f = s.arrow.fields();
        // 15 fixed + 1 promoted + 1 attributes map = 17
        assert_eq!(f.len(), 17);
        assert_eq!(s.arrow.field(0).name(), TRACE_ID);
        assert_eq!(
            s.arrow.field(6).data_type(),
            &DataType::Timestamp(TimeUnit::Nanosecond, None)
        );
        assert!(!s.arrow.field(6).is_nullable()); // start_time is required
        assert_eq!(s.arrow.field(15).name(), "service.name");
        let last = s.arrow.field(16);
        assert_eq!(last.name(), ATTRIBUTES);
        assert!(matches!(last.data_type(), DataType::Map(_, _)));
    }
}

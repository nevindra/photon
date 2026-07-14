use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use std::sync::Arc;

pub const TIMESTAMP: &str = "timestamp";
pub const OBSERVED_TIMESTAMP: &str = "observed_timestamp";
pub const SEVERITY_NUMBER: &str = "severity_number";
pub const SEVERITY_TEXT: &str = "severity_text";
pub const BODY: &str = "body";
pub const TRACE_ID: &str = "trace_id";
pub const SPAN_ID: &str = "span_id";
pub const SCOPE_NAME: &str = "scope_name";
pub const ATTRIBUTES: &str = "attributes";

/// All reserved column names — the fixed columns plus the attributes map. A promoted
/// attribute must not collide with any of these (enforced by `Config::validate`).
pub const FIXED_COLUMNS: &[&str] = &[
    TIMESTAMP,
    OBSERVED_TIMESTAMP,
    SEVERITY_NUMBER,
    SEVERITY_TEXT,
    BODY,
    TRACE_ID,
    SPAN_ID,
    SCOPE_NAME,
    ATTRIBUTES,
];

/// The internal columnar schema for one batch of log records.
/// Layout: fixed columns, then one Utf8 column per promoted attribute,
/// then a Map<Utf8,Utf8> column for long-tail attributes.
#[derive(Debug, Clone)]
pub struct LogSchema {
    pub arrow: Arc<Schema>,
    pub promoted: Vec<String>,
}

impl LogSchema {
    pub fn new(promoted: &[String]) -> LogSchema {
        let mut fields: Vec<Field> = vec![
            Field::new(
                TIMESTAMP,
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                false,
            ),
            Field::new(
                OBSERVED_TIMESTAMP,
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                true,
            ),
            Field::new(SEVERITY_NUMBER, DataType::Int32, true),
            Field::new(SEVERITY_TEXT, DataType::Utf8, true),
            Field::new(BODY, DataType::Utf8, true),
            Field::new(TRACE_ID, DataType::Utf8, true),
            Field::new(SPAN_ID, DataType::Utf8, true),
            Field::new(SCOPE_NAME, DataType::Utf8, true),
        ];
        for name in promoted {
            fields.push(Field::new(name, DataType::Utf8, true));
        }
        fields.push(Field::new(ATTRIBUTES, attributes_map_type(), true));
        LogSchema {
            arrow: Arc::new(Schema::new(fields)),
            promoted: promoted.to_vec(),
        }
    }
}

/// Map<Utf8, Utf8> — non-null keys, nullable values.
pub(crate) fn attributes_map_type() -> DataType {
    let entries = Field::new(
        "entries",
        DataType::Struct(
            vec![
                Field::new("keys", DataType::Utf8, false),
                Field::new("values", DataType::Utf8, true),
            ]
            .into(),
        ),
        false,
    );
    DataType::Map(Arc::new(entries), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_fixed_promoted_and_map_columns() {
        let s = LogSchema::new(&["service.name".to_string(), "host.name".to_string()]);
        let f = s.arrow.fields();
        // 8 fixed + 2 promoted + 1 attributes map = 11
        assert_eq!(f.len(), 11);
        assert_eq!(s.arrow.field(0).name(), TIMESTAMP);
        assert_eq!(
            s.arrow.field(0).data_type(),
            &DataType::Timestamp(TimeUnit::Nanosecond, None)
        );
        // promoted columns appear after the fixed ones, in order
        assert_eq!(s.arrow.field(8).name(), "service.name");
        assert_eq!(s.arrow.field(9).name(), "host.name");
        assert_eq!(s.arrow.field(8).data_type(), &DataType::Utf8);
        // attributes map is last
        let last = s.arrow.field(10);
        assert_eq!(last.name(), ATTRIBUTES);
        assert!(matches!(last.data_type(), DataType::Map(_, _)));
    }
}

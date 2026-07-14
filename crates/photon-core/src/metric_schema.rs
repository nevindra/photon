//! Metrics Arrow schema. Mirrors `schema.rs`/`span_schema.rs`: fixed typed columns → one
//! column per promoted attribute → a single `Map<Utf8,Utf8>` attributes column. Distribution
//! payloads (histogram/exp-histogram/summary) and exemplars are JSON string columns, exactly
//! like spans' `events`/`links` — the codebase has no Arrow List/Struct value columns.
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema, TimeUnit};

use crate::schema::attributes_map_type;

pub const METRIC_NAME: &str = "metric_name";
pub const METRIC_TYPE: &str = "metric_type";
pub const TYPE_TEXT: &str = "type_text";
pub const TEMPORALITY: &str = "temporality";
pub const IS_MONOTONIC: &str = "is_monotonic";
pub const UNIT: &str = "unit";
pub const TIMESTAMP: &str = "timestamp";
pub const START_TIMESTAMP: &str = "start_timestamp";
pub const SCOPE_NAME: &str = "scope_name";
pub const VALUE: &str = "value";
pub const HISTOGRAM: &str = "histogram";
pub const EXP_HISTOGRAM: &str = "exp_histogram";
pub const SUMMARY: &str = "summary";
pub const EXEMPLARS: &str = "exemplars";
pub const ATTRIBUTES: &str = "attributes";

/// Fixed (non-promoted, non-map) column names, for collision checks.
pub const METRIC_FIXED_COLUMNS: &[&str] = &[
    METRIC_NAME,
    METRIC_TYPE,
    TYPE_TEXT,
    TEMPORALITY,
    IS_MONOTONIC,
    UNIT,
    TIMESTAMP,
    START_TIMESTAMP,
    SCOPE_NAME,
    VALUE,
    HISTOGRAM,
    EXP_HISTOGRAM,
    SUMMARY,
    EXEMPLARS,
];

/// Numeric discriminators stored in the `metric_type` column.
pub mod metric_type {
    pub const GAUGE: i32 = 0;
    pub const SUM: i32 = 1;
    pub const HISTOGRAM: i32 = 2;
    pub const EXP_HISTOGRAM: i32 = 3;
    pub const SUMMARY: i32 = 4;
}

/// Columnar schema for a batch of metric points. Layout: 14 fixed columns, then one Utf8
/// column per promoted attribute, then a Map<Utf8,Utf8> for long-tail attributes. Sort key is
/// `(metric_name, service.name, timestamp)`.
#[derive(Debug, Clone)]
pub struct MetricSchema {
    pub arrow: Arc<Schema>,
    pub promoted: Vec<String>,
}

impl MetricSchema {
    pub fn new(promoted: &[String]) -> MetricSchema {
        let ts = || DataType::Timestamp(TimeUnit::Nanosecond, None);
        let mut fields: Vec<Field> = vec![
            Field::new(METRIC_NAME, DataType::Utf8, false),
            Field::new(METRIC_TYPE, DataType::Int32, false),
            Field::new(TYPE_TEXT, DataType::Utf8, true),
            Field::new(TEMPORALITY, DataType::Int32, true),
            Field::new(IS_MONOTONIC, DataType::Boolean, true),
            Field::new(UNIT, DataType::Utf8, true),
            Field::new(TIMESTAMP, ts(), false),
            Field::new(START_TIMESTAMP, ts(), true),
            Field::new(SCOPE_NAME, DataType::Utf8, true),
            Field::new(VALUE, DataType::Float64, true),
            Field::new(HISTOGRAM, DataType::Utf8, true),
            Field::new(EXP_HISTOGRAM, DataType::Utf8, true),
            Field::new(SUMMARY, DataType::Utf8, true),
            Field::new(EXEMPLARS, DataType::Utf8, true),
        ];
        for name in promoted {
            fields.push(Field::new(name, DataType::Utf8, true));
        }
        fields.push(Field::new(ATTRIBUTES, attributes_map_type(), true));
        MetricSchema {
            arrow: Arc::new(Schema::new(fields)),
            promoted: promoted.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, TimeUnit};

    #[test]
    fn builds_fixed_promoted_and_map_columns() {
        let s = MetricSchema::new(&["service.name".to_string()]);
        let f = s.arrow.fields();
        // 14 fixed + 1 promoted + 1 attributes map = 16
        assert_eq!(f.len(), 16);
        assert_eq!(s.arrow.field(0).name(), METRIC_NAME);
        assert!(!s.arrow.field(0).is_nullable(), "metric_name is required");
        assert_eq!(
            s.arrow.field(6).data_type(),
            &DataType::Timestamp(TimeUnit::Nanosecond, None)
        );
        assert!(!s.arrow.field(6).is_nullable(), "timestamp is required");
        assert_eq!(s.arrow.field(9).name(), VALUE);
        assert_eq!(s.arrow.field(9).data_type(), &DataType::Float64);
        // promoted column follows the 14 fixed ones
        assert_eq!(s.arrow.field(14).name(), "service.name");
        // attributes map is last
        let last = s.arrow.field(15);
        assert_eq!(last.name(), ATTRIBUTES);
        assert!(matches!(last.data_type(), DataType::Map(_, _)));
    }
}

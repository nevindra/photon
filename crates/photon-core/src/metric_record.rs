use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanBuilder, Float64Builder, Int32Builder, MapBuilder, RecordBatch, StringBuilder,
    TimestampNanosecondBuilder,
};

use crate::metric_schema::MetricSchema;
use crate::PhotonError;

#[derive(Debug, Clone, Default)]
pub struct MetricPoint {
    pub metric_name: String,
    pub metric_type: i32,
    pub type_text: Option<String>,
    pub temporality: Option<i32>,
    pub is_monotonic: Option<bool>,
    pub unit: Option<String>,
    pub timestamp_nanos: i64,
    pub start_timestamp_nanos: Option<i64>,
    pub scope_name: Option<String>,
    pub value: Option<f64>,
    /// JSON `{count,sum,bucket_counts[],explicit_bounds[],min?,max?}`.
    pub histogram: Option<String>,
    /// JSON `{count,sum,scale,zero_count,positive,negative,min?,max?}`.
    pub exp_histogram: Option<String>,
    /// JSON `{count,sum,quantiles:[{quantile,value}]}`.
    pub summary: Option<String>,
    /// JSON `[{value,timestamp_nanos,trace_id,span_id,filtered_attributes}]`.
    pub exemplars: Option<String>,
    /// ALL attributes (promoted + long-tail); the builder routes each key.
    pub attributes: BTreeMap<String, String>,
}

pub struct MetricBatchBuilder {
    schema: Arc<arrow::datatypes::Schema>,
    promoted_names: Vec<String>,
    /// `schema.promoted` as a set, so attribute routing is an O(1) lookup
    /// instead of an O(promoted) linear scan per attribute.
    promoted_set: HashSet<String>,
    metric_name: StringBuilder,
    metric_type: Int32Builder,
    type_text: StringBuilder,
    temporality: Int32Builder,
    is_monotonic: BooleanBuilder,
    unit: StringBuilder,
    timestamp: TimestampNanosecondBuilder,
    start_timestamp: TimestampNanosecondBuilder,
    scope_name: StringBuilder,
    value: Float64Builder,
    histogram: StringBuilder,
    exp_histogram: StringBuilder,
    summary: StringBuilder,
    exemplars: StringBuilder,
    promoted: Vec<StringBuilder>,
    attributes: MapBuilder<StringBuilder, StringBuilder>,
}

impl MetricBatchBuilder {
    pub fn new(schema: &MetricSchema) -> MetricBatchBuilder {
        MetricBatchBuilder::with_capacity(schema, 0)
    }

    /// Same as [`Self::new`], but pre-sizes the identifier column builders for an
    /// expected row count so large batches don't pay for geometric reallocation.
    pub fn with_capacity(schema: &MetricSchema, rows: usize) -> MetricBatchBuilder {
        MetricBatchBuilder {
            schema: schema.arrow.clone(),
            promoted_names: schema.promoted.clone(),
            promoted_set: schema.promoted.iter().cloned().collect(),
            metric_name: StringBuilder::with_capacity(rows, rows * 16),
            metric_type: Int32Builder::with_capacity(rows),
            type_text: StringBuilder::new(),
            temporality: Int32Builder::with_capacity(rows),
            is_monotonic: BooleanBuilder::with_capacity(rows),
            unit: StringBuilder::new(),
            timestamp: TimestampNanosecondBuilder::with_capacity(rows),
            start_timestamp: TimestampNanosecondBuilder::with_capacity(rows),
            scope_name: StringBuilder::new(),
            value: Float64Builder::with_capacity(rows),
            histogram: StringBuilder::new(),
            exp_histogram: StringBuilder::new(),
            summary: StringBuilder::new(),
            exemplars: StringBuilder::new(),
            promoted: schema
                .promoted
                .iter()
                .map(|_| StringBuilder::new())
                .collect(),
            attributes: MapBuilder::new(None, StringBuilder::new(), StringBuilder::new()),
        }
    }

    pub fn append(&mut self, point: &MetricPoint) {
        self.metric_name.append_value(&point.metric_name);
        self.metric_type.append_value(point.metric_type);
        self.type_text.append_option(point.type_text.as_ref());
        self.temporality.append_option(point.temporality);
        self.is_monotonic.append_option(point.is_monotonic);
        self.unit.append_option(point.unit.as_ref());
        self.timestamp.append_value(point.timestamp_nanos);
        self.start_timestamp
            .append_option(point.start_timestamp_nanos);
        self.scope_name.append_option(point.scope_name.as_ref());
        self.value.append_option(point.value);
        self.histogram.append_option(point.histogram.as_ref());
        self.exp_histogram
            .append_option(point.exp_histogram.as_ref());
        self.summary.append_option(point.summary.as_ref());
        self.exemplars.append_option(point.exemplars.as_ref());

        // route attributes: promoted keys -> their own column, the rest -> the map
        for (i, name) in self.promoted_names.iter().enumerate() {
            self.promoted[i].append_option(point.attributes.get(name));
        }
        for (k, v) in &point.attributes {
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
            Arc::new(self.metric_name.finish()),
            Arc::new(self.metric_type.finish()),
            Arc::new(self.type_text.finish()),
            Arc::new(self.temporality.finish()),
            Arc::new(self.is_monotonic.finish()),
            Arc::new(self.unit.finish()),
            Arc::new(self.timestamp.finish()),
            Arc::new(self.start_timestamp.finish()),
            Arc::new(self.scope_name.finish()),
            Arc::new(self.value.finish()),
            Arc::new(self.histogram.finish()),
            Arc::new(self.exp_histogram.finish()),
            Arc::new(self.summary.finish()),
            Arc::new(self.exemplars.finish()),
        ];
        for b in &mut self.promoted {
            columns.push(Arc::new(b.finish()));
        }
        columns.push(Arc::new(self.attributes.finish()));
        RecordBatch::try_new(self.schema.clone(), columns)
            .map_err(|e| PhotonError::Arrow(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metric_schema::{metric_type, MetricSchema, METRIC_NAME, VALUE};
    use arrow::array::{Array, Float64Array, StringArray};

    fn point(name: &str, svc: &str, ts: i64, value: f64) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        attributes.insert("http.route".to_string(), "/checkout".to_string());
        MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::GAUGE,
            timestamp_nanos: ts,
            value: Some(value),
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn builds_batch_routing_promoted_vs_map() {
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        b.append(&point("cpu.usage", "checkout", 100, 0.73));
        b.append(&point("cpu.usage", "cart", 200, 0.41));
        let batch = b.finish().unwrap();

        assert_eq!(batch.num_rows(), 2);
        let names = batch
            .column_by_name(METRIC_NAME)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "cpu.usage");
        let values = batch
            .column_by_name(VALUE)
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(values.value(0), 0.73);
        // promoted service.name column populated; not duplicated into the map
        let svc = batch
            .column_by_name("service.name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(svc.value(0), "checkout");
    }
}

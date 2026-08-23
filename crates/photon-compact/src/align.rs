//! Re-shaping a decoded batch to the schema this process writes.
//!
//! A merge pass concatenates batches decoded out of Parquet files that were written at different
//! times, and `concat_batches` requires every batch to have the identical schema. That held while
//! a deployment's schema never changed, and stopped holding the moment one did: changing
//! `[schema].promoted_attributes` adds or removes a top-level column, and from then on every merge
//! pass failed with
//!
//! ```text
//! merge_once failed: arrow error: Invalid argument error:
//!   It is not possible to concatenate arrays of different data types.
//! ```
//!
//! once a minute, forever, silently leaving small files unconsolidated — the exact cost the merge
//! exists to remove. This aligns each input to the current schema first: a column the batch does
//! not have becomes nulls, a column whose type has drifted is cast, and the columns come out in
//! the schema's order.
//!
//! A column the batch HAS and the schema does not is dropped, which is the un-promotion case.
//! Nothing is lost that was reachable: the read path resolves an un-promoted key out of the
//! attributes map, where the value already is, and has no way to read the stale column.

use std::sync::Arc;

use arrow::array::{new_null_array, RecordBatch};
use arrow::compute::cast;
use arrow::datatypes::SchemaRef;

use photon_core::PhotonError;

/// `batch` with `target`'s columns, in `target`'s order.
pub(crate) fn align_to(
    batch: &RecordBatch,
    target: &SchemaRef,
) -> Result<RecordBatch, PhotonError> {
    if batch.schema() == *target {
        return Ok(batch.clone());
    }
    let rows = batch.num_rows();
    let mut columns = Vec::with_capacity(target.fields().len());
    for field in target.fields() {
        let column = match batch.column_by_name(field.name()) {
            // The common case even when the schemas differ: most columns are untouched, and
            // cloning an `ArrayRef` copies a pointer, not the data.
            Some(c) if c.data_type() == field.data_type() => Arc::clone(c),
            Some(c) => cast(c, field.data_type())
                .map_err(|e| PhotonError::Arrow(format!("align `{}`: {e}", field.name())))?,
            None => new_null_array(field.data_type(), rows),
        };
        columns.push(column);
    }
    RecordBatch::try_new(Arc::clone(target), columns)
        .map_err(|e| PhotonError::Arrow(format!("align batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};

    fn schema(fields: Vec<Field>) -> SchemaRef {
        Arc::new(Schema::new(fields))
    }

    #[test]
    fn an_identical_schema_is_returned_untouched() {
        let s = schema(vec![Field::new("a", DataType::Int64, true)]);
        let b =
            RecordBatch::try_new(s.clone(), vec![Arc::new(Int64Array::from(vec![1, 2]))]).unwrap();
        assert_eq!(align_to(&b, &s).unwrap(), b);
    }

    #[test]
    fn a_column_the_batch_never_had_becomes_nulls() {
        // The promotion case: an old file has no column for the newly promoted key, and the value
        // is still in its attributes map where the read path coalesces it back.
        let old = schema(vec![Field::new("a", DataType::Int64, true)]);
        let new = schema(vec![
            Field::new("a", DataType::Int64, true),
            Field::new("tenant", DataType::Utf8, true),
        ]);
        let b = RecordBatch::try_new(old, vec![Arc::new(Int64Array::from(vec![1, 2]))]).unwrap();
        let out = align_to(&b, &new).unwrap();
        assert_eq!(out.num_rows(), 2);
        assert_eq!(out.column(1).null_count(), 2);
    }

    #[test]
    fn a_column_the_schema_no_longer_has_is_dropped() {
        let old = schema(vec![
            Field::new("a", DataType::Int64, true),
            Field::new("gone", DataType::Utf8, true),
        ]);
        let new = schema(vec![Field::new("a", DataType::Int64, true)]);
        let b = RecordBatch::try_new(
            old,
            vec![
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(StringArray::from(vec!["x"])),
            ],
        )
        .unwrap();
        let out = align_to(&b, &new).unwrap();
        assert_eq!(out.num_columns(), 1);
    }

    #[test]
    fn columns_come_back_in_the_schemas_order() {
        let old = schema(vec![
            Field::new("b", DataType::Utf8, true),
            Field::new("a", DataType::Int64, true),
        ]);
        let new = schema(vec![
            Field::new("a", DataType::Int64, true),
            Field::new("b", DataType::Utf8, true),
        ]);
        let batch = RecordBatch::try_new(
            old,
            vec![
                Arc::new(StringArray::from(vec!["x"])),
                Arc::new(Int64Array::from(vec![7])),
            ],
        )
        .unwrap();
        let out = align_to(&batch, &new).unwrap();
        assert_eq!(out.schema(), new);
        let a = out.column(0).as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(a.value(0), 7);
    }
}

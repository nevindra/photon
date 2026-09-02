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

use std::collections::HashSet;
use std::sync::Arc;

use arrow::array::{
    new_null_array, Array, ArrayBuilder, ArrayRef, MapArray, RecordBatch, StringArray,
    StringBuilder, StructArray,
};
use arrow::buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};

use photon_core::PhotonError;

/// Move promoted keys that are still sitting in a batch's long-tail `attributes` map into their
/// own top-level columns, and drop them from the map.
///
/// This is what makes promoting a key *pay* on data that already exists. Promotion only changes
/// how NEW rows are written, so before this the values stayed in the map until every file
/// predating the change aged out — one whole retention period. `align_to` made that worse rather
/// than better: it filled the newly promoted column with NULLs and left the values behind.
///
/// The cost of leaving them there is not bytes, it is the plan.
/// `photon_query::span_engine::attr_lookup` reads a promoted key from its column alone **only
/// when no surviving file still declares it in the manifest's `attribute_keys`**; while even one
/// does, every query `coalesce`s column-then-map and the whole `attributes` map is decoded. That
/// map is 63% of a span file's bytes and 96% of its decoded bytes: measured on 300M spans,
/// `/api/red` over 7 days took 12.3 s touching the map and 2.2 s without. So hoisting on the merge
/// path is what eventually flips that switch, one consolidated file at a time.
///
/// Only a key whose column is **absent or entirely NULL** is a candidate — a column the writer
/// filled already routed correctly, and an all-NULL column that the map has nothing for stays
/// NULL. Returns the batch untouched (cheap `Arc` clones) when there is nothing to move, which is
/// every WAL-path compaction: `RecordBatchBuilder` routes promoted keys out of the map at append
/// time, so the fast path costs one `null_count()` per promoted key and no map scan at all.
pub(crate) fn hoist_promoted(
    batch: &RecordBatch,
    promoted: &[String],
) -> Result<RecordBatch, PhotonError> {
    let attrs_name = photon_core::schema::ATTRIBUTES;
    // Candidates: promoted keys with no column, or a column that is all NULL.
    let candidates: Vec<&str> = promoted
        .iter()
        .filter(|k| match batch.column_by_name(k) {
            None => true,
            Some(c) => c.null_count() == c.len(),
        })
        .map(|k| k.as_str())
        .collect();
    if candidates.is_empty() {
        return Ok(batch.clone());
    }
    let Some(map) = batch
        .column_by_name(attrs_name)
        .and_then(|c| c.as_any().downcast_ref::<MapArray>())
    else {
        return Ok(batch.clone());
    };
    let (Some(keys), Some(vals)) = (
        map.keys().as_any().downcast_ref::<StringArray>(),
        map.values().as_any().downcast_ref::<StringArray>(),
    ) else {
        return Ok(batch.clone());
    };

    // ONE pass over the keys child array decides which candidates are actually in this map, so a
    // key that is genuinely absent costs a scan rather than a rebuild.
    let wanted: HashSet<&str> = candidates.iter().copied().collect();
    let mut present: HashSet<&str> = HashSet::new();
    for i in 0..keys.len() {
        let k = keys.value(i);
        if let Some(hit) = wanted.get(k) {
            present.insert(hit);
            if present.len() == wanted.len() {
                break;
            }
        }
    }
    if present.is_empty() {
        return Ok(batch.clone());
    }

    let hoisting: Vec<&str> = candidates
        .iter()
        .copied()
        .filter(|k| present.contains(k))
        .collect();
    let mut built: Vec<StringBuilder> = hoisting.iter().map(|_| StringBuilder::new()).collect();
    let mut kb = StringBuilder::new();
    let mut vb = StringBuilder::new();
    let mut offsets: Vec<i32> = Vec::with_capacity(batch.num_rows() + 1);
    offsets.push(0);
    let mut valid: Vec<bool> = Vec::with_capacity(batch.num_rows());
    let offs = map.offsets();
    // Reused per row: index into `hoisting` -> the value found for it on this row.
    let mut found: Vec<Option<&str>> = vec![None; hoisting.len()];

    for row in 0..batch.num_rows() {
        found.iter_mut().for_each(|f| *f = None);
        if map.is_null(row) {
            valid.push(false);
        } else {
            valid.push(true);
            for i in offs[row] as usize..offs[row + 1] as usize {
                let k = keys.value(i);
                if let Some(slot) = hoisting.iter().position(|h| *h == k) {
                    // First value wins, matching the upstream `BTreeMap` the builder routes from.
                    if found[slot].is_none() && !vals.is_null(i) {
                        found[slot] = Some(vals.value(i));
                    }
                    continue; // hoisted: it leaves the map
                }
                kb.append_value(k);
                if vals.is_null(i) {
                    vb.append_null();
                } else {
                    vb.append_value(vals.value(i));
                }
            }
        }
        offsets.push(ArrayBuilder::len(&kb) as i32);
        for (slot, b) in built.iter_mut().enumerate() {
            b.append_option(found[slot]);
        }
    }

    let new_map = rebuild_map(
        kb.finish(),
        vb.finish(),
        offsets,
        valid,
        batch
            .schema()
            .field_with_name(attrs_name)
            .map_err(|e| PhotonError::Arrow(e.to_string()))?
            .data_type(),
    )?;

    // Replace the columns we filled; append the ones that had no column at all. `align_to` puts
    // them in the target schema's order afterwards.
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
    for (slot, name) in hoisting.iter().enumerate() {
        let array: ArrayRef = Arc::new(built[slot].finish());
        match batch.schema().index_of(name) {
            Ok(i) => columns[i] = array,
            Err(_) => {
                fields.push(Field::new(*name, DataType::Utf8, true));
                columns.push(array);
            }
        }
    }
    let ai = batch
        .schema()
        .index_of(attrs_name)
        .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    columns[ai] = new_map;

    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
        .map_err(|e| PhotonError::Arrow(format!("hoist promoted attributes: {e}")))
}

/// Rebuild a Map column from its parts, forced to `target`'s exact Map type so the result still
/// concatenates with batches that were not rebuilt.
fn rebuild_map(
    keys: StringArray,
    vals: StringArray,
    offsets: Vec<i32>,
    valid: Vec<bool>,
    target: &DataType,
) -> Result<ArrayRef, PhotonError> {
    let DataType::Map(entry_field, ordered) = target else {
        return Err(PhotonError::Arrow(
            "attributes column is not a Map".to_string(),
        ));
    };
    let DataType::Struct(fields) = entry_field.data_type() else {
        return Err(PhotonError::Arrow(
            "attributes map entry is not a Struct".to_string(),
        ));
    };
    let entries = StructArray::try_new(
        fields.clone(),
        vec![Arc::new(keys) as ArrayRef, Arc::new(vals) as ArrayRef],
        None,
    )
    .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    let nulls = if valid.iter().all(|v| *v) {
        None
    } else {
        Some(NullBuffer::from(valid))
    };
    let map = MapArray::try_new(
        entry_field.clone(),
        OffsetBuffer::new(ScalarBuffer::from(offsets)),
        entries,
        nulls,
        *ordered,
    )
    .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    Ok(Arc::new(map))
}

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
    use arrow::array::{Int64Array, MapBuilder, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};

    /// A batch of `rows` map rows plus whatever extra columns are given. `None` in `rows` is a
    /// NULL map row.
    fn batch_with_map(
        rows: &[Option<Vec<(&str, Option<&str>)>>],
        extra: Vec<(Field, ArrayRef)>,
    ) -> RecordBatch {
        let mut mb = MapBuilder::new(None, StringBuilder::new(), StringBuilder::new());
        for row in rows {
            match row {
                None => mb.append(false).unwrap(),
                Some(entries) => {
                    for (k, v) in entries {
                        mb.keys().append_value(k);
                        match v {
                            Some(v) => mb.values().append_value(v),
                            None => mb.values().append_null(),
                        }
                    }
                    mb.append(true).unwrap();
                }
            }
        }
        let map = mb.finish();
        let mut fields: Vec<Field> = extra.iter().map(|(f, _)| f.clone()).collect();
        let mut cols: Vec<ArrayRef> = extra.into_iter().map(|(_, a)| a).collect();
        fields.push(Field::new(
            photon_core::schema::ATTRIBUTES,
            map.data_type().clone(),
            true,
        ));
        cols.push(Arc::new(map));
        RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).unwrap()
    }

    /// Every `(key, value)` of one row's map, in order.
    fn map_row(batch: &RecordBatch, row: usize) -> Vec<(String, Option<String>)> {
        let m = batch
            .column_by_name(photon_core::schema::ATTRIBUTES)
            .unwrap()
            .as_any()
            .downcast_ref::<MapArray>()
            .unwrap()
            .clone();
        if m.is_null(row) {
            return Vec::new();
        }
        let keys = m.keys().as_any().downcast_ref::<StringArray>().unwrap();
        let vals = m.values().as_any().downcast_ref::<StringArray>().unwrap();
        let o = m.offsets();
        (o[row] as usize..o[row + 1] as usize)
            .map(|i| {
                (
                    keys.value(i).to_string(),
                    (!vals.is_null(i)).then(|| vals.value(i).to_string()),
                )
            })
            .collect()
    }

    fn col(batch: &RecordBatch, name: &str) -> Vec<Option<String>> {
        let c = batch.column_by_name(name).unwrap();
        let s = c.as_any().downcast_ref::<StringArray>().unwrap();
        (0..s.len())
            .map(|i| (!s.is_null(i)).then(|| s.value(i).to_string()))
            .collect()
    }

    #[test]
    fn a_key_still_in_the_map_moves_into_a_new_column() {
        let b = batch_with_map(
            &[
                Some(vec![("tenant", Some("jkt")), ("region", Some("id"))]),
                Some(vec![("region", Some("sg")), ("tenant", Some("sgp"))]),
            ],
            vec![],
        );
        let out = hoist_promoted(&b, &["tenant".to_string()]).unwrap();

        assert_eq!(
            col(&out, "tenant"),
            vec![Some("jkt".into()), Some("sgp".into())]
        );
        // and it is GONE from the map — this is what lets the manifest stop declaring the key
        assert_eq!(
            map_row(&out, 0),
            vec![("region".to_string(), Some("id".to_string()))]
        );
        assert_eq!(
            map_row(&out, 1),
            vec![("region".to_string(), Some("sg".to_string()))]
        );
        assert_eq!(out.num_rows(), 2);
    }

    #[test]
    fn a_key_with_an_all_null_column_is_filled_from_the_map() {
        // The state a real deployment sits in today: an earlier merge ran `align_to`, which
        // created the column as NULLs and left the values in the map.
        let nulls: ArrayRef = Arc::new(StringArray::from(vec![None as Option<&str>, None]));
        let b = batch_with_map(
            &[
                Some(vec![("tenant", Some("jkt"))]),
                Some(vec![("tenant", Some("sgp"))]),
            ],
            vec![(Field::new("tenant", DataType::Utf8, true), nulls)],
        );
        let out = hoist_promoted(&b, &["tenant".to_string()]).unwrap();
        assert_eq!(
            col(&out, "tenant"),
            vec![Some("jkt".into()), Some("sgp".into())]
        );
        assert!(map_row(&out, 0).is_empty());
        // The column was replaced in place, not appended twice.
        assert_eq!(out.num_columns(), b.num_columns());
    }

    #[test]
    fn a_key_the_writer_already_routed_is_left_alone() {
        let filled: ArrayRef = Arc::new(StringArray::from(vec![Some("jkt"), Some("sgp")]));
        let b = batch_with_map(
            &[
                Some(vec![("region", Some("id"))]),
                Some(vec![("region", Some("sg"))]),
            ],
            vec![(Field::new("tenant", DataType::Utf8, true), filled)],
        );
        let out = hoist_promoted(&b, &["tenant".to_string()]).unwrap();
        assert_eq!(out, b, "the WAL fast path must not rebuild the map");
    }

    #[test]
    fn a_candidate_that_is_not_in_the_map_costs_a_scan_and_nothing_else() {
        let b = batch_with_map(&[Some(vec![("region", Some("id"))])], vec![]);
        let out = hoist_promoted(&b, &["tenant".to_string()]).unwrap();
        assert_eq!(out, b);
    }

    #[test]
    fn null_map_rows_and_null_values_survive() {
        let b = batch_with_map(
            &[
                Some(vec![("tenant", Some("jkt")), ("region", None)]),
                None,
                Some(vec![("region", Some("sg"))]),
            ],
            vec![],
        );
        let out = hoist_promoted(&b, &["tenant".to_string()]).unwrap();
        assert_eq!(col(&out, "tenant"), vec![Some("jkt".into()), None, None]);
        assert_eq!(map_row(&out, 0), vec![("region".to_string(), None)]);
        assert!(map_row(&out, 1).is_empty());
        assert_eq!(
            map_row(&out, 2),
            vec![("region".to_string(), Some("sg".to_string()))]
        );
        let m = out.column_by_name(photon_core::schema::ATTRIBUTES).unwrap();
        assert!(m.is_null(1), "a NULL map row stays NULL");
    }

    #[test]
    fn hoisting_several_keys_at_once_keeps_them_apart() {
        let b = batch_with_map(
            &[
                Some(vec![
                    ("tenant", Some("jkt")),
                    ("http.response.status_code", Some("200")),
                    ("region", Some("id")),
                ]),
                Some(vec![("tenant", Some("sgp"))]),
            ],
            vec![],
        );
        let out = hoist_promoted(
            &b,
            &[
                "tenant".to_string(),
                "http.response.status_code".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(
            col(&out, "tenant"),
            vec![Some("jkt".into()), Some("sgp".into())]
        );
        assert_eq!(
            col(&out, "http.response.status_code"),
            vec![Some("200".into()), None]
        );
        assert_eq!(
            map_row(&out, 0),
            vec![("region".to_string(), Some("id".to_string()))]
        );
    }

    #[test]
    fn the_result_still_aligns_and_concatenates() {
        // The point of forcing the rebuilt map back to the source Map type: a hoisted batch has
        // to concat with one that needed no hoisting.
        let target = Arc::new(Schema::new(vec![
            Field::new("tenant", DataType::Utf8, true),
            Field::new(
                photon_core::schema::ATTRIBUTES,
                photon_core::schema::LogSchema::new(&[])
                    .arrow
                    .field_with_name(photon_core::schema::ATTRIBUTES)
                    .unwrap()
                    .data_type()
                    .clone(),
                true,
            ),
        ]));
        let a = batch_with_map(&[Some(vec![("tenant", Some("jkt"))])], vec![]);
        let b = batch_with_map(
            &[Some(vec![("region", Some("id"))])],
            vec![(
                Field::new("tenant", DataType::Utf8, true),
                Arc::new(StringArray::from(vec![Some("sgp")])) as ArrayRef,
            )],
        );
        let a = align_to(
            &hoist_promoted(&a, &["tenant".to_string()]).unwrap(),
            &target,
        )
        .unwrap();
        let b = align_to(
            &hoist_promoted(&b, &["tenant".to_string()]).unwrap(),
            &target,
        )
        .unwrap();
        let cat = arrow::compute::concat_batches(&target, &[a, b]).unwrap();
        assert_eq!(cat.num_rows(), 2);
        assert_eq!(
            col(&cat, "tenant"),
            vec![Some("jkt".into()), Some("sgp".into())]
        );
    }

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

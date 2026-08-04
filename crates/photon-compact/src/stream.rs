//! Signal-agnostic streaming-Parquet write + fsync durability helpers, shared by the logs,
//! spans, and metrics compactors. Extracted from the logs `Compactor` (B2/WS3) so all three
//! write paths get identical crash-consistency: stream ONE zstd Parquet file straight to the
//! hot store's backing directory (temp file + fsync + atomic rename + parent-dir fsync — no
//! whole-file `Vec<u8>`), and pin the just-saved manifest to disk before the point of no return.

use std::fs::File;
use std::path::{Path, PathBuf};

use arrow::datatypes::{DataType, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::WriterProperties;
use parquet::schema::types::ColumnPath;

use photon_core::PhotonError;
use photon_storage::Storage;

/// Default zstd compression level: matches the level `Compression::ZSTD(Default::default())`
/// hardcoded before the level became configurable — `parquet::basic::ZstdLevel::try_new(1)` equals
/// `ZstdLevel::default()` in parquet 53. Note this no longer implies byte-identical output to that
/// era: the per-column encodings in [`writer_properties`] changed independently of the level. Do
/// not raise it blindly — measured on a production corpus, **level 3 is ~3% LARGER than level 1**
/// here; the next level that actually pays is 6.
pub(crate) const DEFAULT_ZSTD_LEVEL: i32 = 1;

/// Cap on rows per Parquet row group, overriding parquet-rs's default of 1,048,576. Without this,
/// `ArrowWriter` buffers an entire row group's column data in memory before flushing it, so a large
/// sorted batch (bounded by `wal.segment_max_bytes`) can transiently hold up to a million rows of
/// decoded columns alongside the batch itself. 128k keeps that buffer small while still leaving
/// row-group pruning (min/max stats per group) reasonably coarse-grained.
pub(crate) const MAX_ROW_GROUP_SIZE: usize = 131_072;

/// How a signal's time columns should be encoded — a property of that signal's **sort key**, which
/// only the calling compactor knows.
///
/// Both variants were measured on a production corpus (relative to the previous
/// dictionary-everywhere default, same zstd level); the split is not a style preference:
///
/// | signal  | sort key                                          | `Delta` | `Dictionary` |
/// |---------|---------------------------------------------------|---------|--------------|
/// | logs    | `(service.name, timestamp)`                       | −18.3%  | baseline     |
/// | spans   | `(service.name, start_time)`                      | −11.5%  | baseline     |
/// | metrics | `(metric_name, service.name, host.name, timestamp)` | **+2.4%** | baseline   |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeEncoding {
    /// **Logs and spans.** Time is the *trailing* key of a two-part sort, so within each service
    /// run the column climbs monotonically and essentially every value is distinct: the dictionary
    /// spills and the column falls back to PLAIN, a flat 8 bytes/row that zstd barely dents
    /// (measured ratio 1.2x). `DELTA_BINARY_PACKED` stores the step instead of the stamp.
    Delta,
    /// **Metrics.** Timestamp is the *last* of four sort keys, so it restarts on every series
    /// rather than climbing — and one scrape stamps hundreds of rows with the **identical**
    /// instant. That makes the column highly repetitive, which is the dictionary's best case and
    /// delta's worst: forcing `Delta` here measured **2.4% larger**. Keep the defaults.
    Dictionary,
}

/// Top-level columns holding a **random hex identifier**, where parquet-rs's default
/// dictionary-first encoding is pure overhead: every value is distinct, so the dictionary is as
/// large as the data it indexes and buys nothing. Named rather than type-derived because they are
/// `Utf8` like every other string column — only their *content* makes them different. `trace_id` /
/// `span_id` are shared by the logs and spans schemas; `parent_span_id` is spans-only. Metrics has
/// none of them, so the lookup simply never matches there.
fn is_hex_id_column(name: &str) -> bool {
    name == photon_core::schema::TRACE_ID
        || name == photon_core::schema::SPAN_ID
        || name == photon_core::span_schema::PARENT_SPAN_ID
}

/// Build the Parquet writer properties for one signal's batch: zstd at `zstd_level`, capped row
/// groups, plus **per-column encodings chosen from the Arrow type**.
///
/// parquet-rs defaults every column to dictionary-first with a PLAIN fallback. That is right for
/// the low-cardinality string and enum columns (`service.name`, `severity_text`, `kind`, …) and
/// wrong for the two column shapes that dominate these files:
///
/// - **`Timestamp` / `Int64`** — the wall-clock and duration columns, but only under
///   [`TimeEncoding::Delta`]; see that type for why metrics opts out. On a production spans corpus
///   these three columns (`start_time_nanos`, `end_time_nanos`, `duration_nanos`) were **30% of
///   the file** at a compression ratio of 1.2x.
/// - **hex ids** — see [`is_hex_id_column`]; `DELTA_BYTE_ARRAY` at least shares the common prefix
///   length instead of paying for a useless dictionary. Another **33% of a spans file**. Applied
///   under both policies: it is a statement about the *content* being random, which no sort key
///   changes. Metrics has no such column, so the rule simply never fires there.
///
/// `Int32` is deliberately left on the dictionary path. In all three schemas it carries only
/// small enums — `severity_number`, `kind`, `status_code`, `metric_type`, `temporality` — where a
/// handful of distinct values RLE-compress to nearly nothing. Measured: restricting delta to
/// `Timestamp`/`Int64` instead of "all integers" is 0.4 pp *better* on logs and neutral on spans,
/// so the narrower rule costs nothing and keeps the dictionary where it genuinely wins. Floats
/// (metrics' `value`) and nested columns (the `attributes` map) keep the defaults —
/// `DELTA_BINARY_PACKED` is an integer-only encoding and would fail the write outright.
fn writer_properties(
    schema: &Schema,
    zstd_level: i32,
    time_encoding: TimeEncoding,
) -> Result<WriterProperties, PhotonError> {
    let level = parquet::basic::ZstdLevel::try_new(zstd_level).map_err(|e| {
        PhotonError::Config(format!("invalid storage.zstd_level {zstd_level}: {e}"))
    })?;
    let mut builder = WriterProperties::builder()
        .set_compression(Compression::ZSTD(level))
        .set_max_row_group_size(MAX_ROW_GROUP_SIZE);

    for field in schema.fields() {
        let encoding = match field.data_type() {
            DataType::Timestamp(_, _) | DataType::Int64 if time_encoding == TimeEncoding::Delta => {
                Encoding::DELTA_BINARY_PACKED
            }
            DataType::Utf8 if is_hex_id_column(field.name()) => Encoding::DELTA_BYTE_ARRAY,
            _ => continue,
        };
        // Both calls are required: `set_column_encoding` only names the *fallback* used once the
        // dictionary is out of the way, so leaving the dictionary enabled would keep it in front.
        let path = ColumnPath::from(field.name().as_str());
        builder = builder
            .set_column_dictionary_enabled(path.clone(), false)
            .set_column_encoding(path, encoding);
    }

    Ok(builder.build())
}

/// Resolve an object path to its real on-disk location under the hot store's local root, so a
/// blocking task can stream a Parquet encode straight to a `File`. The object path maps 1:1 onto
/// `<hot_dir>/<object_path>`, so the same hot store still serves it via `get`. Errors when the hot
/// store is not backed by a local directory (streamed compaction requires one).
pub(crate) fn hot_local_path(storage: &Storage, object_path: &str) -> Result<PathBuf, PhotonError> {
    let root = storage.hot_local_root().ok_or_else(|| {
        PhotonError::Storage(
            "hot store is not backed by a local directory; streamed compaction requires one"
                .to_string(),
        )
    })?;
    Ok(root.join(object_path))
}

/// fsync the just-saved manifest file's contents AND its parent directory entry, making both
/// durable before the caller removes a WAL segment / deletes superseded inputs. A no-op when the
/// hot store is not local (in-memory test stores). `manifest_object_path` is the per-signal
/// manifest object key (logs / spans / metrics).
pub(crate) async fn fsync_manifest(
    storage: &Storage,
    manifest_object_path: &str,
) -> Result<(), PhotonError> {
    let Some(root) = storage.hot_local_root() else {
        return Ok(());
    };
    let manifest_path = root.join(manifest_object_path);
    tokio::task::spawn_blocking(move || fsync_file_and_parent(&manifest_path))
        .await
        .map_err(|e| PhotonError::Io(format!("manifest fsync task panicked: {e}")))?
}

/// Stream a sorted batch to a zstd-compressed Parquet file at `target` via an `ArrowWriter` over a
/// `std::fs::File`, without ever holding the whole compressed file in RAM. Writes to a sibling
/// `.tmp` path in the SAME directory, fsyncs it, atomically renames it into place, then fsyncs the
/// parent directory so the rename itself is crash-durable — a crash mid-write can never leave a
/// torn file visible at `target`, and a crash after the rename can never lose it. The parent dir is
/// created first — a raw `std::fs` write, unlike `object_store::put`, does not auto-create parents.
/// `zstd_level` is the configured `[storage] zstd_level` (validated to `1..=19` at config load);
/// per-column encodings come from [`writer_properties`].
///
/// The bytes are **not** interchangeable with what older Photon versions wrote at the same level —
/// the encoding rules changed. That is a write-side change only: Parquet records each column's
/// encoding in its own metadata, so files written either way read back identically and no
/// migration is needed. Existing files simply keep their old (larger) encoding until retention
/// ages them out or a merge pass rewrites them.
pub(crate) fn write_parquet_streamed(
    target: &Path,
    batch: &RecordBatch,
    zstd_level: i32,
    time_encoding: TimeEncoding,
) -> Result<(), PhotonError> {
    let parent = target.parent().ok_or_else(|| {
        PhotonError::Io(format!("parquet target {target:?} has no parent directory"))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| PhotonError::Io(format!("failed to create {parent:?}: {e}")))?;

    let tmp = tmp_path(target);
    let file = File::create(&tmp)
        .map_err(|e| PhotonError::Io(format!("failed to create {tmp:?}: {e}")))?;

    let props = writer_properties(batch.schema_ref(), zstd_level, time_encoding)?;
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))
        .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    writer
        .write(batch)
        .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    let file = writer
        .into_inner()
        .map_err(|e| PhotonError::Arrow(e.to_string()))?;
    file.sync_all()
        .map_err(|e| PhotonError::Io(format!("failed to fsync {tmp:?}: {e}")))?;
    drop(file);

    std::fs::rename(&tmp, target)
        .map_err(|e| PhotonError::Io(format!("failed to rename {tmp:?} -> {target:?}: {e}")))?;
    fsync_dir(parent)?;
    Ok(())
}

/// fsync a directory so its recent entry changes (a `rename`/`create`) are durable.
fn fsync_dir(dir: &Path) -> Result<(), PhotonError> {
    let handle = File::open(dir)
        .map_err(|e| PhotonError::Io(format!("failed to open dir {dir:?} for fsync: {e}")))?;
    handle
        .sync_all()
        .map_err(|e| PhotonError::Io(format!("failed to fsync dir {dir:?}: {e}")))
}

/// fsync a file's contents AND its parent directory entry, making both durable.
fn fsync_file_and_parent(path: &Path) -> Result<(), PhotonError> {
    let file = File::open(path)
        .map_err(|e| PhotonError::Io(format!("failed to open {path:?} for fsync: {e}")))?;
    file.sync_all()
        .map_err(|e| PhotonError::Io(format!("failed to fsync {path:?}: {e}")))?;
    drop(file);
    if let Some(parent) = path.parent() {
        fsync_dir(parent)?;
    }
    Ok(())
}

/// Sibling temp path in the SAME directory as `target` (same-filesystem, so the rename is atomic).
fn tmp_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use arrow::array::{
        Float64Array, Int32Array, Int64Array, StringArray, TimestampNanosecondArray,
    };
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    fn sample_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![1, 2, 3]))]).unwrap()
    }

    fn read_back(target: &Path) -> Vec<RecordBatch> {
        let file = File::open(target).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        reader.map(|b| b.unwrap()).collect()
    }

    #[test]
    fn default_level_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(
            &target,
            &sample_batch(),
            DEFAULT_ZSTD_LEVEL,
            TimeEncoding::Delta,
        )
        .unwrap();

        let batches = read_back(&target);
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
    }

    /// A non-default level (still valid, `1..=19` per `Config::validate`) must also produce a
    /// readable Parquet file — the level only changes the codec's internal effort, not the
    /// logical rows the reader sees back.
    #[test]
    fn non_default_level_still_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(&target, &sample_batch(), 9, TimeEncoding::Delta).unwrap();

        let batches = read_back(&target);
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
    }

    #[test]
    fn out_of_range_level_errors_instead_of_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        let err =
            write_parquet_streamed(&target, &sample_batch(), 0, TimeEncoding::Delta).unwrap_err();
        assert!(err.to_string().contains("zstd_level"));
    }

    /// A batch shaped like a real spans/metrics file: the two column kinds whose encoding we
    /// override, plus the kinds that must be left alone.
    fn mixed_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "start_time_nanos",
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                false,
            ),
            Field::new("duration_nanos", DataType::Int64, true),
            Field::new("trace_id", DataType::Utf8, false),
            Field::new("span_id", DataType::Utf8, false),
            Field::new("parent_span_id", DataType::Utf8, true),
            // Low-cardinality enum + name: must KEEP the dictionary.
            Field::new("status_code", DataType::Int32, true),
            Field::new("name", DataType::Utf8, true),
            // Float: DELTA_BINARY_PACKED is integer-only, so this must not be touched.
            Field::new("value", DataType::Float64, true),
        ]));
        let n = 512i64;
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(TimestampNanosecondArray::from_iter_values(
                    (0..n).map(|i| 1_700_000_000_000_000_000 + i * 1_000_000),
                )),
                Arc::new(Int64Array::from_iter_values(0..n)),
                Arc::new(StringArray::from_iter_values(
                    (0..n).map(|i| format!("{i:032x}")),
                )),
                Arc::new(StringArray::from_iter_values(
                    (0..n).map(|i| format!("{i:016x}")),
                )),
                Arc::new(StringArray::from_iter_values(
                    (0..n).map(|i| format!("{:016x}", i / 4)),
                )),
                Arc::new(Int32Array::from_iter_values((0..n).map(|i| (i % 3) as i32))),
                Arc::new(StringArray::from_iter_values(
                    (0..n).map(|i| format!("op-{}", i % 8)),
                )),
                Arc::new(Float64Array::from_iter_values((0..n).map(|i| i as f64))),
            ],
        )
        .unwrap()
    }

    /// Encodings actually recorded in the written file, per column.
    fn encodings_of(target: &Path) -> std::collections::HashMap<String, Vec<Encoding>> {
        let file = File::open(target).unwrap();
        let md = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .metadata()
            .clone();
        let rg = md.row_group(0);
        (0..rg.num_columns())
            .map(|i| {
                let c = rg.column(i);
                (c.column_path().string(), c.encodings().clone())
            })
            .collect()
    }

    fn is_dictionary(e: &Encoding) -> bool {
        matches!(e, Encoding::RLE_DICTIONARY | Encoding::PLAIN_DICTIONARY)
    }

    /// The wall-clock/duration columns dominate these files and every value is distinct, so the
    /// default dictionary spills to PLAIN — a flat 8 bytes/row. Rows arrive in sort-key order, so
    /// `DELTA_BINARY_PACKED` is the encoding that actually exploits that. Locks in BOTH halves:
    /// the delta encoding is present AND no dictionary sits in front of it.
    #[test]
    fn timestamp_and_int64_columns_are_delta_encoded_without_a_dictionary() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(
            &target,
            &mixed_batch(),
            DEFAULT_ZSTD_LEVEL,
            TimeEncoding::Delta,
        )
        .unwrap();

        let encs = encodings_of(&target);
        for col in ["start_time_nanos", "duration_nanos"] {
            let e = &encs[col];
            assert!(
                e.contains(&Encoding::DELTA_BINARY_PACKED),
                "{col} should be DELTA_BINARY_PACKED, got {e:?}"
            );
            assert!(
                !e.iter().any(is_dictionary),
                "{col} should not carry a dictionary, got {e:?}"
            );
        }
    }

    /// Random hex ids are all-distinct, so a dictionary is as big as the data it indexes and buys
    /// nothing. On the production spans corpus these three columns were 33% of the file.
    #[test]
    fn hex_id_columns_drop_the_dictionary() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(
            &target,
            &mixed_batch(),
            DEFAULT_ZSTD_LEVEL,
            TimeEncoding::Delta,
        )
        .unwrap();

        let encs = encodings_of(&target);
        for col in ["trace_id", "span_id", "parent_span_id"] {
            let e = &encs[col];
            assert!(
                e.contains(&Encoding::DELTA_BYTE_ARRAY),
                "{col} should be DELTA_BYTE_ARRAY, got {e:?}"
            );
            assert!(
                !e.iter().any(is_dictionary),
                "{col} should not carry a dictionary, got {e:?}"
            );
        }
    }

    /// The other half of the rule, and the easier one to break: `Int32` in these schemas is only
    /// ever a small enum (`status_code`, `kind`, `severity_number`, `metric_type`, `temporality`)
    /// and low-cardinality strings are the dictionary's best case. Widening the delta rule to "all
    /// integers" or "all columns" would silently regress exactly these.
    #[test]
    fn low_cardinality_enum_and_string_columns_keep_their_dictionary() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(
            &target,
            &mixed_batch(),
            DEFAULT_ZSTD_LEVEL,
            TimeEncoding::Delta,
        )
        .unwrap();

        let encs = encodings_of(&target);
        for col in ["status_code", "name"] {
            let e = &encs[col];
            assert!(
                e.iter().any(is_dictionary),
                "{col} should still be dictionary-encoded, got {e:?}"
            );
            assert!(
                !e.contains(&Encoding::DELTA_BINARY_PACKED),
                "{col} should not be delta-encoded, got {e:?}"
            );
        }
    }

    /// The metrics policy, and the reason it exists. Metrics sorts timestamp LAST
    /// (`metric_name, service.name, host.name, timestamp`) and one scrape stamps many rows with
    /// the identical instant, so the column is repetitive rather than climbing — the dictionary's
    /// best case. Measured on a production corpus, forcing delta here was **2.4% larger**. If a
    /// future refactor "unifies" the two policies, this test is what should stop it.
    #[test]
    fn metrics_policy_keeps_the_dictionary_on_time_columns() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(
            &target,
            &mixed_batch(),
            DEFAULT_ZSTD_LEVEL,
            TimeEncoding::Dictionary,
        )
        .unwrap();

        let encs = encodings_of(&target);
        for col in ["start_time_nanos", "duration_nanos"] {
            let e = &encs[col];
            assert!(
                e.iter().any(is_dictionary),
                "{col} should keep its dictionary under the metrics policy, got {e:?}"
            );
            assert!(
                !e.contains(&Encoding::DELTA_BINARY_PACKED),
                "{col} should not be delta-encoded under the metrics policy, got {e:?}"
            );
        }
        // The hex-id rule is about content, not sort order, so it still applies. (Metrics has no
        // such column in practice; this pins the policy split to time columns only.)
        assert!(!encs["trace_id"].iter().any(is_dictionary));
    }

    /// `DELTA_BINARY_PACKED` is integer-only — applying it to metrics' `value` would fail the
    /// write outright. Round-tripping a float column proves the rule skips it.
    #[test]
    fn float_columns_round_trip_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(
            &target,
            &mixed_batch(),
            DEFAULT_ZSTD_LEVEL,
            TimeEncoding::Delta,
        )
        .unwrap();

        let batches = read_back(&target);
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 512);
        assert!(!encodings_of(&target)["value"].contains(&Encoding::DELTA_BINARY_PACKED));
    }

    /// The encodings are a write-side choice recorded per column in the file's own metadata, so a
    /// re-encoded file must hand back byte-for-byte the same logical rows.
    #[test]
    fn re_encoded_columns_round_trip_to_identical_values() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        let batch = mixed_batch();
        write_parquet_streamed(&target, &batch, DEFAULT_ZSTD_LEVEL, TimeEncoding::Delta).unwrap();

        let read = arrow::compute::concat_batches(&batch.schema(), &read_back(&target)).unwrap();
        assert_eq!(read, batch);
    }

    /// A batch bigger than one row group's worth of rows must still be split into multiple row
    /// groups on write — otherwise the `ArrowWriter` buffers the whole batch's column data in one
    /// in-progress row group (parquet-rs's default `max_row_group_size` is 1,048,576 rows), which
    /// is exactly the peak-RSS knob this test locks in. 262,145 rows is just over 2x the intended
    /// cap (131,072) so a correct writer must emit at least 3 row groups, none larger than the cap.
    #[test]
    fn write_parquet_streamed_caps_row_group_size() {
        let cap = MAX_ROW_GROUP_SIZE as i64;
        let n: i64 = cap * 2 + 1;

        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from_iter_values(0..n))])
                .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.parquet");
        write_parquet_streamed(&target, &batch, DEFAULT_ZSTD_LEVEL, TimeEncoding::Delta).unwrap();

        let file = File::open(&target).unwrap();
        let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        let row_groups = reader_builder.metadata().row_groups();
        assert!(
            !row_groups.is_empty(),
            "expected at least one row group in the written file"
        );
        for rg in row_groups {
            assert!(
                rg.num_rows() <= cap,
                "row group has {} rows, expected <= {cap}",
                rg.num_rows()
            );
        }
    }
}

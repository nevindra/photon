//! Metric discovery surfaces: `catalog` (all metrics + type/unit/last-seen), `metadata` (one
//! metric's detail), `labels` (label keys/values for autocomplete). Bounded query-time scans over
//! time-pruned candidate files — NOT precomputed manifest rollups (a deferred optimization). Label
//! *keys* are the manifest `attribute_keys` union (a per-file superset), documented as such.

use std::collections::BTreeSet;

use arrow::array::{Array, BooleanArray, Int32Array, Int64Array, StringArray, UInt64Array};
use arrow::datatypes::DataType;
use datafusion::functions_aggregate::expr_fn::{approx_distinct, bool_or, count, max, min};
use datafusion::prelude::{cast, col, concat_ws, lit, strpos, Expr};

use photon_core::metric_schema;
use photon_core::query::MetricFieldResolver;
use photon_core::PhotonError;

use crate::col_ref;
use crate::metric_engine::MetricRequest;
use crate::metric_predicate::metric_field_col;
use crate::MetricsQueryEngine;

#[derive(Clone)]
pub struct MetricCatalogEntry {
    pub name: String,
    pub metric_type: i32,
    pub type_text: Option<String>,
    pub unit: Option<String>,
    pub temporality: Option<i32>,
    pub is_monotonic: Option<bool>,
    pub last_seen_nanos: i64,
    pub series_count: u64,
}

pub struct MetricMetadata {
    pub name: String,
    pub metric_type: i32,
    pub type_text: Option<String>,
    pub temporality: Option<i32>,
    pub is_monotonic: Option<bool>,
    pub unit: Option<String>,
    pub last_seen_nanos: i64,
    pub series_count: u64,
    pub attribute_keys: Vec<String>,
}

pub enum LabelsResult {
    Keys(Vec<String>),
    Values { values: Vec<String>, capped: bool },
}

const LABEL_VALUES_CAP: usize = 1000;

/// `concat_ws(0x1f, <promoted label columns>)` — a per-row series fingerprint for
/// `approx_distinct` cardinality. Undercounts series distinguished only by map-tail attributes
/// (documented estimate). `service.name` is always present in `promoted`.
fn series_fingerprint(promoted: &[String]) -> Expr {
    let sep = lit("\u{1f}");
    let cols: Vec<Expr> = promoted.iter().map(|p| col_ref(p)).collect();
    concat_ws(sep, cols)
}

/// Collapse Prometheus classic-histogram families into a single `HISTOGRAM`-typed entry per base.
/// A base `B` is detected when an entry named `B_bucket` is present; its `B_bucket`/`B_sum`/`B_count`
/// entries are removed and replaced by one entry `{ name: B, metric_type: HISTOGRAM, .. }` carrying
/// the bucket entry's unit and the family's max `last_seen`. Non-family metrics pass through
/// unchanged, order-stably. Reuses the same suffix convention as ingest classification.
fn fold_classic_histograms(entries: Vec<MetricCatalogEntry>) -> Vec<MetricCatalogEntry> {
    use crate::metric_classic_hist::classic_base;
    use photon_core::metric_schema::metric_type;
    use std::collections::HashSet;

    // Bases that actually have a `_bucket` series.
    let bases: HashSet<&str> = entries
        .iter()
        .filter_map(|e| classic_base(&e.name))
        .collect();

    let is_family_member = |name: &str| -> Option<String> {
        for suffix in ["_bucket", "_sum", "_count"] {
            if let Some(base) = name.strip_suffix(suffix) {
                if bases.contains(base) {
                    return Some(base.to_string());
                }
            }
        }
        None
    };

    let mut out: Vec<MetricCatalogEntry> = Vec::with_capacity(entries.len());
    let mut emitted: HashSet<String> = HashSet::new();
    for e in entries.iter() {
        match is_family_member(&e.name) {
            Some(base) => {
                if emitted.insert(base.clone()) {
                    // Synthesize the folded histogram entry once, positioned where the family first appears.
                    // Prefer the `_bucket` entry's unit; fall back to whatever this member carries.
                    let bucket_unit = entries
                        .iter()
                        .find(|x| x.name == format!("{base}_bucket"))
                        .and_then(|x| x.unit.clone());
                    let last_seen = entries
                        .iter()
                        .filter(|x| is_family_member(&x.name).as_deref() == Some(base.as_str()))
                        .map(|x| x.last_seen_nanos)
                        .max()
                        .unwrap_or(e.last_seen_nanos);
                    let series_count = entries
                        .iter()
                        .find(|x| x.name == format!("{base}_bucket"))
                        .map(|x| x.series_count)
                        .unwrap_or(0);
                    out.push(MetricCatalogEntry {
                        name: base,
                        metric_type: metric_type::HISTOGRAM,
                        type_text: Some("HISTOGRAM".to_string()),
                        unit: bucket_unit,
                        temporality: Some(crate::metric_query::TEMPORALITY_CUMULATIVE),
                        is_monotonic: Some(true),
                        last_seen_nanos: last_seen,
                        series_count,
                    });
                }
                // else: drop the raw family member.
            }
            None => out.push(e.clone()),
        }
    }
    out
}

impl MetricsQueryEngine {
    pub async fn catalog(
        &self,
        start_ts_nanos: i64,
        end_ts_nanos: i64,
        search: Option<&str>,
        type_filter: Option<i32>,
    ) -> Result<Vec<MetricCatalogEntry>, PhotonError> {
        let Some(df) = self.time_survivors_df(start_ts_nanos, end_ts_nanos).await? else {
            return Ok(Vec::new());
        };
        let ts = col_ref(metric_schema::TIMESTAMP);
        let mut filter = cast(ts.clone(), DataType::Int64)
            .gt_eq(lit(start_ts_nanos))
            .and(cast(ts, DataType::Int64).lt_eq(lit(end_ts_nanos)));
        // NOTE: `type_filter` is intentionally NOT applied here (pre-aggregation) — folding a
        // classic-histogram family (`<base>_bucket`/`_sum`/`_count`, all stored as SUM) into one
        // HISTOGRAM entry needs to see the whole family. It's applied post-fold below instead.
        if let Some(s) = search {
            // case-sensitive substring on metric_name (matches the grammar's case-sensitivity).
            filter = filter.and(
                strpos(col_ref(metric_schema::METRIC_NAME), lit(s.to_string())).gt(lit(0_i64)),
            );
        }

        let batches = df
            .filter(filter)
            .map_err(|e| PhotonError::Query(format!("catalog filter: {e}")))?
            .aggregate(
                vec![col_ref(metric_schema::METRIC_NAME).alias("name")],
                vec![
                    min(col_ref(metric_schema::METRIC_TYPE)).alias("metric_type"),
                    min(col_ref(metric_schema::TYPE_TEXT)).alias("type_text"),
                    min(col_ref(metric_schema::UNIT)).alias("unit"),
                    min(col_ref(metric_schema::TEMPORALITY)).alias("temporality"),
                    bool_or(col_ref(metric_schema::IS_MONOTONIC)).alias("is_monotonic"),
                    max(cast(col_ref(metric_schema::TIMESTAMP), DataType::Int64))
                        .alias("last_seen"),
                    approx_distinct(series_fingerprint(self.promoted_attributes()))
                        .alias("series_count"),
                ],
            )
            .map_err(|e| PhotonError::Query(format!("catalog aggregate: {e}")))?
            .sort(vec![col("name").sort(true, false)])
            .map_err(|e| PhotonError::Query(format!("catalog sort: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("catalog collect: {e}")))?;

        let mut out = Vec::new();
        for b in &batches {
            let name = str_col(b, 0)?;
            let mtype = i32_col(b, 1)?;
            let type_text = str_col(b, 2)?;
            let unit = str_col(b, 3)?;
            let temporality = i32_col(b, 4)?;
            let is_monotonic = bool_col(b, 5)?;
            let last_seen = i64_col(b, 6)?;
            let series_count = u64_col(b, 7)?;
            for i in 0..b.num_rows() {
                out.push(MetricCatalogEntry {
                    name: name.value(i).to_string(),
                    metric_type: mtype.value(i),
                    type_text: opt_str(type_text, i),
                    unit: opt_str(unit, i),
                    temporality: opt_i32(temporality, i),
                    is_monotonic: opt_bool(is_monotonic, i),
                    last_seen_nanos: last_seen.value(i),
                    series_count: series_count.value(i),
                });
            }
        }
        let folded = fold_classic_histograms(out);
        let folded = match type_filter {
            Some(t) => folded.into_iter().filter(|e| e.metric_type == t).collect(),
            None => folded,
        };
        Ok(folded)
    }

    pub async fn metadata(
        &self,
        name: &str,
        start_ts_nanos: i64,
        end_ts_nanos: i64,
    ) -> Result<Option<MetricMetadata>, PhotonError> {
        let base = MetricRequest {
            metric: name.to_string(),
            start_ts_nanos,
            end_ts_nanos,
            filter: None,
            host: None,
        };
        // NOTE: a classic-histogram base (e.g. `http_req_duration_seconds`) never appears as a
        // literal `metric_name` value — only its `_bucket`/`_sum`/`_count` family does — and the
        // metrics skip-index bloom is exact-match (not tokenized, see photon-index::build_metrics),
        // so `survivors_df` legitimately returns `None` for it. That means this first early return
        // is the one actually hit on the classic-histogram path, not just the `n == 0` case below;
        // both (and the empty-batches case) must fall back to `classic_histogram_metadata`.
        let Some(df) = self.survivors_df(&base).await? else {
            return self
                .classic_histogram_metadata(name, start_ts_nanos, end_ts_nanos)
                .await;
        };
        let batches = df
            .filter(crate::metric_engine::metric_base_predicate(&base))
            .map_err(|e| PhotonError::Query(format!("metadata filter: {e}")))?
            .aggregate(
                vec![],
                vec![
                    count(lit(1i64)).alias("n"),
                    min(col_ref(metric_schema::METRIC_TYPE)).alias("metric_type"),
                    min(col_ref(metric_schema::TYPE_TEXT)).alias("type_text"),
                    min(col_ref(metric_schema::TEMPORALITY)).alias("temporality"),
                    bool_or(col_ref(metric_schema::IS_MONOTONIC)).alias("is_monotonic"),
                    min(col_ref(metric_schema::UNIT)).alias("unit"),
                    max(cast(col_ref(metric_schema::TIMESTAMP), DataType::Int64))
                        .alias("last_seen"),
                    approx_distinct(series_fingerprint(self.promoted_attributes()))
                        .alias("series_count"),
                ],
            )
            .map_err(|e| PhotonError::Query(format!("metadata aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("metadata collect: {e}")))?;

        let Some(b) = batches.iter().find(|b| b.num_rows() > 0) else {
            return self
                .classic_histogram_metadata(name, start_ts_nanos, end_ts_nanos)
                .await;
        };
        let n = i64_col(b, 0)?.value(0);
        if n == 0 {
            // metric not present in window — maybe it's a classic-histogram base.
            return self
                .classic_histogram_metadata(name, start_ts_nanos, end_ts_nanos)
                .await;
        }
        let metric_type = i32_col(b, 1)?.value(0);
        let type_text = opt_str(str_col(b, 2)?, 0);
        let temporality = opt_i32(i32_col(b, 3)?, 0);
        let is_monotonic = opt_bool(bool_col(b, 4)?, 0);
        let unit = opt_str(str_col(b, 5)?, 0);
        let last_seen_nanos = i64_col(b, 6)?.value(0);
        let series_count = u64_col(b, 7)?.value(0);

        Ok(Some(MetricMetadata {
            name: name.to_string(),
            metric_type,
            type_text,
            temporality,
            is_monotonic,
            unit,
            last_seen_nanos,
            series_count,
            attribute_keys: self.metric_attribute_keys(name, start_ts_nanos, end_ts_nanos)?,
        }))
    }

    /// `name` may be a Prometheus classic-histogram base with no direct rows of its own — only
    /// `<name>_bucket`/`_sum`/`_count` exist. Probe the bucket series' own metadata and, if
    /// present, resynthesize a `HISTOGRAM`-typed entry from it (dropping `le` from the advertised
    /// label keys — it's consumed by the aggregation, not a user-facing dimension). `None` if even
    /// the bucket series has no rows in the window, i.e. `name` isn't a metric at all.
    ///
    /// Boxed: this recurses into `metadata()`, which calls back into this fn — an async-fn cycle
    /// needs one boxed indirection to have a finite-sized future.
    ///
    /// Guarded against unbounded recursion: a name that already carries a classic-histogram family
    /// suffix (`_bucket`/`_sum`/`_count`) is a family LEAF, never itself a plausible base — trying
    /// `<name>_bucket` on it would recurse toward `..._bucket_bucket_bucket...` forever whenever the
    /// probed metric genuinely doesn't exist (e.g. `metadata("does.not.exist")` bottoms out at
    /// `does.not.exist_bucket`, which must terminate here rather than trying
    /// `does.not.exist_bucket_bucket`).
    fn classic_histogram_metadata<'a>(
        &'a self,
        name: &'a str,
        start_ts_nanos: i64,
        end_ts_nanos: i64,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<MetricMetadata>, PhotonError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if name.ends_with("_bucket") || name.ends_with("_sum") || name.ends_with("_count") {
                return Ok(None);
            }
            use crate::metric_classic_hist::bucket_name;
            use photon_core::metric_schema::metric_type;

            let Some(bucket_meta) = self
                .metadata(&bucket_name(name), start_ts_nanos, end_ts_nanos)
                .await?
            else {
                return Ok(None);
            };
            Ok(Some(MetricMetadata {
                name: name.to_string(),
                metric_type: metric_type::HISTOGRAM,
                type_text: Some("HISTOGRAM".to_string()),
                temporality: Some(crate::metric_query::TEMPORALITY_CUMULATIVE),
                is_monotonic: Some(true),
                unit: bucket_meta.unit,
                last_seen_nanos: bucket_meta.last_seen_nanos,
                series_count: bucket_meta.series_count,
                attribute_keys: bucket_meta
                    .attribute_keys
                    .into_iter()
                    .filter(|k| k != "le")
                    .collect(),
            }))
        })
    }

    /// Union of `attribute_keys` over files that pass (time overlap + metric_name bloom). A
    /// per-file superset (keys of *other* metrics in the same files may appear) — good enough for
    /// autocomplete, and cheap (manifest + `.idx` only, no Parquet scan). Runs sync fs I/O.
    fn metric_attribute_keys(
        &self,
        name: &str,
        start_ts_nanos: i64,
        end_ts_nanos: i64,
    ) -> Result<Vec<String>, PhotonError> {
        let req = MetricRequest {
            metric: name.to_string(),
            start_ts_nanos,
            end_ts_nanos,
            filter: None,
            host: None,
        };
        let manifest = self.load_metrics_manifest()?;
        let mut keys: BTreeSet<String> = BTreeSet::new();
        keys.insert("service".to_string());
        // Reuse prune() to get surviving files, then map each back to its FileEntry's keys.
        let kept = self.prune(&req)?;
        for entry in manifest.candidates(start_ts_nanos, end_ts_nanos) {
            let p = self
                .hot_dir
                .join(&entry.path)
                .to_string_lossy()
                .into_owned();
            if kept.contains(&p) {
                for k in &entry.attribute_keys {
                    keys.insert(k.clone());
                }
            }
        }
        Ok(keys.into_iter().collect())
    }

    pub async fn labels(
        &self,
        metric: &str,
        key: Option<&str>,
        start_ts_nanos: i64,
        end_ts_nanos: i64,
    ) -> Result<LabelsResult, PhotonError> {
        let Some(key) = key else {
            return Ok(LabelsResult::Keys(self.metric_attribute_keys(
                metric,
                start_ts_nanos,
                end_ts_nanos,
            )?));
        };

        let base = MetricRequest {
            metric: metric.to_string(),
            start_ts_nanos,
            end_ts_nanos,
            filter: None,
            host: None,
        };
        let Some(df) = self.survivors_df(&base).await? else {
            return Ok(LabelsResult::Values {
                values: Vec::new(),
                capped: false,
            });
        };
        let fr = MetricFieldResolver::new(self.promoted_attributes())
            .resolve_field_name(key)
            .map_err(|e| {
                PhotonError::Query(format!("cannot list values for `{key}`: {}", e.message))
            })?;
        let value_expr = metric_field_col(&fr);

        // DISTINCT non-null values ordered by count desc, fetch cap+1 to detect truncation.
        let batches = df
            .filter(crate::metric_engine::metric_base_predicate(&base))
            .map_err(|e| PhotonError::Query(format!("labels filter: {e}")))?
            .aggregate(
                vec![value_expr.alias("value")],
                vec![count(lit(1i64)).alias("n")],
            )
            .map_err(|e| PhotonError::Query(format!("labels aggregate: {e}")))?
            .filter(col("value").is_not_null())
            .map_err(|e| PhotonError::Query(format!("labels not-null: {e}")))?
            .sort(vec![
                col("n").sort(false, false),
                col("value").sort(true, false),
            ])
            .map_err(|e| PhotonError::Query(format!("labels sort: {e}")))?
            .limit(0, Some(LABEL_VALUES_CAP + 1))
            .map_err(|e| PhotonError::Query(format!("labels limit: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("labels collect: {e}")))?;

        let mut values = Vec::new();
        for b in &batches {
            let c = str_col(b, 0)?;
            for i in 0..b.num_rows() {
                values.push(c.value(i).to_string());
            }
        }
        let capped = values.len() > LABEL_VALUES_CAP;
        values.truncate(LABEL_VALUES_CAP);
        Ok(LabelsResult::Values { values, capped })
    }
}

// --- column decode helpers (downcast + null handling) ---
fn str_col(b: &arrow::array::RecordBatch, i: usize) -> Result<&StringArray, PhotonError> {
    b.column(i)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PhotonError::Query(format!("catalog: column {i} not Utf8")))
}
fn i32_col(b: &arrow::array::RecordBatch, i: usize) -> Result<&Int32Array, PhotonError> {
    b.column(i)
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or_else(|| PhotonError::Query(format!("catalog: column {i} not Int32")))
}
fn i64_col(b: &arrow::array::RecordBatch, i: usize) -> Result<&Int64Array, PhotonError> {
    b.column(i)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| PhotonError::Query(format!("catalog: column {i} not Int64")))
}
fn u64_col(b: &arrow::array::RecordBatch, i: usize) -> Result<&UInt64Array, PhotonError> {
    b.column(i)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(|| PhotonError::Query(format!("catalog: column {i} not UInt64")))
}
fn bool_col(b: &arrow::array::RecordBatch, i: usize) -> Result<&BooleanArray, PhotonError> {
    b.column(i)
        .as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or_else(|| PhotonError::Query(format!("catalog: column {i} not Bool")))
}
fn opt_str(c: &StringArray, i: usize) -> Option<String> {
    (!c.is_null(i)).then(|| c.value(i).to_string())
}
fn opt_i32(c: &Int32Array, i: usize) -> Option<i32> {
    (!c.is_null(i)).then(|| c.value(i))
}
fn opt_bool(c: &BooleanArray, i: usize) -> Option<bool> {
    (!c.is_null(i)).then(|| c.value(i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use arrow::array::RecordBatch;
    use object_store::local::LocalFileSystem;
    use photon_compact::MetricsCompactor;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::metric_schema::MetricSchema;
    use photon_core::segment::SegmentId;
    use photon_storage::{Replicator, Storage};
    use photon_wal::Wal;

    /// Minimal in-memory WAL that hands the compactor pre-built segments, so the test controls
    /// segment ids deterministically. Mirrors the `FakeWal` in `metric_engine.rs`'s own tests.
    struct FakeWal {
        segments: Mutex<Vec<(SegmentId, Vec<RecordBatch>)>>,
    }
    #[allow(clippy::manual_async_fn)]
    impl Wal for FakeWal {
        fn append(
            &self,
            _b: RecordBatch,
        ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!() }
        }
        fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!() }
        }
        fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
            let mut ids: Vec<SegmentId> = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .map(|(id, _)| *id)
                .collect();
            ids.sort();
            Ok(ids)
        }
        fn read_segment(
            &self,
            id: SegmentId,
        ) -> impl std::future::Future<Output = Result<Vec<RecordBatch>, PhotonError>> + Send
        {
            let batches = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .find(|(sid, _)| *sid == id)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            async move { Ok(batches) }
        }
        fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError> {
            self.segments.lock().unwrap().retain(|(sid, _)| *sid != id);
            Ok(())
        }
    }

    fn point(name: &str, ts: i64, service: &str, metric_type: i32, unit: &str) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), service.to_string());
        attributes.insert("region".to_string(), "us-east".to_string());
        MetricPoint {
            metric_name: name.to_string(),
            timestamp_nanos: ts,
            value: Some(1.0),
            metric_type,
            unit: Some(unit.to_string()),
            attributes,
            ..Default::default()
        }
    }

    fn batch(schema: &MetricSchema, points: &[MetricPoint]) -> RecordBatch {
        let mut b = MetricBatchBuilder::new(schema);
        for p in points {
            b.append(p);
        }
        b.finish().unwrap()
    }

    /// Drive the real `MetricsCompactor` over the given `FakeWal` segments to produce genuine
    /// `data-metrics/<stem>.parquet` + `.idx` sidecars and a metrics manifest under `hot`.
    async fn compact(
        hot: &std::path::Path,
        schema: &MetricSchema,
        segments: Vec<(SegmentId, Vec<RecordBatch>)>,
    ) {
        let storage = Storage {
            hot: Arc::new(LocalFileSystem::new_with_prefix(hot).unwrap()),
            durable: None,
            hot_dir: Some(hot.to_path_buf()),
        };
        let wal = Arc::new(FakeWal {
            segments: Mutex::new(segments),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal, storage, replicator, schema.clone());
        while compactor.run_once().await.unwrap().is_some() {}
    }

    async fn fixture() -> (tempfile::TempDir, MetricsQueryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);

        compact(
            &hot,
            &schema,
            vec![
                (
                    SegmentId(0),
                    vec![batch(
                        &schema,
                        &[
                            point(
                                "cpu.usage",
                                10,
                                "checkout",
                                metric_schema::metric_type::GAUGE,
                                "percent",
                            ),
                            point(
                                "cpu.usage",
                                20,
                                "cart",
                                metric_schema::metric_type::GAUGE,
                                "percent",
                            ),
                        ],
                    )],
                ),
                (
                    SegmentId(1),
                    vec![batch(
                        &schema,
                        &[point(
                            "mem.usage",
                            30,
                            "checkout",
                            metric_schema::metric_type::SUM,
                            "bytes",
                        )],
                    )],
                ),
            ],
        )
        .await;

        let engine = MetricsQueryEngine::new(hot, schema).unwrap();
        (dir, engine)
    }

    #[tokio::test]
    async fn catalog_lists_each_metric_once_with_type_and_unit() {
        let (_dir, engine) = fixture().await;
        let entries = engine.catalog(0, 100, None, None).await.unwrap();
        assert_eq!(entries.len(), 2, "two distinct metric names");
        let cpu = entries.iter().find(|e| e.name == "cpu.usage").unwrap();
        assert_eq!(cpu.metric_type, metric_schema::metric_type::GAUGE);
        assert_eq!(cpu.unit.as_deref(), Some("percent"));
        assert_eq!(cpu.last_seen_nanos, 20, "max ts across cpu.usage points");
        assert!(cpu.series_count >= 1);

        let mem = entries.iter().find(|e| e.name == "mem.usage").unwrap();
        assert_eq!(mem.metric_type, metric_schema::metric_type::SUM);
        assert_eq!(mem.unit.as_deref(), Some("bytes"));
        assert_eq!(mem.last_seen_nanos, 30);
    }

    #[tokio::test]
    async fn catalog_search_filters_by_substring() {
        let (_dir, engine) = fixture().await;
        let entries = engine.catalog(0, 100, Some("cpu"), None).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "cpu.usage");

        let none = engine.catalog(0, 100, Some("disk"), None).await.unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn catalog_type_filter_filters_by_metric_type() {
        let (_dir, engine) = fixture().await;
        let sums = engine
            .catalog(0, 100, None, Some(metric_schema::metric_type::SUM))
            .await
            .unwrap();
        assert_eq!(sums.len(), 1);
        assert_eq!(sums[0].name, "mem.usage");

        let gauges = engine
            .catalog(0, 100, None, Some(metric_schema::metric_type::GAUGE))
            .await
            .unwrap();
        assert_eq!(gauges.len(), 1);
        assert_eq!(gauges[0].name, "cpu.usage");
    }

    #[tokio::test]
    async fn metadata_unknown_metric_is_none() {
        let (_dir, engine) = fixture().await;
        assert!(engine
            .metadata("does.not.exist", 0, 100)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn metadata_known_metric_has_populated_attribute_keys() {
        let (_dir, engine) = fixture().await;
        let meta = engine.metadata("cpu.usage", 0, 100).await.unwrap().unwrap();
        assert_eq!(meta.name, "cpu.usage");
        assert_eq!(meta.metric_type, metric_schema::metric_type::GAUGE);
        assert_eq!(meta.unit.as_deref(), Some("percent"));
        assert_eq!(meta.last_seen_nanos, 20);
        assert!(meta.attribute_keys.contains(&"service".to_string()));
        assert!(meta.attribute_keys.contains(&"region".to_string()));
    }

    /// Classic-histogram fixture: `<base>_bucket` (with `le`), `_sum`, `_count`, all stored as
    /// SUM-typed series, plus an unrelated gauge — mirrors real Prometheus remote-write shape.
    async fn classic_histogram_fixture() -> (tempfile::TempDir, MetricsQueryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]);

        fn bucket_point(le: &str, ts: i64) -> MetricPoint {
            let mut attributes = std::collections::BTreeMap::new();
            attributes.insert("service.name".to_string(), "checkout".to_string());
            attributes.insert("le".to_string(), le.to_string());
            MetricPoint {
                metric_name: "http_req_duration_seconds_bucket".to_string(),
                timestamp_nanos: ts,
                value: Some(1.0),
                metric_type: metric_schema::metric_type::SUM,
                unit: Some("seconds".to_string()),
                attributes,
                ..Default::default()
            }
        }
        fn family_point(name: &str, ts: i64) -> MetricPoint {
            let mut attributes = std::collections::BTreeMap::new();
            attributes.insert("service.name".to_string(), "checkout".to_string());
            MetricPoint {
                metric_name: name.to_string(),
                timestamp_nanos: ts,
                value: Some(1.0),
                metric_type: metric_schema::metric_type::SUM,
                unit: Some("seconds".to_string()),
                attributes,
                ..Default::default()
            }
        }

        compact(
            &hot,
            &schema,
            vec![(
                SegmentId(0),
                vec![batch(
                    &schema,
                    &[
                        bucket_point("0.5", 10),
                        bucket_point("+Inf", 20),
                        family_point("http_req_duration_seconds_sum", 15),
                        family_point("http_req_duration_seconds_count", 15),
                        point(
                            "cpu.usage",
                            5,
                            "checkout",
                            metric_schema::metric_type::GAUGE,
                            "percent",
                        ),
                    ],
                )],
            )],
        )
        .await;

        let engine = MetricsQueryEngine::new(hot, schema).unwrap();
        (dir, engine)
    }

    #[tokio::test]
    async fn catalog_folds_classic_histogram_family_and_applies_type_filter_post_fold() {
        let (_dir, engine) = classic_histogram_fixture().await;

        let all = engine.catalog(0, 100, None, None).await.unwrap();
        assert_eq!(
            all.len(),
            2,
            "bucket/sum/count collapse into one histogram entry, plus the gauge"
        );
        let hist = all
            .iter()
            .find(|e| e.name == "http_req_duration_seconds")
            .expect("folded histogram entry present");
        assert_eq!(hist.metric_type, metric_schema::metric_type::HISTOGRAM);
        assert_eq!(hist.unit.as_deref(), Some("seconds"));
        assert!(all.iter().any(|e| e.name == "cpu.usage"));

        // type=histogram must see the synthesized entry (fold happens before the filter).
        let hist_only = engine
            .catalog(0, 100, None, Some(metric_schema::metric_type::HISTOGRAM))
            .await
            .unwrap();
        assert_eq!(hist_only.len(), 1);
        assert_eq!(hist_only[0].name, "http_req_duration_seconds");

        // type=sum must NOT see the raw bucket/sum/count family — it was folded away.
        let sum_only = engine
            .catalog(0, 100, None, Some(metric_schema::metric_type::SUM))
            .await
            .unwrap();
        assert!(
            sum_only.is_empty(),
            "raw SUM-typed family members must be hidden post-fold, got {:?}",
            sum_only.iter().map(|e| &e.name).collect::<Vec<_>>()
        );

        // type=gauge is unaffected by the fold.
        let gauge_only = engine
            .catalog(0, 100, None, Some(metric_schema::metric_type::GAUGE))
            .await
            .unwrap();
        assert_eq!(gauge_only.len(), 1);
        assert_eq!(gauge_only[0].name, "cpu.usage");
    }

    #[tokio::test]
    async fn metadata_synthesizes_histogram_for_classic_base() {
        let (_dir, engine) = classic_histogram_fixture().await;

        let meta = engine
            .metadata("http_req_duration_seconds", 0, 100)
            .await
            .unwrap()
            .expect("classic-histogram base synthesized from its _bucket family");
        assert_eq!(meta.name, "http_req_duration_seconds");
        assert_eq!(meta.metric_type, metric_schema::metric_type::HISTOGRAM);
        assert_eq!(meta.unit.as_deref(), Some("seconds"));
        assert_eq!(meta.last_seen_nanos, 20, "max ts of the _bucket series");
        assert!(
            !meta.attribute_keys.iter().any(|k| k == "le"),
            "le is consumed by the aggregation, not an advertised label key"
        );
        assert!(meta.attribute_keys.contains(&"service".to_string()));

        // The raw bucket series is still independently queryable (used internally by percentile
        // evaluation), and is NOT itself reported as HISTOGRAM-typed.
        let bucket_meta = engine
            .metadata("http_req_duration_seconds_bucket", 0, 100)
            .await
            .unwrap()
            .expect("bucket series has its own metadata");
        assert_eq!(bucket_meta.metric_type, metric_schema::metric_type::SUM);
    }

    #[tokio::test]
    async fn labels_without_key_returns_keys_including_service() {
        let (_dir, engine) = fixture().await;
        let LabelsResult::Keys(keys) = engine.labels("cpu.usage", None, 0, 100).await.unwrap()
        else {
            panic!("expected Keys variant");
        };
        assert!(keys.contains(&"service".to_string()));
        assert!(keys.contains(&"region".to_string()));
    }

    #[tokio::test]
    async fn labels_with_service_key_returns_distinct_service_values() {
        let (_dir, engine) = fixture().await;
        let LabelsResult::Values { mut values, capped } = engine
            .labels("cpu.usage", Some("service.name"), 0, 100)
            .await
            .unwrap()
        else {
            panic!("expected Values variant");
        };
        values.sort();
        assert_eq!(values, vec!["cart".to_string(), "checkout".to_string()]);
        assert!(!capped);
    }

    #[test]
    fn fold_replaces_bucket_family_with_one_histogram_entry() {
        fn e(name: &str, mt: i32) -> MetricCatalogEntry {
            MetricCatalogEntry {
                name: name.into(),
                metric_type: mt,
                type_text: None,
                unit: Some("seconds".into()),
                temporality: Some(2),
                is_monotonic: Some(true),
                last_seen_nanos: 100,
                series_count: 3,
            }
        }
        use photon_core::metric_schema::metric_type;
        let input = vec![
            e("http_req_duration_seconds_bucket", metric_type::SUM),
            e("http_req_duration_seconds_sum", metric_type::SUM),
            e("http_req_duration_seconds_count", metric_type::SUM),
            e("some_gauge", metric_type::GAUGE),
        ];
        let out = fold_classic_histograms(input);
        // The three family series collapse into one HISTOGRAM entry named for the base; gauge untouched.
        assert_eq!(out.len(), 2);
        let h = out
            .iter()
            .find(|x| x.name == "http_req_duration_seconds")
            .expect("folded base present");
        assert_eq!(h.metric_type, metric_type::HISTOGRAM);
        assert_eq!(h.unit.as_deref(), Some("seconds"));
        assert!(
            out.iter().all(|x| !x.name.ends_with("_bucket")),
            "raw bucket entry hidden"
        );
        assert!(out.iter().any(|x| x.name == "some_gauge"));
    }

    #[test]
    fn fold_leaves_lone_counters_alone() {
        // A `_sum`/`_count` with no matching `_bucket` is NOT a histogram family — leave as-is.
        fn e(name: &str) -> MetricCatalogEntry {
            MetricCatalogEntry {
                name: name.into(),
                metric_type: 1,
                type_text: None,
                unit: None,
                temporality: Some(2),
                is_monotonic: Some(true),
                last_seen_nanos: 1,
                series_count: 1,
            }
        }
        let out = fold_classic_histograms(vec![e("bytes_total"), e("orders_sum")]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|x| x.metric_type == 1));
    }
}

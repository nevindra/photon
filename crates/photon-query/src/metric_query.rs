//! Time-series query pipeline (`query_series`) — the numbers half of the metrics engine.
//! Everything SQL can express (gauge avg/min/max/sum/count, delta-Sum sum/increase/rate) is a
//! bucketed grouped aggregate over the pruned candidate files, mirroring `histogram.rs`'s
//! integer-arithmetic bucketing. Cumulative reset-aware rate/increase and gauge `last` need
//! adjacency Rust cannot get from a bucket aggregate — they live in `metric_query`'s pointwise
//! path (added in Task 7 alongside this file).

use std::collections::BTreeMap;

use arrow::array::{Array, Float64Array, Int64Array, StringArray};
use arrow::datatypes::DataType;
use datafusion::functions_aggregate::expr_fn::{avg, count, max, min, sum};
use datafusion::prelude::{cast, col, Expr};

use photon_core::metric_agg::{default_agg, Agg};
use photon_core::metric_schema;
use photon_core::query::{MetricFieldResolver, MetricResolvedQuery};
use photon_core::PhotonError;

use crate::col_ref;
use crate::metric_engine::{metric_base_predicate, MetricRequest};
use crate::metric_predicate::{metric_field_col, metrics_host_literal};
use crate::MetricsQueryEngine;

/// OTLP aggregation temporality, as stored verbatim by Phase-1 `metrics_mapping.rs`
/// (`AggregationTemporality::Delta as i32 == 1`, `Cumulative as i32 == 2`).
#[allow(dead_code)] // used by the pointwise path (Task 7); kept here for a single source of truth.
pub(crate) const TEMPORALITY_DELTA: i32 = 1;
pub(crate) const TEMPORALITY_CUMULATIVE: i32 = 2;

pub(crate) const MAX_SERIES: usize = 1000;

pub struct MetricSeriesRequest {
    pub metric: String,
    pub agg: Option<Agg>,
    pub group_by: Vec<String>,
    pub filter: Option<MetricResolvedQuery>,
    pub start_ts_nanos: i64,
    pub end_ts_nanos: i64,
    pub buckets: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesPoint {
    pub t: i64,
    pub v: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesResult {
    pub labels: BTreeMap<String, String>,
    pub points: Vec<SeriesPoint>,
}

pub struct QuerySeriesResult {
    pub series: Vec<SeriesResult>,
    pub default_agg: Agg,
    pub chosen_agg: Agg,
    pub step_nanos: i64,
    pub capped: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProbeMeta {
    pub metric_type: i32,
    pub temporality: Option<i32>,
    pub is_monotonic: Option<bool>,
    #[allow(dead_code)]
    pub unit: Option<String>,
}

/// Bucket `i`'s start timestamp (epoch nanos). Identical math to `histogram.rs::bucket_start`.
pub(crate) fn bucket_start(start: i64, end: i64, buckets: usize, i: usize) -> i64 {
    let span = (end - start) as i128;
    start + (span * i as i128 / buckets as i128) as i64
}

/// The bucket-index Expr: `(ts - start) / step`, divide-first (see `crate::bucket_math`), clamped
/// so `ts == end` lands in the last bucket. Same as `histogram.rs`/`span_histogram.rs`.
pub(crate) fn bucket_index_expr(start: i64, end: i64, buckets: usize) -> Result<Expr, PhotonError> {
    let ts_nanos = cast(col_ref(metric_schema::TIMESTAMP), DataType::Int64);
    crate::bucket_math::bucket_index_expr(ts_nanos, start, end, buckets)
        .map_err(|e| PhotonError::Query(format!("metric bucket case: {e}")))
}

impl MetricsQueryEngine {
    /// Discover one metric's type/temporality/is_monotonic/unit from the pruned data (LIMIT 1).
    /// `None` ⇒ the metric has no rows in the window (unknown/empty). This is the FULL
    /// prune+`read_parquet`+scan `query_series` used to run unconditionally on every call — callers
    /// should prefer `metric_meta_probe_cached`, which skips this when the metadata is already
    /// known for the current manifest generation.
    pub(crate) async fn metric_meta_probe(
        &self,
        req: &MetricRequest,
    ) -> Result<Option<ProbeMeta>, PhotonError> {
        // Test-only instrument: proves a cache hit in `metric_meta_probe_cached` actually skips
        // this method, without reaching into cache internals from a test.
        #[cfg(test)]
        self.probe_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let Some(df) = self.survivors_df(req).await? else {
            return Ok(None);
        };
        let batches = df
            .filter(metric_base_predicate(req))
            .map_err(|e| PhotonError::Query(format!("probe filter: {e}")))?
            .select(vec![
                col_ref(metric_schema::METRIC_TYPE),
                col_ref(metric_schema::TEMPORALITY),
                col_ref(metric_schema::IS_MONOTONIC),
                col_ref(metric_schema::UNIT),
            ])
            .map_err(|e| PhotonError::Query(format!("probe select: {e}")))?
            .limit(0, Some(1))
            .map_err(|e| PhotonError::Query(format!("probe limit: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("probe collect: {e}")))?;
        let Some(b) = batches.iter().find(|b| b.num_rows() > 0) else {
            return Ok(None);
        };
        // metric_type Int32 non-null; temporality Int32 nullable; is_monotonic Bool nullable;
        // unit Utf8 nullable. Downcast each and read row 0.
        Ok(Some(read_probe_row(b)))
    }

    /// Cache-then-probe wrapper around `metric_meta_probe`. Every metric chart query
    /// (`query_series`) needs this metadata at least once, sometimes twice (the classic-histogram
    /// `_bucket` companion probe) — dashboards multiply that by panel count. A metric's
    /// type/temporality/monotonicity/unit is stable, so once discovered for the CURRENT manifest
    /// generation it's safe to reuse without re-running the full prune+open+scan.
    ///
    /// A cache miss (`cached_metric_meta` returns `None`) always falls through to the real probe —
    /// there is no cached "confirmed absent" state, so a brand-new metric, or one with no rows in
    /// this particular window, is probed exactly as before. Invalidation is manifest `Arc`
    /// pointer-equality, mirroring `crate::ServicesCache` / `distinct_services`: see
    /// `MetricsQueryEngine::cached_metric_meta` / `cache_metric_meta` in `metric_engine.rs`.
    pub(crate) async fn metric_meta_probe_cached(
        &self,
        req: &MetricRequest,
    ) -> Result<Option<ProbeMeta>, PhotonError> {
        let manifest = self.load_metrics_manifest()?;
        if let Some(meta) = self.cached_metric_meta(&manifest, &req.metric) {
            return Ok(Some(meta));
        }
        let probed = self.metric_meta_probe(req).await?;
        if let Some(meta) = &probed {
            self.cache_metric_meta(&manifest, &req.metric, meta.clone());
        }
        Ok(probed)
    }

    pub async fn query_series(
        &self,
        req: MetricSeriesRequest,
    ) -> Result<QuerySeriesResult, PhotonError> {
        // Defense-in-depth: this is the central engine entry point every metrics aggregation path
        // (metric_dist.rs, metric_classic_hist.rs) routes through, so it must not trust a caller's
        // `buckets` even though `photon-api`'s handlers already clamp it — a huge value would
        // otherwise scale multiple `(0..buckets)` allocations downstream. Mirrors `photon-api`'s
        // `MAX_BUCKETS` (`crates/photon-api/src/query_params.rs`); `photon-query` can't depend on
        // `photon-api`, so the value is restated here as a literal.
        let buckets = req.buckets.clamp(1, 3000);
        let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);
        let step_nanos = ((end - start).max(1) / buckets as i64).max(1);

        let base = MetricRequest {
            metric: req.metric.clone(),
            start_ts_nanos: start,
            end_ts_nanos: end,
            filter: req.filter.clone(),
            // A `host.name:<v>` equality in the filter also prunes files by the skip-index host
            // range (Task 1.3) — not just the row predicate. Anything else leaves pruning off.
            host: req.filter.as_ref().and_then(metrics_host_literal),
        };

        // Discover type → default agg. No rows for the exact name ⇒ it may still be a classic
        // Prometheus histogram base (`<base>_bucket` stored as flat cumulative SUM series); route
        // to the query-time reassembly path. Otherwise empty (200). Checked only in the `None` arm
        // so a genuinely-stored metric that happens to have a `_bucket` sibling is never shadowed.
        let Some(meta) = self.metric_meta_probe_cached(&base).await? else {
            use crate::metric_classic_hist::bucket_name;
            let bucket_probe = MetricRequest {
                metric: bucket_name(&req.metric),
                start_ts_nanos: start,
                end_ts_nanos: end,
                filter: req.filter.clone(),
                host: None,
            };
            if self
                .metric_meta_probe_cached(&bucket_probe)
                .await?
                .is_some()
            {
                let default = default_agg(metric_schema::metric_type::HISTOGRAM, Some(true)); // P99
                let chosen = req.agg.unwrap_or(default);
                return self
                    .query_series_classic_histogram(&req, chosen, default, step_nanos, buckets)
                    .await;
            }
            let default = Agg::Avg;
            return Ok(QuerySeriesResult {
                series: Vec::new(),
                default_agg: default,
                chosen_agg: req.agg.unwrap_or(default),
                step_nanos,
                capped: false,
            });
        };
        let default = default_agg(meta.metric_type, meta.is_monotonic);
        let chosen = req.agg.unwrap_or(default);

        // Distribution types get their own Rust roll-up paths (metric_dist.rs). Stays a `match`
        // (not collapsed to `if`) because HISTOGRAM/EXP_HISTOGRAM/SUMMARY are all routed here,
        // each to its own `query_series_*` method in metric_dist.rs.
        match meta.metric_type {
            metric_schema::metric_type::HISTOGRAM => {
                return self
                    .query_series_histogram(&base, &req, meta, default, chosen, step_nanos, buckets)
                    .await
            }
            metric_schema::metric_type::EXP_HISTOGRAM => {
                return self
                    .query_series_exp_histogram(
                        &base, &req, meta, default, chosen, step_nanos, buckets,
                    )
                    .await
            }
            metric_schema::metric_type::SUMMARY => {
                return self
                    .query_series_summary(&base, &req, meta, default, chosen, step_nanos, buckets)
                    .await
            }
            _ => {}
        }

        // Route: this task handles the SQL-expressible aggregations; the pointwise path (Task 7)
        // handles cumulative rate/increase and gauge last.
        let is_cumulative = meta.temporality == Some(TEMPORALITY_CUMULATIVE);
        let sql_series = match chosen {
            Agg::Avg | Agg::Min | Agg::Max | Agg::Sum | Agg::Count => {
                self.series_sql_agg(&base, &req, chosen, /*scale_by_step=*/ false)
                    .await?
            }
            Agg::Increase if !is_cumulative => {
                // delta-Sum increase = Σ values per bucket.
                self.series_sql_agg(&base, &req, Agg::Sum, false).await?
            }
            Agg::Rate if !is_cumulative => {
                // delta-Sum rate = (Σ values per bucket) / step_seconds.
                self.series_sql_agg(&base, &req, Agg::Sum, true).await?
            }
            Agg::Rate | Agg::Increase | Agg::Last => {
                // cumulative rate/increase + gauge last — pointwise path (Task 7).
                return self
                    .query_series_pointwise(req, meta, default, chosen, step_nanos, buckets)
                    .await;
            }
            Agg::P50 | Agg::P90 | Agg::P99 | Agg::Median => {
                return Err(PhotonError::Query(format!(
                    "aggregation `{}` requires a histogram, exponential-histogram, or summary metric",
                    chosen.as_str()
                )));
            }
        };

        let (series, capped) = sql_series;
        Ok(QuerySeriesResult {
            series,
            default_agg: default,
            chosen_agg: chosen,
            step_nanos,
            capped,
        })
    }

    /// SQL bucketed grouped aggregate. `scale_by_step` divides each value by the step in seconds
    /// (for delta rate). Returns (series, capped).
    async fn series_sql_agg(
        &self,
        base: &MetricRequest,
        req: &MetricSeriesRequest,
        agg: Agg,
        scale_by_step: bool,
    ) -> Result<(Vec<SeriesResult>, bool), PhotonError> {
        let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);
        // Second, independent binding of `req.buckets` (this helper is reached from
        // `query_series`'s SQL aggregation path via `&req`, not the already-clamped local) — clamp
        // again here rather than trust that every caller re-derives from the clamped value.
        let buckets = req.buckets.clamp(1, 3000);
        let Some(df) = self.survivors_df(base).await? else {
            return Ok((Vec::new(), false));
        };
        let group_cols = self.resolve_group_cols(&req.group_by)?;
        let bucket = bucket_index_expr(start, end, buckets)?;

        let value = col_ref(metric_schema::VALUE);
        let agg_expr = match agg {
            Agg::Avg => avg(value),
            Agg::Min => min(value),
            Agg::Max => max(value),
            Agg::Sum => sum(value),
            // `count` yields Int64; cast so the value column downcast stays Float64 like the rest.
            Agg::Count => cast(count(value), DataType::Float64),
            _ => {
                return Err(PhotonError::Query(
                    "series_sql_agg: non-SQL aggregation".to_string(),
                ))
            }
        }
        .alias("v");

        let mut group_exprs = vec![bucket.alias("__bucket")];
        for (i, ge) in group_cols.iter().enumerate() {
            group_exprs.push(ge.clone().alias(format!("__g{i}")));
        }

        let batches = df
            .filter(metric_base_predicate(base))
            .map_err(|e| PhotonError::Query(format!("series filter: {e}")))?
            .aggregate(group_exprs, vec![agg_expr])
            .map_err(|e| PhotonError::Query(format!("series aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("series collect: {e}")))?;

        let step_secs = (((end - start).max(1)) as f64 / buckets as f64) / 1e9;
        let mut assembler = SeriesAssembler::new(&req.group_by, start, end, buckets);
        for b in &batches {
            let bucket_col = b
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("series: bucket not Int64".into()))?;
            let n_group = req.group_by.len();
            let v_col = b
                .column(1 + n_group)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| PhotonError::Query("series: value not Float64".into()))?;
            let group_arrays: Vec<&StringArray> = (0..n_group)
                .map(|g| b.column(1 + g).as_any().downcast_ref::<StringArray>())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| PhotonError::Query("series: group col not Utf8".into()))?;
            for i in 0..b.num_rows() {
                let bidx = (bucket_col.value(i).clamp(0, buckets as i64 - 1)) as usize;
                let key: Vec<Option<String>> = group_arrays
                    .iter()
                    .map(|a| {
                        if a.is_null(i) {
                            None
                        } else {
                            Some(a.value(i).to_string())
                        }
                    })
                    .collect();
                let mut v = if v_col.is_null(i) {
                    None
                } else {
                    Some(v_col.value(i))
                };
                if scale_by_step {
                    v = v.map(|x| x / step_secs);
                }
                assembler.put(key, bidx, v);
            }
        }
        Ok(assembler.finish())
    }

    /// Resolve each group-by label name to a column Expr (promoted col or `attributes[key]`).
    pub(crate) fn resolve_group_cols(&self, group_by: &[String]) -> Result<Vec<Expr>, PhotonError> {
        let resolver = MetricFieldResolver::new(self.promoted_attributes());
        group_by
            .iter()
            .map(|name| {
                let fr = resolver.resolve_field_name(name).map_err(|e| {
                    PhotonError::Query(format!("cannot group by `{name}`: {}", e.message))
                })?;
                Ok(metric_field_col(&fr))
            })
            .collect()
    }

    /// Cumulative reset-aware rate/increase and gauge `last`. Fetches per-series time-ordered raw
    /// points (sorted by group key then timestamp), then walks each series in Rust applying the
    /// counter-reset rule and rolls contributions into buckets.
    async fn query_series_pointwise(
        &self,
        req: MetricSeriesRequest,
        meta: ProbeMeta,
        default: Agg,
        chosen: Agg,
        step_nanos: i64,
        buckets: usize,
    ) -> Result<QuerySeriesResult, PhotonError> {
        let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);

        // rate/increase are counter operations — only meaningful on a Sum. `last` works on any
        // numeric series (gauge).
        if matches!(chosen, Agg::Rate | Agg::Increase)
            && meta.metric_type != metric_schema::metric_type::SUM
        {
            return Err(PhotonError::Query(format!(
                "`{}` requires a Sum metric",
                chosen.as_str()
            )));
        }

        let base = MetricRequest {
            metric: req.metric.clone(),
            start_ts_nanos: start,
            end_ts_nanos: end,
            filter: req.filter.clone(),
            host: None,
        };
        let Some(df) = self.survivors_df(&base).await? else {
            return Ok(QuerySeriesResult {
                series: Vec::new(),
                default_agg: default,
                chosen_agg: chosen,
                step_nanos,
                capped: false,
            });
        };

        // Cloned up front so the per-series `build` closure can capture the label names without
        // conflicting with the immutable borrows of `req` above.
        let group_by = req.group_by.clone();
        let n_group = group_by.len();
        let group_cols = self.resolve_group_cols(&group_by)?;
        let mut selects: Vec<Expr> = group_cols
            .iter()
            .enumerate()
            .map(|(i, e)| e.clone().alias(format!("__g{i}")))
            .collect();
        selects.push(cast(col_ref(metric_schema::TIMESTAMP), DataType::Int64).alias("__ts"));
        selects.push(col_ref(metric_schema::VALUE).alias("__v"));
        selects.push(cast(col_ref(metric_schema::START_TIMESTAMP), DataType::Int64).alias("__st"));

        let mut sorts: Vec<_> = (0..n_group)
            .map(|i| col(format!("__g{i}")).sort(true, true))
            .collect();
        sorts.push(col("__ts").sort(true, false));

        let batches = df
            .filter(metric_base_predicate(&base))
            .map_err(|e| PhotonError::Query(format!("pointwise filter: {e}")))?
            .select(selects)
            .map_err(|e| PhotonError::Query(format!("pointwise select: {e}")))?
            .sort(sorts)
            .map_err(|e| PhotonError::Query(format!("pointwise sort: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("pointwise collect: {e}")))?;

        let step_secs = (((end - start).max(1)) as f64 / buckets as f64) / 1e9;

        // Walk globally-sorted rows, flushing a SeriesResult each time the group key changes.
        let mut series: Vec<SeriesResult> = Vec::new();
        let mut cur_key: Option<Vec<Option<String>>> = None;
        let mut cur_rows: Vec<PointRow> = Vec::new();
        let mut capped = false;

        let build = |key: &[Option<String>], rows: &[PointRow]| -> SeriesResult {
            let points = match chosen {
                Agg::Rate => reset_aware_series(rows, start, end, buckets, true, step_secs),
                Agg::Increase => reset_aware_series(rows, start, end, buckets, false, step_secs),
                Agg::Last => last_series(rows, start, end, buckets),
                _ => unreachable!("pointwise path only handles rate/increase/last"),
            };
            let mut labels = BTreeMap::new();
            for (name, val) in group_by.iter().zip(key.iter()) {
                if let Some(v) = val {
                    labels.insert(name.clone(), v.clone());
                }
            }
            SeriesResult { labels, points }
        };

        for b in &batches {
            let ts = b
                .column(n_group)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("pointwise: __ts not Int64".into()))?;
            let v = b
                .column(n_group + 1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| PhotonError::Query("pointwise: __v not Float64".into()))?;
            let st = b
                .column(n_group + 2)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("pointwise: __st not Int64".into()))?;
            let gcols: Vec<&StringArray> = (0..n_group)
                .map(|g| b.column(g).as_any().downcast_ref::<StringArray>())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| PhotonError::Query("pointwise: group col not Utf8".into()))?;

            for i in 0..b.num_rows() {
                let key: Vec<Option<String>> = gcols
                    .iter()
                    .map(|a| {
                        if a.is_null(i) {
                            None
                        } else {
                            Some(a.value(i).to_string())
                        }
                    })
                    .collect();
                if cur_key.as_ref() != Some(&key) {
                    if let Some(k) = cur_key.take() {
                        if series.len() < MAX_SERIES {
                            series.push(build(&k, &cur_rows));
                        } else {
                            capped = true;
                        }
                    }
                    cur_key = Some(key);
                    cur_rows = Vec::new();
                }
                cur_rows.push(PointRow {
                    ts: ts.value(i),
                    v: if v.is_null(i) { None } else { Some(v.value(i)) },
                    st: if st.is_null(i) {
                        None
                    } else {
                        Some(st.value(i))
                    },
                });
            }
        }
        if let Some(k) = cur_key.take() {
            if series.len() < MAX_SERIES {
                series.push(build(&k, &cur_rows));
            } else {
                capped = true;
            }
        }
        if capped {
            eprintln!(
                "photon-query: warning: metric series truncated to {MAX_SERIES}; refine group-by/filter"
            );
        }

        Ok(QuerySeriesResult {
            series,
            default_agg: default,
            chosen_agg: chosen,
            step_nanos,
            capped,
        })
    }
}

/// One raw sample of a single series (already time-ordered). `v`/`st` are nullable in Parquet.
pub(crate) struct PointRow {
    pub ts: i64,
    pub v: Option<f64>,
    pub st: Option<i64>,
}

/// Bucket index for a timestamp: `(ts - start) / step`, divide-first (see `crate::bucket_math`),
/// clamped to the last bucket.
pub(crate) fn bucket_of(ts: i64, start: i64, end: i64, buckets: usize) -> usize {
    crate::bucket_math::bucket_index(ts, start, end, buckets)
}

/// Reset-aware cumulative rollup for one series. Walks consecutive samples: a counter reset —
/// value decreases OR `start_timestamp` advances — contributes the new (post-reset) value; else
/// the positive delta. The first sample contributes 0 (no predecessor). Each contribution is
/// attributed to the bucket of the later sample. `rate` divides each bucket by `step_secs`.
pub(crate) fn reset_aware_series(
    rows: &[PointRow],
    start: i64,
    end: i64,
    buckets: usize,
    rate: bool,
    step_secs: f64,
) -> Vec<SeriesPoint> {
    let mut inc = vec![0f64; buckets];
    let mut has = vec![false; buckets];
    let mut prev: Option<(f64, Option<i64>)> = None;
    for r in rows {
        let Some(vi) = r.v else { continue };
        let b = bucket_of(r.ts, start, end, buckets);
        let contribution = match prev {
            None => 0.0,
            Some((pv, pst)) => {
                let reset = vi < pv || (r.st.is_some() && pst.is_some() && r.st > pst);
                if reset {
                    vi
                } else {
                    vi - pv
                }
            }
        };
        inc[b] += contribution;
        has[b] = true;
        prev = Some((vi, r.st));
    }
    (0..buckets)
        .map(|i| SeriesPoint {
            t: bucket_start(start, end, buckets, i),
            v: has[i].then(|| if rate { inc[i] / step_secs } else { inc[i] }),
        })
        .collect()
}

/// Gauge `last`: the value at the maximum timestamp within each bucket. `rows` are time-sorted, so
/// the last write per bucket wins.
pub(crate) fn last_series(
    rows: &[PointRow],
    start: i64,
    end: i64,
    buckets: usize,
) -> Vec<SeriesPoint> {
    let mut last: Vec<Option<f64>> = vec![None; buckets];
    for r in rows {
        if let Some(x) = r.v {
            last[bucket_of(r.ts, start, end, buckets)] = Some(x);
        }
    }
    (0..buckets)
        .map(|i| SeriesPoint {
            t: bucket_start(start, end, buckets, i),
            v: last[i],
        })
        .collect()
}

/// Groups rows into per-series bucket vectors, capping distinct series at `MAX_SERIES`.
pub(crate) struct SeriesAssembler {
    group_by: Vec<String>,
    start: i64,
    end: i64,
    buckets: usize,
    series: BTreeMap<Vec<Option<String>>, Vec<SeriesPoint>>,
    capped: bool,
}

/// The all-`None` bucket skeleton every series starts from: one point per bucket carrying its
/// start timestamp. The single source shared by `SeriesAssembler::empty_points` and `put`.
fn empty_points(start: i64, end: i64, buckets: usize) -> Vec<SeriesPoint> {
    (0..buckets)
        .map(|i| SeriesPoint {
            t: bucket_start(start, end, buckets, i),
            v: None,
        })
        .collect()
}

impl SeriesAssembler {
    pub(crate) fn new(
        group_by: &[String],
        start: i64,
        end: i64,
        buckets: usize,
    ) -> SeriesAssembler {
        SeriesAssembler {
            group_by: group_by.to_vec(),
            start,
            end,
            buckets,
            series: BTreeMap::new(),
            capped: false,
        }
    }

    #[allow(dead_code)] // shared skeleton helper; also used by the Task-7 pointwise path.
    fn empty_points(&self) -> Vec<SeriesPoint> {
        empty_points(self.start, self.end, self.buckets)
    }

    /// Set bucket `bidx` of the series identified by `key` to `v`. Late series past the cap are
    /// dropped (flagging `capped`) rather than silently ignored.
    pub(crate) fn put(&mut self, key: Vec<Option<String>>, bidx: usize, v: Option<f64>) {
        if !self.series.contains_key(&key) && self.series.len() >= MAX_SERIES {
            self.capped = true;
            return;
        }
        let (start, end, buckets) = (self.start, self.end, self.buckets);
        let pts = self
            .series
            .entry(key)
            .or_insert_with(|| empty_points(start, end, buckets));
        if bidx < pts.len() {
            pts[bidx].v = v;
        }
    }

    pub(crate) fn finish(self) -> (Vec<SeriesResult>, bool) {
        if self.capped {
            eprintln!(
                "photon-query: warning: metric series truncated to {MAX_SERIES}; refine group-by/filter"
            );
        }
        let group_by = self.group_by;
        let out = self
            .series
            .into_iter()
            .map(|(key, points)| {
                let mut labels = BTreeMap::new();
                for (name, val) in group_by.iter().zip(key) {
                    if let Some(v) = val {
                        labels.insert(name.clone(), v);
                    }
                }
                SeriesResult { labels, points }
            })
            .collect();
        (out, self.capped)
    }
}

fn read_probe_row(b: &arrow::array::RecordBatch) -> ProbeMeta {
    use arrow::array::{BooleanArray, Int32Array, StringArray};
    let mtype = b.column(0).as_any().downcast_ref::<Int32Array>().unwrap();
    let temp = b.column(1).as_any().downcast_ref::<Int32Array>();
    let mono = b.column(2).as_any().downcast_ref::<BooleanArray>();
    let unit = b.column(3).as_any().downcast_ref::<StringArray>();
    ProbeMeta {
        metric_type: mtype.value(0),
        temporality: temp.and_then(|c| (!c.is_null(0)).then(|| c.value(0))),
        is_monotonic: mono.and_then(|c| (!c.is_null(0)).then(|| c.value(0))),
        unit: unit.and_then(|c| (!c.is_null(0)).then(|| c.value(0).to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::metric_schema::{metric_type, MetricSchema};

    /// One gauge point for `svc` at `ts` with value `v` (service.name promoted).
    fn gpoint(name: &str, svc: &str, ts: i64, v: f64) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::GAUGE,
            timestamp_nanos: ts,
            value: Some(v),
            attributes,
            ..Default::default()
        }
    }

    /// An engine over a hand-built in-memory gauge batch (no compaction). Window [0, 200], 2
    /// buckets: service=a → (0→10, 100→30), service=b → (0→5, 100→7).
    fn gauge_engine() -> MetricsQueryEngine {
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        for p in [
            gpoint("g", "a", 0, 10.0),
            gpoint("g", "a", 100, 30.0),
            gpoint("g", "b", 0, 5.0),
            gpoint("g", "b", 100, 7.0),
        ] {
            b.append(&p);
        }
        let batch = b.finish().unwrap();
        MetricsQueryEngine::from_batch(schema, batch)
    }

    #[tokio::test]
    async fn gauge_avg_bucketed_and_grouped() {
        let engine = gauge_engine();
        let res = engine
            .query_series(MetricSeriesRequest {
                metric: "g".into(),
                agg: Some(Agg::Avg),
                group_by: vec!["service".into()],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 200,
                buckets: 2,
            })
            .await
            .unwrap();

        assert_eq!(res.default_agg, Agg::Avg);
        assert_eq!(res.chosen_agg, Agg::Avg);
        assert_eq!(res.step_nanos, 100);
        assert!(!res.capped);
        assert_eq!(res.series.len(), 2, "one series per service");

        // BTreeMap iteration is key-sorted: "a" before "b".
        let a = &res.series[0];
        assert_eq!(a.labels.get("service").map(String::as_str), Some("a"));
        assert_eq!(
            a.points.iter().map(|p| p.t).collect::<Vec<_>>(),
            vec![0, 100]
        );
        assert_eq!(
            a.points.iter().map(|p| p.v).collect::<Vec<_>>(),
            vec![Some(10.0), Some(30.0)]
        );

        let b = &res.series[1];
        assert_eq!(b.labels.get("service").map(String::as_str), Some("b"));
        assert_eq!(
            b.points.iter().map(|p| p.v).collect::<Vec<_>>(),
            vec![Some(5.0), Some(7.0)]
        );
    }

    /// DoS defense-in-depth: an oversized `buckets` must be clamped to `MAX_BUCKETS` (3000) at
    /// `query_series`, not allocate a 10-million-element `Vec` per series. Also exercises the SQL
    /// aggregation path (`Agg::Avg` → `series_sql_agg`), which re-derives `buckets` from `req`
    /// independently of `query_series`'s own local — both bindings must clamp.
    #[tokio::test]
    async fn buckets_request_is_clamped_to_max() {
        let engine = gauge_engine();
        let res = engine
            .query_series(MetricSeriesRequest {
                metric: "g".into(),
                agg: Some(Agg::Avg),
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 200,
                buckets: 10_000_000,
            })
            .await
            .unwrap();

        assert_eq!(res.series.len(), 1);
        assert_eq!(
            res.series[0].points.len(),
            3000,
            "buckets must clamp to MAX_BUCKETS (3000), not the caller-supplied 10_000_000"
        );
    }

    #[tokio::test]
    async fn gauge_avg_ungrouped_is_one_series() {
        let engine = gauge_engine();
        let res = engine
            .query_series(MetricSeriesRequest {
                metric: "g".into(),
                agg: Some(Agg::Avg),
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 200,
                buckets: 2,
            })
            .await
            .unwrap();

        assert_eq!(res.series.len(), 1, "no group-by → one merged series");
        let s = &res.series[0];
        assert!(s.labels.is_empty());
        assert_eq!(s.points[0].t, 0);
        assert_eq!(s.points[1].t, 100);
        // bucket 0 = avg(10, 5) = 7.5; bucket 1 = avg(30, 7) = 18.5.
        assert_eq!(s.points[0].v, Some(7.5));
        assert_eq!(s.points[1].v, Some(18.5));
    }

    #[tokio::test]
    async fn wide_window_series_bucket_index_does_not_overflow() {
        // Regression for the i64-overflow bug: the old multiply-first `bucket_index_expr`
        // (`(ts - start) * buckets / span`) overflowed `i64` once the window exceeded ~35 days at
        // `buckets = 3000` (`span * buckets > i64::MAX`). A 90-day window at the max bucket count
        // exercises exactly that overflow through the real `query_series` → `series_sql_agg` →
        // `bucket_index_expr` DataFusion path (not just the pure-Rust `bucket_math` unit tests).
        const NS_PER_DAY: i64 = 24 * 3600 * 1_000_000_000;
        let end = 90 * NS_PER_DAY;
        let buckets = 3000usize;
        assert!(
            end.checked_mul(buckets as i64).is_none(),
            "window must be wide enough to have overflowed the old multiply-first formula"
        );

        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        for p in [
            gpoint("g", "a", 0, 10.0),
            gpoint("g", "a", end / 2, 20.0),
            gpoint("g", "a", end, 30.0),
        ] {
            b.append(&p);
        }
        let batch = b.finish().unwrap();
        let engine = MetricsQueryEngine::from_batch(schema, batch);

        let res = engine
            .query_series(MetricSeriesRequest {
                metric: "g".into(),
                agg: Some(Agg::Avg),
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: end,
                buckets,
            })
            .await
            .unwrap();

        assert_eq!(res.series.len(), 1);
        let pts = &res.series[0].points;
        assert_eq!(pts.len(), buckets);
        assert_eq!(
            pts[0].v,
            Some(10.0),
            "row at window start lands in bucket 0"
        );
        assert_eq!(
            pts[buckets - 1].v,
            Some(30.0),
            "row at window end lands in the last bucket"
        );
    }
}

#[cfg(test)]
mod pointwise_tests {
    use super::*;

    // (ts, value, start_ts) rows for one series, already time-sorted.
    fn row(ts: i64, v: f64, st: Option<i64>) -> PointRow {
        PointRow { ts, v: Some(v), st }
    }

    #[test]
    fn cumulative_increase_no_reset() {
        // window [0,100], 2 buckets (width 50). Cumulative counter 0→10→25→40.
        // deltas: (10),(15),(15). ts placed so 10 lands bucket0, 25 & 40 land bucket1.
        let rows = vec![
            row(0, 0.0, Some(0)),
            row(10, 10.0, Some(0)),
            row(60, 25.0, Some(0)),
            row(90, 40.0, Some(0)),
        ];
        let pts = reset_aware_series(&rows, 0, 100, 2, /*rate=*/ false, 50.0);
        // bucket0 increase: contributions of ts=0 (0, first) + ts=10 (10-0=10) = 10.
        // bucket1 increase: ts=60 (25-10=15) + ts=90 (40-25=15) = 30.
        assert_eq!(pts[0].v, Some(10.0));
        assert_eq!(pts[1].v, Some(30.0));
    }

    #[test]
    fn cumulative_increase_with_reset_by_decrease() {
        // Counter resets: 0→100 then drops to 5 (restart) →20. The reset contributes the NEW value
        // (5), never a negative delta.
        let rows = vec![
            row(0, 0.0, Some(0)),
            row(40, 100.0, Some(0)),
            row(60, 5.0, Some(0)),
            row(90, 20.0, Some(0)),
        ];
        let pts = reset_aware_series(&rows, 0, 100, 2, false, 50.0);
        // bucket0: ts0 (0) + ts40 (100-0=100) = 100.
        // bucket1: ts60 (reset → +5) + ts90 (20-5=15) = 20.
        assert_eq!(pts[0].v, Some(100.0));
        assert_eq!(pts[1].v, Some(20.0));
    }

    #[test]
    fn cumulative_reset_by_start_timestamp_advance() {
        // Value does not decrease (10 → 12) but start_timestamp advances → treat as reset, so the
        // point contributes its full value (12), not 12-10=2.
        let rows = vec![row(10, 10.0, Some(0)), row(60, 12.0, Some(50))];
        let pts = reset_aware_series(&rows, 0, 100, 2, false, 50.0);
        // ts10 is the FIRST sample → contributes 0 (no predecessor); only ts10 in bucket0 → 0.
        assert_eq!(pts[0].v, Some(0.0));
        assert_eq!(pts[1].v, Some(12.0)); // start advanced → full value
    }

    #[test]
    fn rate_divides_by_step_seconds() {
        let rows = vec![row(0, 0.0, Some(0)), row(90, 60.0, Some(0))];
        // increase in bucket1 = 60; step_secs passed explicitly.
        let pts = reset_aware_series(&rows, 0, 100, 2, /*rate=*/ true, 30.0);
        assert_eq!(pts[1].v, Some(2.0)); // 60 / 30
    }

    #[test]
    fn last_takes_max_ts_value_per_bucket() {
        let rows = vec![
            row(0, 1.0, None),
            row(10, 2.0, None),
            row(60, 9.0, None),
            row(90, 3.0, None),
        ];
        let pts = last_series(&rows, 0, 100, 2);
        assert_eq!(pts[0].v, Some(2.0)); // last in bucket0 by ts
        assert_eq!(pts[1].v, Some(3.0)); // last in bucket1 by ts
    }

    #[test]
    fn empty_buckets_are_gaps() {
        let rows = vec![row(5, 1.0, None)];
        let pts = last_series(&rows, 0, 100, 2);
        assert_eq!(pts[0].v, Some(1.0));
        assert_eq!(pts[1].v, None); // no point → gap
    }
}

//! Prometheus classic-histogram reassembly (query-time, Approach A). A classic histogram arrives
//! (via Plan-1 remote-write) as flat cumulative `SUM` series: `<base>_bucket{le=..}` (count of
//! observations ≤ le), `<base>_sum`, `<base>_count`. This module turns the per-`le` bucket counts
//! back into a distribution and computes quantiles, reusing `metric_dist`'s interpolation and
//! `metric_query`'s reset-aware rollup. Storage is unchanged; nothing here is stateful.

use std::collections::BTreeMap;

use arrow::array::{Array, Float64Array, Int64Array, StringArray};
use arrow::datatypes::DataType;
use datafusion::prelude::{cast, col, Expr};

use photon_core::metric_agg::Agg;
use photon_core::metric_schema;
use photon_core::query::MetricFieldResolver;
use photon_core::PhotonError;

use crate::metric_dist::{hist_ranges, interpolate_quantile, quantile_of};
use crate::metric_engine::{metric_base_predicate, MetricRequest};
use crate::metric_predicate::metric_field_col;
use crate::metric_query::{
    bucket_start, reset_aware_series, MetricSeriesRequest, PointRow, QuerySeriesResult,
    SeriesPoint, SeriesResult, MAX_SERIES,
};
use crate::{col_ref, MetricsQueryEngine};

/// Parse a Prometheus `le` label value: `"+Inf"` (any case) → +∞, else an `f64`. `None` if garbage
/// or NaN (a NaN bound would poison the reassembled distribution, so it's rejected here rather
/// than flowing through as a bound).
pub(crate) fn parse_le(s: &str) -> Option<f64> {
    match s {
        "+Inf" | "+inf" | "Inf" | "inf" => Some(f64::INFINITY),
        _ => s.parse::<f64>().ok().filter(|v| !v.is_nan()),
    }
}

/// The base name of a classic-histogram bucket series: `foo_bucket` → `Some("foo")`, else `None`.
pub(crate) fn classic_base(name: &str) -> Option<&str> {
    name.strip_suffix("_bucket")
}

pub(crate) fn bucket_name(base: &str) -> String {
    format!("{base}_bucket")
}
pub(crate) fn sum_name(base: &str) -> String {
    format!("{base}_sum")
}
pub(crate) fn count_name(base: &str) -> String {
    format!("{base}_count")
}

/// Quantile `q` from cumulative-in-`le` counts `(le, cumulative_count)` (need not be sorted).
/// Differences adjacent cumulative counts into per-bucket counts, builds ascending
/// `(lo,hi,count)` ranges via `hist_ranges`, and interpolates via `interpolate_quantile`.
/// All infinite `le`s (real remote-write input should have at most one `+Inf`, but a
/// misbehaving/adversarial exporter could send several — or spellings that overflow to
/// infinity) collapse into a single trailing overflow bucket, so `counts.len()` is always
/// `bounds.len() + 1` (or `bounds.len()` if there's no infinite `le` at all) — `hist_ranges`
/// can never be asked to index past `bounds`. `None` if empty or total count ≤ 0.
pub(crate) fn classic_quantile(cum_le: &[(f64, f64)], q: f64) -> Option<f64> {
    if cum_le.is_empty() {
        return None;
    }
    let mut v: Vec<(f64, f64)> = cum_le.to_vec();
    // Ascending by le; +Inf (INFINITY) sorts last.
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut bounds: Vec<f64> = Vec::with_capacity(v.len());
    let mut counts: Vec<f64> = Vec::with_capacity(v.len());
    let mut overflow = 0.0_f64;
    let mut has_overflow = false;
    let mut prev_cum = 0.0_f64;
    for &(le, c) in &v {
        // Guard tiny negative deltas (float noise / partial resets) — buckets are non-negative.
        let bucket = (c - prev_cum).max(0.0);
        if le.is_infinite() {
            // Fold every infinite-`le` bucket into one overflow total instead of pushing a
            // `counts` entry per occurrence — a `counts` entry that outnumbers `bounds` by more
            // than one is exactly what makes `hist_ranges` index out of bounds.
            overflow += bucket;
            has_overflow = true;
        } else {
            bounds.push(le);
            counts.push(bucket);
        }
        prev_cum = c;
    }
    if has_overflow {
        counts.push(overflow); // single overflow bucket: no finite upper bound
    }
    interpolate_quantile(&hist_ranges(&bounds, &counts), q)
}

/// A `<base>_x` (`_bucket`/`_sum`/`_count`) read over `req`'s window + label filter.
fn companion_request(req: &MetricSeriesRequest, metric: String) -> MetricRequest {
    MetricRequest {
        metric,
        start_ts_nanos: req.start_ts_nanos,
        end_ts_nanos: req.end_ts_nanos,
        filter: req.filter.clone(),
        host: None,
    }
}

/// The per-bucket step in seconds — identical math to the pointwise/SQL paths.
fn step_secs(start: i64, end: i64, buckets: usize) -> f64 {
    (((end - start).max(1)) as f64 / buckets as f64) / 1e9
}

/// Build a `labels` map from group-by names zipped with a resolved group key (dropping `None`s).
fn labels_from(group_by: &[String], key: &[Option<String>]) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    for (name, val) in group_by.iter().zip(key.iter()) {
        if let Some(v) = val {
            labels.insert(name.clone(), v.clone());
        }
    }
    labels
}

impl MetricsQueryEngine {
    /// Query-time reassembly of a Prometheus classic histogram stored (by Plan-1 remote-write) as
    /// flat cumulative `SUM` series: `<base>_bucket{le=..}` (count of observations ≤ le),
    /// `<base>_sum`, `<base>_count`. Percentiles diff the per-`le` cumulative counts back into a
    /// distribution and interpolate (`classic_quantile`); count/sum/avg roll up the companion
    /// counters with the reset-aware increase rule. The `le` label is always consumed by the
    /// reassembly — never a user group-by. Storage is untouched; nothing here is stateful.
    pub(crate) async fn query_series_classic_histogram(
        &self,
        req: &MetricSeriesRequest,
        chosen: Agg,
        default: Agg,
        step_nanos: i64,
        buckets: usize,
    ) -> Result<QuerySeriesResult, PhotonError> {
        // Same allow-list (and error string) as the OTLP histogram path (metric_dist.rs).
        if !matches!(
            chosen,
            Agg::P50 | Agg::P90 | Agg::P99 | Agg::Median | Agg::Count | Agg::Sum | Agg::Avg
        ) {
            return Err(PhotonError::Query(format!(
                "aggregation `{}` is not supported for histogram metrics (use p50/p90/p99/count/sum/avg)",
                chosen.as_str()
            )));
        }

        let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);
        // Defense-in-depth: mirrors `photon-api`'s `MAX_BUCKETS`
        // (`crates/photon-api/src/query_params.rs`); `photon-query` can't depend on `photon-api`,
        // so the value is restated here as a literal. Without this, the `(0..buckets)` builds
        // below (and `reset_aware_series`'s own per-series allocation) scale directly with a
        // caller-supplied `buckets`.
        let buckets = buckets.clamp(1, 3000);

        // `le` is consumed by the reassembly — never a user group-by (drop it if a caller passes it).
        let group_by: Vec<String> = req
            .group_by
            .iter()
            .filter(|g| g.as_str() != "le")
            .cloned()
            .collect();

        let (series, capped) = match chosen {
            Agg::Count => {
                let base = companion_request(req, count_name(&req.metric));
                self.classic_counter_rollup(&base, &group_by, start, end, buckets)
                    .await?
            }
            Agg::Sum => {
                let base = companion_request(req, sum_name(&req.metric));
                self.classic_counter_rollup(&base, &group_by, start, end, buckets)
                    .await?
            }
            Agg::Avg => {
                self.classic_avg(req, &group_by, start, end, buckets)
                    .await?
            }
            // P50 | P90 | P99 | Median — guaranteed by the allow-list above.
            _ => {
                let q = quantile_of(chosen).expect("allow-list guarantees a quantile agg");
                self.classic_percentiles(req, &group_by, q, start, end, buckets)
                    .await?
            }
        };

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

    /// Scan one metric's numeric `value` rows, grouped by the resolved `group_by` columns, sorted
    /// by `(group key, timestamp)`. Mirrors `collect_dist_series` / `query_series_pointwise`, but
    /// reads the `value` column (not a JSON payload) and returns the raw group key alongside the
    /// time-ordered `PointRow`s. Distinct series are capped at `MAX_SERIES`.
    async fn collect_value_series(
        &self,
        base: &MetricRequest,
        group_by: &[String],
    ) -> Result<(Vec<(Vec<Option<String>>, Vec<PointRow>)>, bool), PhotonError> {
        let Some(df) = self.survivors_df(base).await? else {
            return Ok((Vec::new(), false));
        };
        let n_group = group_by.len();
        let group_cols = self.resolve_group_cols(group_by)?;
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
            .filter(metric_base_predicate(base))
            .map_err(|e| PhotonError::Query(format!("classic-hist counter filter: {e}")))?
            .select(selects)
            .map_err(|e| PhotonError::Query(format!("classic-hist counter select: {e}")))?
            .sort(sorts)
            .map_err(|e| PhotonError::Query(format!("classic-hist counter sort: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("classic-hist counter collect: {e}")))?;

        let mut out: Vec<(Vec<Option<String>>, Vec<PointRow>)> = Vec::new();
        let mut cur_key: Option<Vec<Option<String>>> = None;
        let mut cur_rows: Vec<PointRow> = Vec::new();
        let mut capped = false;

        for b in &batches {
            let ts = b
                .column(n_group)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("classic-hist counter: __ts not Int64".into()))?;
            let v = b
                .column(n_group + 1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| {
                    PhotonError::Query("classic-hist counter: __v not Float64".into())
                })?;
            let st = b
                .column(n_group + 2)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("classic-hist counter: __st not Int64".into()))?;
            let gcols: Vec<&StringArray> = (0..n_group)
                .map(|g| b.column(g).as_any().downcast_ref::<StringArray>())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    PhotonError::Query("classic-hist counter: group col not Utf8".into())
                })?;

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
                        if out.len() < MAX_SERIES {
                            out.push((k, std::mem::take(&mut cur_rows)));
                        } else {
                            capped = true;
                            cur_rows.clear();
                        }
                    }
                    cur_key = Some(key);
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
            if out.len() < MAX_SERIES {
                out.push((k, cur_rows));
            } else {
                capped = true;
            }
        }
        Ok((out, capped))
    }

    /// Count/Sum: read one companion counter (`<base>_count` / `<base>_sum`) and roll each series
    /// up with the reset-aware increase rule — one `SeriesResult` per group key.
    async fn classic_counter_rollup(
        &self,
        base: &MetricRequest,
        group_by: &[String],
        start: i64,
        end: i64,
        buckets: usize,
    ) -> Result<(Vec<SeriesResult>, bool), PhotonError> {
        let step = step_secs(start, end, buckets);
        let (rows_by_key, capped) = self.collect_value_series(base, group_by).await?;
        let series = rows_by_key
            .into_iter()
            .map(|(key, rows)| SeriesResult {
                labels: labels_from(group_by, &key),
                points: reset_aware_series(&rows, start, end, buckets, false, step),
            })
            .collect();
        Ok((series, capped))
    }

    /// Avg: per group key, per bucket, `<base>_sum` increase ÷ `<base>_count` increase (gap where
    /// the count increase is ≤ 0 or a series is missing its sum companion).
    async fn classic_avg(
        &self,
        req: &MetricSeriesRequest,
        group_by: &[String],
        start: i64,
        end: i64,
        buckets: usize,
    ) -> Result<(Vec<SeriesResult>, bool), PhotonError> {
        let step = step_secs(start, end, buckets);
        let count_base = companion_request(req, count_name(&req.metric));
        let sum_base = companion_request(req, sum_name(&req.metric));
        let (counts, c1) = self.collect_value_series(&count_base, group_by).await?;
        let (sums_vec, c2) = self.collect_value_series(&sum_base, group_by).await?;
        let sums: BTreeMap<Vec<Option<String>>, Vec<PointRow>> = sums_vec.into_iter().collect();

        let series = counts
            .into_iter()
            .map(|(key, count_rows)| {
                let count_pts = reset_aware_series(&count_rows, start, end, buckets, false, step);
                let sum_pts = sums
                    .get(&key)
                    .map(|r| reset_aware_series(r, start, end, buckets, false, step));
                let points = (0..buckets)
                    .map(|i| {
                        let c = count_pts[i].v;
                        let s = sum_pts.as_ref().and_then(|sp| sp[i].v);
                        let v = match (s, c) {
                            (Some(s), Some(c)) if c > 0.0 => Some(s / c),
                            _ => None,
                        };
                        SeriesPoint {
                            t: bucket_start(start, end, buckets, i),
                            v,
                        }
                    })
                    .collect();
                SeriesResult {
                    labels: labels_from(group_by, &key),
                    points,
                }
            })
            .collect();
        Ok((series, c1 || c2))
    }

    /// Percentiles: read `<base>_bucket` rows (projecting the `le` label + `value`), group by
    /// `(group key, le)`, reset-aware-increase each `(group key, le)` per time bucket, then per
    /// `(group key, time bucket)` collect the per-`le` cumulative increases and interpolate with
    /// `classic_quantile(q)`. Rows whose `le` is null/garbage are skipped.
    async fn classic_percentiles(
        &self,
        req: &MetricSeriesRequest,
        group_by: &[String],
        q: f64,
        start: i64,
        end: i64,
        buckets: usize,
    ) -> Result<(Vec<SeriesResult>, bool), PhotonError> {
        let step = step_secs(start, end, buckets);
        let base = companion_request(req, bucket_name(&req.metric));
        let Some(df) = self.survivors_df(&base).await? else {
            return Ok((Vec::new(), false));
        };
        let n_group = group_by.len();
        let group_cols = self.resolve_group_cols(group_by)?;
        // Resolve the `le` label to its column (a map attribute unless promoted).
        let resolver = MetricFieldResolver::new(self.promoted_attributes());
        let le_field = resolver
            .resolve_field_name("le")
            .map_err(|e| PhotonError::Query(format!("cannot resolve `le` label: {}", e.message)))?;

        let mut selects: Vec<Expr> = group_cols
            .iter()
            .enumerate()
            .map(|(i, e)| e.clone().alias(format!("__g{i}")))
            .collect();
        selects.push(metric_field_col(&le_field).alias("__le"));
        selects.push(cast(col_ref(metric_schema::TIMESTAMP), DataType::Int64).alias("__ts"));
        selects.push(col_ref(metric_schema::VALUE).alias("__v"));
        selects.push(cast(col_ref(metric_schema::START_TIMESTAMP), DataType::Int64).alias("__st"));

        let mut sorts: Vec<_> = (0..n_group)
            .map(|i| col(format!("__g{i}")).sort(true, true))
            .collect();
        sorts.push(col("__le").sort(true, true));
        sorts.push(col("__ts").sort(true, false));

        let batches = df
            .filter(metric_base_predicate(&base))
            .map_err(|e| PhotonError::Query(format!("classic-hist bucket filter: {e}")))?
            .select(selects)
            .map_err(|e| PhotonError::Query(format!("classic-hist bucket select: {e}")))?
            .sort(sorts)
            .map_err(|e| PhotonError::Query(format!("classic-hist bucket sort: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("classic-hist bucket collect: {e}")))?;

        // group key -> Vec<(le, per-bucket cumulative increase)>. Sorted by (group key, le, ts) so
        // every (group key, le) run is contiguous.
        type Acc = BTreeMap<Vec<Option<String>>, Vec<(f64, Vec<SeriesPoint>)>>;
        let mut acc: Acc = BTreeMap::new();
        let mut capped = false;
        let mut cur: Option<(Vec<Option<String>>, Option<String>)> = None;
        let mut cur_rows: Vec<PointRow> = Vec::new();

        let flush = |acc: &mut Acc,
                     capped: &mut bool,
                     gkey: Vec<Option<String>>,
                     le: Option<String>,
                     rows: Vec<PointRow>| {
            let Some(le_str) = le else { return }; // null le → skip these rows
            let Some(le_f64) = parse_le(&le_str) else {
                return;
            }; // garbage le → skip
            if !acc.contains_key(&gkey) && acc.len() >= MAX_SERIES {
                *capped = true;
                return;
            }
            let pts = reset_aware_series(&rows, start, end, buckets, false, step);
            acc.entry(gkey).or_default().push((le_f64, pts));
        };

        for b in &batches {
            let le = b
                .column(n_group)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PhotonError::Query("classic-hist bucket: __le not Utf8".into()))?;
            let ts = b
                .column(n_group + 1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("classic-hist bucket: __ts not Int64".into()))?;
            let v = b
                .column(n_group + 2)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| PhotonError::Query("classic-hist bucket: __v not Float64".into()))?;
            let st = b
                .column(n_group + 3)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("classic-hist bucket: __st not Int64".into()))?;
            let gcols: Vec<&StringArray> = (0..n_group)
                .map(|g| b.column(g).as_any().downcast_ref::<StringArray>())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    PhotonError::Query("classic-hist bucket: group col not Utf8".into())
                })?;

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
                let le_val = if le.is_null(i) {
                    None
                } else {
                    Some(le.value(i).to_string())
                };
                let this = (key, le_val);
                if cur.as_ref() != Some(&this) {
                    if let Some((gk, l)) = cur.take() {
                        flush(&mut acc, &mut capped, gk, l, std::mem::take(&mut cur_rows));
                    }
                    cur = Some(this);
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
        if let Some((gk, l)) = cur.take() {
            flush(&mut acc, &mut capped, gk, l, cur_rows);
        }

        let series = acc
            .into_iter()
            .map(|(key, les)| {
                let points = (0..buckets)
                    .map(|i| {
                        // Per-`le` cumulative increase for this time bucket (skip les with no data).
                        let mut pairs: Vec<(f64, f64)> = Vec::with_capacity(les.len());
                        for (le, pts) in &les {
                            if let Some(val) = pts[i].v {
                                pairs.push((*le, val));
                            }
                        }
                        SeriesPoint {
                            t: bucket_start(start, end, buckets, i),
                            v: classic_quantile(&pairs, q),
                        }
                    })
                    .collect();
                SeriesResult {
                    labels: labels_from(group_by, &key),
                    points,
                }
            })
            .collect();

        Ok((series, capped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_le_handles_inf_and_numbers() {
        assert_eq!(parse_le("+Inf"), Some(f64::INFINITY));
        assert_eq!(parse_le("0.5"), Some(0.5));
        assert_eq!(parse_le("100"), Some(100.0));
        assert_eq!(parse_le("nonsense"), None);
    }

    #[test]
    fn classic_base_strips_bucket_suffix() {
        assert_eq!(
            classic_base("http_req_duration_seconds_bucket"),
            Some("http_req_duration_seconds")
        );
        assert_eq!(classic_base("http_req_total"), None);
    }

    #[test]
    fn quantile_median_interpolates_within_the_bucket() {
        // Cumulative: le=1 →10, le=2 →30, le=+Inf →30. Per-bucket: (0,1]=10, (1,2]=20, overflow=0.
        // total=30, median rank=15 lands in (1,2] at frac (15-10)/20=0.25 → 1 + 1*0.25 = 1.25.
        let cum = [(1.0, 10.0), (2.0, 30.0), (f64::INFINITY, 30.0)];
        let q = classic_quantile(&cum, 0.5).unwrap();
        assert!((q - 1.25).abs() < 1e-9, "got {q}");
    }

    #[test]
    fn quantile_p90_unsorted_input_ok() {
        // Same data, shuffled; p90 rank = 27 lands in (1,2] at (27-10)/20=0.85 → 1.85.
        let cum = [(f64::INFINITY, 30.0), (2.0, 30.0), (1.0, 10.0)];
        let q = classic_quantile(&cum, 0.9).unwrap();
        assert!((q - 1.85).abs() < 1e-9, "got {q}");
    }

    #[test]
    fn quantile_in_overflow_bucket_returns_lower_bound() {
        // le=1 →5, le=+Inf →10. p99 rank=9.9 is in the overflow (1,+Inf] → returns lower bound 1.0.
        let cum = [(1.0, 5.0), (f64::INFINITY, 10.0)];
        assert_eq!(classic_quantile(&cum, 0.99), Some(1.0));
    }

    #[test]
    fn quantile_empty_or_zero_total_is_none() {
        assert_eq!(classic_quantile(&[], 0.5), None);
        assert_eq!(
            classic_quantile(&[(1.0, 0.0), (f64::INFINITY, 0.0)], 0.5),
            None
        );
    }

    #[test]
    fn duplicate_infinity_le_does_not_panic() {
        // Two infinite `le`s for the same (group, bucket) — adversarial/misbehaving remote-write
        // input. Both fold into one overflow bucket instead of producing an over-long `counts`,
        // so `hist_ranges` never indexes past `bounds` (regression test for the OOB panic).
        let cum = [(1.0, 10.0), (f64::INFINITY, 15.0), (f64::INFINITY, 20.0)];
        let q = classic_quantile(&cum, 0.5);
        // total=20, median rank=10 lands exactly at the top of the finite bucket (0,1] → 1.0.
        assert_eq!(q, Some(1.0), "got {q:?}");
    }

    #[test]
    fn nan_le_is_rejected_by_parse_le() {
        assert_eq!(parse_le("NaN"), None);
        assert_eq!(parse_le("+Inf"), Some(f64::INFINITY));
        assert_eq!(parse_le("0.5"), Some(0.5));
    }
}

#[cfg(test)]
mod engine_tests {
    use crate::metric_engine::MetricsQueryEngine;
    use crate::metric_query::MetricSeriesRequest;
    use photon_core::metric_agg::Agg;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::metric_schema::{metric_type, MetricSchema};

    /// One cumulative `<name>` point (SUM, cumulative, monotonic) for `svc` at `ts` with an
    /// optional `le` attribute and value `v`.
    fn bpoint(name: &str, svc: &str, le: Option<&str>, ts: i64, v: f64) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        if let Some(l) = le {
            attributes.insert("le".to_string(), l.to_string());
        }
        MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::SUM,
            temporality: Some(2),
            is_monotonic: Some(true),
            timestamp_nanos: ts,
            start_timestamp_nanos: Some(0),
            value: Some(v),
            attributes,
            ..Default::default()
        }
    }

    // A classic histogram `h` for service `a`: at t=0 all buckets 0; at t=100 cumulative
    // le=1→10, le=2→30, le=+Inf→30, plus h_sum→45, h_count→30. Window [0,200], 2 buckets.
    fn classic_engine() -> MetricsQueryEngine {
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        for p in [
            bpoint("h_bucket", "a", Some("1"), 0, 0.0),
            bpoint("h_bucket", "a", Some("2"), 0, 0.0),
            bpoint("h_bucket", "a", Some("+Inf"), 0, 0.0),
            bpoint("h_bucket", "a", Some("1"), 100, 10.0),
            bpoint("h_bucket", "a", Some("2"), 100, 30.0),
            bpoint("h_bucket", "a", Some("+Inf"), 100, 30.0),
            bpoint("h_sum", "a", None, 0, 0.0),
            bpoint("h_sum", "a", None, 100, 45.0),
            bpoint("h_count", "a", None, 0, 0.0),
            bpoint("h_count", "a", None, 100, 30.0),
        ] {
            b.append(&p);
        }
        MetricsQueryEngine::from_batch(schema, b.finish().unwrap())
    }

    async fn series1(agg: Agg) -> crate::metric_query::QuerySeriesResult {
        classic_engine()
            .query_series(MetricSeriesRequest {
                metric: "h".into(),
                agg: Some(agg),
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 200,
                buckets: 2,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn p50_reassembles_from_le_buckets() {
        let r = series1(Agg::P50).await;
        assert_eq!(r.chosen_agg, Agg::P50);
        let s = &r.series[0];
        // bucket 0 (t=0): all-zero increase → total 0 → None.
        assert_eq!(s.points[0].v, None);
        // bucket 1 (t=100): increases le1=10,le2=20,ovf=0; median rank 15 in (1,2] → 1.25.
        let v = s.points[1].v.unwrap();
        assert!((v - 1.25).abs() < 1e-9, "got {v}");
    }

    #[tokio::test]
    async fn count_sum_avg_from_companion_series() {
        let c = series1(Agg::Count).await.series[0].points[1].v.unwrap();
        assert!((c - 30.0).abs() < 1e-9, "count {c}");
        let sum = series1(Agg::Sum).await.series[0].points[1].v.unwrap();
        assert!((sum - 45.0).abs() < 1e-9, "sum {sum}");
        let avg = series1(Agg::Avg).await.series[0].points[1].v.unwrap();
        assert!((avg - 1.5).abs() < 1e-9, "avg {avg}"); // 45/30
    }

    #[tokio::test]
    async fn default_agg_is_p99_for_classic_histogram() {
        let r = classic_engine()
            .query_series(MetricSeriesRequest {
                metric: "h".into(),
                agg: None,
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 200,
                buckets: 2,
            })
            .await
            .unwrap();
        assert_eq!(r.default_agg, Agg::P99);
        assert_eq!(r.chosen_agg, Agg::P99);
    }

    #[tokio::test]
    async fn engine_clamps_a_dos_sized_bucket_count() {
        // Defense-in-depth for the classic-histogram reassembly path: a caller-supplied
        // `buckets = 10_000_000` must not drive a multi-million-entry `Vec<SeriesPoint>` per
        // series (the `(0..buckets)` builds inside `classic_avg`/`classic_percentiles` and
        // `classic_counter_rollup`'s `reset_aware_series` all scale with `buckets`).
        let r = classic_engine()
            .query_series(MetricSeriesRequest {
                metric: "h".into(),
                agg: Some(Agg::P50),
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 200,
                buckets: 10_000_000,
            })
            .await
            .unwrap();
        assert!(
            r.series[0].points.len() <= 3000,
            "buckets must be clamped to MAX_BUCKETS, got {}",
            r.series[0].points.len()
        );
    }
}

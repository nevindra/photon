//! Distribution query paths — explicit Histogram, Exponential Histogram, Summary. Pure quantile
//! math (table-tested here) plus the engine scan methods (added in later tasks). Label work stays
//! in DataFusion, so series partition by the resolved group-by columns, exactly like
//! `metric_query::query_series_pointwise`.

use std::collections::BTreeMap;

use arrow::array::{Array, Int64Array, StringArray};
use arrow::datatypes::DataType;
use datafusion::prelude::{cast, col};

use photon_core::metric_agg::Agg;
use photon_core::metric_schema;
use photon_core::PhotonError;

use crate::metric_engine::{metric_base_predicate, MetricRequest};
use crate::metric_query::{
    bucket_of, bucket_start, MetricSeriesRequest, ProbeMeta, QuerySeriesResult, SeriesPoint,
    SeriesResult, MAX_SERIES, TEMPORALITY_CUMULATIVE,
};
use crate::{col_ref, MetricsQueryEngine};

/// Read-side histogram payload (a subset of what the ingest side serializes).
#[derive(serde::Deserialize)]
pub(crate) struct HistPayload {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub sum: Option<f64>,
    #[serde(default)]
    pub bucket_counts: Vec<u64>,
    #[serde(default)]
    pub explicit_bounds: Vec<f64>,
}

/// The target quantile for a quantile aggregation, or `None` for non-quantile aggs.
pub(crate) fn quantile_of(agg: Agg) -> Option<f64> {
    match agg {
        Agg::P50 | Agg::Median => Some(0.5),
        Agg::P90 => Some(0.9),
        Agg::P99 => Some(0.99),
        _ => None,
    }
}

/// Value-ascending `(lower, upper, count)` ranges for an explicit-bucket histogram. `counts` has
/// one entry per bucket (usually `bounds.len()+1`): bucket 0 is `(0, bounds[0]]`, bucket `i` is
/// `(bounds[i-1], bounds[i]]`, the overflow bucket is `(bounds[last], +Inf]`.
pub(crate) fn hist_ranges(bounds: &[f64], counts: &[f64]) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::with_capacity(counts.len());
    for (i, &c) in counts.iter().enumerate() {
        let lo = if i == 0 { 0.0 } else { bounds[i - 1] };
        let hi = bounds.get(i).copied().unwrap_or(f64::INFINITY);
        out.push((lo, hi, c));
    }
    out
}

/// Linear-interpolate quantile `q` (clamped to 0..=1) from value-ascending `(lower, upper, count)`
/// ranges. `None` when total count is zero. A range whose `upper` is `+Inf` (the overflow bucket)
/// yields its `lower` when the rank lands inside it — you cannot interpolate to infinity.
pub(crate) fn interpolate_quantile(ranges: &[(f64, f64, f64)], q: f64) -> Option<f64> {
    let total: f64 = ranges.iter().map(|r| r.2).sum();
    if total <= 0.0 {
        return None;
    }
    let rank = (q.clamp(0.0, 1.0) * total).clamp(0.0, total);
    let mut cum = 0.0;
    for &(lo, hi, c) in ranges {
        if c <= 0.0 {
            continue;
        }
        if cum + c >= rank {
            if hi.is_infinite() {
                return Some(lo);
            }
            let frac = (rank - cum) / c;
            return Some(lo + (hi - lo) * frac);
        }
        cum += c;
    }
    // rank == total (floating slack): the top non-empty range's upper (or lower if +Inf).
    ranges
        .iter()
        .rev()
        .find(|r| r.2 > 0.0)
        .map(|&(lo, hi, _)| if hi.is_infinite() { lo } else { hi })
}

/// One raw distribution sample of a single series (already time-ordered), carrying the JSON
/// payload column (`histogram` / `exp_histogram` / `summary`). Null payloads are skipped at scan.
pub(crate) struct DistRow {
    pub ts: i64,
    pub st: Option<i64>,
    pub json: String,
}

impl MetricsQueryEngine {
    /// Scan pruned survivors for one metric, projecting `group_by` label columns + timestamp +
    /// start_timestamp + the given JSON payload column, sorted by `(group key, timestamp)`. Groups
    /// rows into per-series (`labels`, time-ordered `rows`). Series identity = the resolved group
    /// columns (same as `query_series_pointwise`); capped at `MAX_SERIES`.
    pub(crate) async fn collect_dist_series(
        &self,
        base: &MetricRequest,
        group_by: &[String],
        json_col: &str,
    ) -> Result<(Vec<(BTreeMap<String, String>, Vec<DistRow>)>, bool), PhotonError> {
        let Some(df) = self.survivors_df(base).await? else {
            return Ok((Vec::new(), false));
        };
        let n_group = group_by.len();
        let group_cols = self.resolve_group_cols(group_by)?;
        let mut selects: Vec<_> = group_cols
            .iter()
            .enumerate()
            .map(|(i, e)| e.clone().alias(format!("__g{i}")))
            .collect();
        selects.push(cast(col_ref(metric_schema::TIMESTAMP), DataType::Int64).alias("__ts"));
        selects.push(cast(col_ref(metric_schema::START_TIMESTAMP), DataType::Int64).alias("__st"));
        selects.push(col_ref(json_col).alias("__j"));

        let mut sorts: Vec<_> = (0..n_group)
            .map(|i| col(format!("__g{i}")).sort(true, true))
            .collect();
        sorts.push(col("__ts").sort(true, false));

        let batches = df
            .filter(metric_base_predicate(base))
            .map_err(|e| PhotonError::Query(format!("dist filter: {e}")))?
            .select(selects)
            .map_err(|e| PhotonError::Query(format!("dist select: {e}")))?
            .sort(sorts)
            .map_err(|e| PhotonError::Query(format!("dist sort: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("dist collect: {e}")))?;

        let mut out: Vec<(BTreeMap<String, String>, Vec<DistRow>)> = Vec::new();
        let mut cur_key: Option<Vec<Option<String>>> = None;
        let mut cur_rows: Vec<DistRow> = Vec::new();
        let mut capped = false;

        let flush = |out: &mut Vec<(BTreeMap<String, String>, Vec<DistRow>)>,
                     capped: &mut bool,
                     key: Vec<Option<String>>,
                     rows: Vec<DistRow>| {
            if out.len() >= MAX_SERIES {
                *capped = true;
                return;
            }
            let mut labels = BTreeMap::new();
            for (name, val) in group_by.iter().zip(key) {
                if let Some(v) = val {
                    labels.insert(name.clone(), v);
                }
            }
            out.push((labels, rows));
        };

        for b in &batches {
            let ts = b
                .column(n_group)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("dist: __ts not Int64".into()))?;
            let st = b
                .column(n_group + 1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query("dist: __st not Int64".into()))?;
            let j = b
                .column(n_group + 2)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PhotonError::Query("dist: __j not Utf8".into()))?;
            let gcols: Vec<&StringArray> = (0..n_group)
                .map(|g| b.column(g).as_any().downcast_ref::<StringArray>())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| PhotonError::Query("dist: group col not Utf8".into()))?;

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
                        flush(&mut out, &mut capped, k, std::mem::take(&mut cur_rows));
                    }
                    cur_key = Some(key);
                }
                if j.is_null(i) {
                    continue; // no distribution payload → skip
                }
                cur_rows.push(DistRow {
                    ts: ts.value(i),
                    st: if st.is_null(i) {
                        None
                    } else {
                        Some(st.value(i))
                    },
                    json: j.value(i).to_string(),
                });
            }
        }
        if let Some(k) = cur_key.take() {
            flush(&mut out, &mut capped, k, cur_rows);
        }
        Ok((out, capped))
    }

    /// Histogram series query: p50/p90/p99 (interpolated) + count/sum/avg, per group per bucket.
    /// 8 args mirrors `query_series_pointwise`'s shape plus `base` (needed to re-scan by JSON
    /// column here rather than `value`); the exp-histogram/summary siblings share this signature.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn query_series_histogram(
        &self,
        base: &MetricRequest,
        req: &MetricSeriesRequest,
        meta: ProbeMeta,
        default: Agg,
        chosen: Agg,
        step_nanos: i64,
        buckets: usize,
    ) -> Result<QuerySeriesResult, PhotonError> {
        // Reject aggs a histogram cannot answer (rate/increase/min/max/last are meaningless here).
        if !matches!(
            chosen,
            Agg::P50 | Agg::P90 | Agg::P99 | Agg::Median | Agg::Count | Agg::Sum | Agg::Avg
        ) {
            return Err(PhotonError::Query(format!(
                "aggregation `{}` is not supported for histogram metrics (use p50/p90/p99/count/sum/avg)",
                chosen.as_str()
            )));
        }
        let cumulative = meta.temporality == Some(TEMPORALITY_CUMULATIVE);
        let (groups, capped) = self
            .collect_dist_series(base, &req.group_by, metric_schema::HISTOGRAM)
            .await?;
        let series = groups
            .into_iter()
            .map(|(labels, rows)| SeriesResult {
                labels,
                points: histogram_series(
                    &rows,
                    cumulative,
                    chosen,
                    req.start_ts_nanos,
                    req.end_ts_nanos,
                    buckets,
                ),
            })
            .collect();
        Ok(QuerySeriesResult {
            series,
            default_agg: default,
            chosen_agg: chosen,
            step_nanos,
            capped,
        })
    }
}

/// One-group histogram roll-up. Parses each row's `histogram` JSON, buckets contributions in time,
/// and per bucket answers the aggregation. For DELTA temporality each point's `bucket_counts` are
/// summed element-wise into the point's time bucket; for CUMULATIVE they are reset-aware-delta'd
/// across consecutive samples (a `count` decrease, an advanced `start_timestamp`, or a changed
/// bound-vector length is a reset → the new counts are the contribution; the first sample
/// contributes 0). Canonical bounds are taken from the first parsed row.
pub(crate) fn histogram_series(
    rows: &[DistRow],
    cumulative: bool,
    agg: Agg,
    start: i64,
    end: i64,
    buckets: usize,
) -> Vec<SeriesPoint> {
    let parsed: Vec<(i64, Option<i64>, HistPayload)> = rows
        .iter()
        .filter_map(|r| {
            serde_json::from_str::<HistPayload>(&r.json)
                .ok()
                .map(|p| (r.ts, r.st, p))
        })
        .collect();
    let empty = || {
        (0..buckets)
            .map(|i| SeriesPoint {
                t: bucket_start(start, end, buckets, i),
                v: None,
            })
            .collect::<Vec<_>>()
    };
    let Some(bounds) = parsed
        .iter()
        .find(|(_, _, p)| !p.explicit_bounds.is_empty())
        .map(|(_, _, p)| p.explicit_bounds.clone())
    else {
        return empty();
    };
    let nb = bounds.len() + 1;

    let mut counts = vec![vec![0f64; nb]; buckets];
    let mut hsum = vec![0f64; buckets];
    let mut hcount = vec![0f64; buckets];
    let mut has = vec![false; buckets];

    if cumulative {
        let mut prev: Option<(Vec<f64>, f64, f64, Option<i64>)> = None;
        for (ts, st, p) in &parsed {
            let b = bucket_of(*ts, start, end, buckets);
            let cur: Vec<f64> = p.bucket_counts.iter().map(|&c| c as f64).collect();
            let (inc_counts, inc_sum, inc_count) = match &prev {
                None => (vec![0f64; cur.len()], 0.0, 0.0),
                Some((pc, ps, pcnt, pst)) => {
                    let reset = (p.count as f64) < *pcnt
                        || (st.is_some() && pst.is_some() && *st > *pst)
                        || cur.len() != pc.len();
                    if reset {
                        (
                            cur.clone(),
                            p.sum.unwrap_or(0.0).max(0.0),
                            (p.count as f64).max(0.0),
                        )
                    } else {
                        let d = cur.iter().zip(pc).map(|(c, o)| (c - o).max(0.0)).collect();
                        (
                            d,
                            (p.sum.unwrap_or(0.0) - *ps).max(0.0),
                            (p.count as f64 - *pcnt).max(0.0),
                        )
                    }
                }
            };
            add_into(&mut counts[b], &inc_counts);
            hsum[b] += inc_sum;
            hcount[b] += inc_count;
            has[b] = true;
            prev = Some((cur, p.sum.unwrap_or(0.0), p.count as f64, *st));
        }
    } else {
        for (ts, _st, p) in &parsed {
            let b = bucket_of(*ts, start, end, buckets);
            let cur: Vec<f64> = p.bucket_counts.iter().map(|&c| c as f64).collect();
            add_into(&mut counts[b], &cur);
            hsum[b] += p.sum.unwrap_or(0.0);
            hcount[b] += p.count as f64;
            has[b] = true;
        }
    }

    (0..buckets)
        .map(|i| {
            let v = if !has[i] {
                None
            } else {
                match agg {
                    Agg::Count => Some(hcount[i]),
                    Agg::Sum => Some(hsum[i]),
                    Agg::Avg => (hcount[i] > 0.0).then(|| hsum[i] / hcount[i]),
                    _ => match quantile_of(agg) {
                        Some(q) => interpolate_quantile(&hist_ranges(&bounds, &counts[i]), q),
                        None => None,
                    },
                }
            };
            SeriesPoint {
                t: bucket_start(start, end, buckets, i),
                v,
            }
        })
        .collect()
}

/// Element-wise `acc += add`, tolerating length mismatch (extra `add` entries are ignored).
fn add_into(acc: &mut [f64], add: &[f64]) {
    for (a, b) in acc.iter_mut().zip(add) {
        *a += *b;
    }
}

impl MetricsQueryEngine {
    /// Exp-histogram series query: p50/p90/p99 (merge + interpolate) + count/sum/avg. Mirrors
    /// `query_series_histogram`, re-scanning by the `exp_histogram` JSON column.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn query_series_exp_histogram(
        &self,
        base: &MetricRequest,
        req: &MetricSeriesRequest,
        meta: ProbeMeta,
        default: Agg,
        chosen: Agg,
        step_nanos: i64,
        buckets: usize,
    ) -> Result<QuerySeriesResult, PhotonError> {
        if !matches!(
            chosen,
            Agg::P50 | Agg::P90 | Agg::P99 | Agg::Median | Agg::Count | Agg::Sum | Agg::Avg
        ) {
            return Err(PhotonError::Query(format!(
                "aggregation `{}` is not supported for exponential-histogram metrics (use p50/p90/p99/count/sum/avg)",
                chosen.as_str()
            )));
        }
        let cumulative = meta.temporality == Some(TEMPORALITY_CUMULATIVE);
        let (groups, capped) = self
            .collect_dist_series(base, &req.group_by, metric_schema::EXP_HISTOGRAM)
            .await?;
        let series = groups
            .into_iter()
            .map(|(labels, rows)| SeriesResult {
                labels,
                points: exp_histogram_series(
                    &rows,
                    cumulative,
                    chosen,
                    req.start_ts_nanos,
                    req.end_ts_nanos,
                    buckets,
                ),
            })
            .collect();
        Ok(QuerySeriesResult {
            series,
            default_agg: default,
            chosen_agg: chosen,
            step_nanos,
            capped,
        })
    }
}

/// One-group exp-histogram roll-up. Per time bucket it merges the contributing payloads into an
/// `ExpAccum` (aligning scales) and answers the aggregation. DELTA sums each point into its
/// bucket; CUMULATIVE reset-aware-deltas `count`/`sum` across consecutive samples and feeds the
/// current payload into the bucket (a `count` decrease, advanced `start_timestamp`, or scale change
/// is a reset → the sample itself is the contribution; the first sample contributes nothing).
pub(crate) fn exp_histogram_series(
    rows: &[DistRow],
    cumulative: bool,
    agg: Agg,
    start: i64,
    end: i64,
    buckets: usize,
) -> Vec<SeriesPoint> {
    let parsed: Vec<(i64, Option<i64>, ExpPayload)> = rows
        .iter()
        .filter_map(|r| {
            serde_json::from_str::<ExpPayload>(&r.json)
                .ok()
                .map(|p| (r.ts, r.st, p))
        })
        .collect();

    let mut accs: Vec<ExpAccum> = (0..buckets).map(|_| ExpAccum::new()).collect();
    let mut hsum = vec![0f64; buckets];
    let mut hcount = vec![0f64; buckets];
    let mut has = vec![false; buckets];

    if cumulative {
        // prev cumulative totals; we contribute delta count/sum, and (approximately) the current
        // bucket structure as the incremental distribution. Interleaving caveat matches the
        // explicit-histogram path.
        let mut prev: Option<(f64, f64, i32, Option<i64>)> = None; // (count, sum, scale, st)
        for (ts, st, p) in &parsed {
            let b = bucket_of(*ts, start, end, buckets);
            let (inc_count, inc_sum, contribute) = match &prev {
                None => (0.0, 0.0, false),
                Some((pc, ps, pscale, pst)) => {
                    let reset = (p.count as f64) < *pc
                        || (st.is_some() && pst.is_some() && *st > *pst)
                        || p.scale != *pscale;
                    if reset {
                        (p.count as f64, p.sum.unwrap_or(0.0), true)
                    } else {
                        (
                            (p.count as f64 - *pc).max(0.0),
                            (p.sum.unwrap_or(0.0) - *ps).max(0.0),
                            true,
                        )
                    }
                }
            };
            if contribute {
                accs[b].add(p);
                hsum[b] += inc_sum;
                hcount[b] += inc_count;
            }
            has[b] = true;
            prev = Some((p.count as f64, p.sum.unwrap_or(0.0), p.scale, *st));
        }
    } else {
        for (ts, _st, p) in &parsed {
            let b = bucket_of(*ts, start, end, buckets);
            accs[b].add(p);
            hsum[b] += p.sum.unwrap_or(0.0);
            hcount[b] += p.count as f64;
            has[b] = true;
        }
    }

    (0..buckets)
        .map(|i| {
            let v = if !has[i] {
                None
            } else {
                match agg {
                    Agg::Count => Some(hcount[i]),
                    Agg::Sum => Some(hsum[i]),
                    Agg::Avg => (hcount[i] > 0.0).then(|| hsum[i] / hcount[i]),
                    _ => match quantile_of(agg) {
                        Some(q) => interpolate_quantile(&accs[i].ranges(), q),
                        None => None,
                    },
                }
            };
            SeriesPoint {
                t: bucket_start(start, end, buckets, i),
                v,
            }
        })
        .collect()
}

#[derive(serde::Deserialize)]
pub(crate) struct BucketsPayload {
    #[serde(default)]
    pub offset: i32,
    #[serde(default)]
    pub bucket_counts: Vec<u64>,
}

#[derive(serde::Deserialize)]
pub(crate) struct ExpPayload {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub sum: Option<f64>,
    #[serde(default)]
    pub scale: i32,
    #[serde(default)]
    pub zero_count: u64,
    #[serde(default)]
    pub positive: Option<BucketsPayload>,
    #[serde(default)]
    pub negative: Option<BucketsPayload>,
}

/// Merge adjacent exponential buckets to drop `levels` scale levels. A bucket at absolute index
/// `i` (`= offset + local`) maps to absolute index `i >> levels` at the coarser scale. Returns the
/// coarser `(offset, dense counts)`.
pub(crate) fn exp_downscale(offset: i32, counts: &[f64], levels: u32) -> (i32, Vec<f64>) {
    if levels == 0 || counts.is_empty() {
        return (offset, counts.to_vec());
    }
    // Arithmetic shift toward -inf for negatives (Rust `>>` on i32 is arithmetic).
    let idx = |local: usize| ((offset as i64 + local as i64) >> levels) as i32;
    let new_offset = idx(0);
    let last = idx(counts.len() - 1);
    let len = (last - new_offset + 1).max(0) as usize;
    let mut out = vec![0f64; len];
    for (local, &c) in counts.iter().enumerate() {
        let slot = (idx(local) - new_offset) as usize;
        out[slot] += c;
    }
    (new_offset, out)
}

/// Accumulates exponential-histogram buckets across points/series, aligning to the coarsest scale
/// seen so far. `pos`/`neg` are dense count vectors indexed from `pos_offset`/`neg_offset`.
pub(crate) struct ExpAccum {
    pub scale: i32,
    pub zero: f64,
    pub pos_offset: i32,
    pub pos: Vec<f64>,
    pub neg_offset: i32,
    pub neg: Vec<f64>,
    pub has: bool,
}

impl ExpAccum {
    pub(crate) fn new() -> ExpAccum {
        ExpAccum {
            scale: i32::MAX,
            zero: 0.0,
            pos_offset: 0,
            pos: Vec::new(),
            neg_offset: 0,
            neg: Vec::new(),
            has: false,
        }
    }

    /// Sum one payload in, down-scaling whichever side (accumulator or payload) is finer so both
    /// share the coarser scale.
    pub(crate) fn add(&mut self, p: &ExpPayload) {
        let pv: Vec<f64> = p
            .positive
            .as_ref()
            .map(|b| b.bucket_counts.iter().map(|&c| c as f64).collect())
            .unwrap_or_default();
        let nv: Vec<f64> = p
            .negative
            .as_ref()
            .map(|b| b.bucket_counts.iter().map(|&c| c as f64).collect())
            .unwrap_or_default();
        let p_pos_off = p.positive.as_ref().map(|b| b.offset).unwrap_or(0);
        let p_neg_off = p.negative.as_ref().map(|b| b.offset).unwrap_or(0);

        if !self.has {
            self.scale = p.scale;
            self.zero = p.zero_count as f64;
            self.pos_offset = p_pos_off;
            self.pos = pv;
            self.neg_offset = p_neg_off;
            self.neg = nv;
            self.has = true;
            return;
        }
        let target = self.scale.min(p.scale);
        if self.scale > target {
            let levels = (self.scale - target) as u32;
            let (o, c) = exp_downscale(self.pos_offset, &self.pos, levels);
            self.pos_offset = o;
            self.pos = c;
            let (o, c) = exp_downscale(self.neg_offset, &self.neg, levels);
            self.neg_offset = o;
            self.neg = c;
            self.scale = target;
        }
        let (p_pos_off, pv) = if p.scale > target {
            exp_downscale(p_pos_off, &pv, (p.scale - target) as u32)
        } else {
            (p_pos_off, pv)
        };
        let (p_neg_off, nv) = if p.scale > target {
            exp_downscale(p_neg_off, &nv, (p.scale - target) as u32)
        } else {
            (p_neg_off, nv)
        };
        self.zero += p.zero_count as f64;
        merge_dense(&mut self.pos_offset, &mut self.pos, p_pos_off, &pv);
        merge_dense(&mut self.neg_offset, &mut self.neg, p_neg_off, &nv);
    }

    /// Value-ascending `(lower, upper, count)` ranges: negative buckets (most-negative first), the
    /// zero bucket at 0, then positive buckets.
    pub(crate) fn ranges(&self) -> Vec<(f64, f64, f64)> {
        let base = 2f64.powf(2f64.powi(-self.scale));
        let mut out: Vec<(f64, f64, f64)> = Vec::new();
        // negatives: abs-index i covers (-base^(i+1), -base^i]; ascending value → highest i first.
        for local in (0..self.neg.len()).rev() {
            let i = self.neg_offset + local as i32;
            let (lo, hi) = (-base.powi(i + 1), -base.powi(i));
            out.push((lo, hi, self.neg[local]));
        }
        if self.zero > 0.0 {
            out.push((0.0, 0.0, self.zero));
        }
        for local in 0..self.pos.len() {
            let i = self.pos_offset + local as i32;
            out.push((base.powi(i), base.powi(i + 1), self.pos[local]));
        }
        out
    }
}

/// Add sparse `(offset, counts)` into a dense `(offset, counts)` accumulator, growing as needed.
fn merge_dense(acc_off: &mut i32, acc: &mut Vec<f64>, off: i32, counts: &[f64]) {
    if counts.is_empty() {
        return;
    }
    if acc.is_empty() {
        *acc_off = off;
        *acc = counts.to_vec();
        return;
    }
    let lo = (*acc_off).min(off);
    let hi = (*acc_off + acc.len() as i32).max(off + counts.len() as i32);
    let mut grown = vec![0f64; (hi - lo) as usize];
    for (k, &c) in acc.iter().enumerate() {
        grown[(*acc_off + k as i32 - lo) as usize] += c;
    }
    for (k, &c) in counts.iter().enumerate() {
        grown[(off + k as i32 - lo) as usize] += c;
    }
    *acc_off = lo;
    *acc = grown;
}

#[cfg(test)]
mod math_tests {
    use super::*;

    // Explicit bounds [10,50,100,500] → 5 buckets; per-bucket counts [2,5,10,3,0], total 20.
    // Cumulative: [2,7,17,20,20]. Hand-computed:
    //   p50 rank 10 → bucket 2 (50,100]: 50 + 50*((10-7)/10) = 65
    //   p90 rank 18 → bucket 3 (100,500]: 100 + 400*((18-17)/3) = 233.333…
    //   p99 rank 19.8 → bucket 3: 100 + 400*((19.8-17)/3) = 473.333…
    #[test]
    fn histogram_quantile_interpolates_known_distribution() {
        let bounds = [10.0, 50.0, 100.0, 500.0];
        let counts = [2.0, 5.0, 10.0, 3.0, 0.0];
        let ranges = hist_ranges(&bounds, &counts);
        let p = |q: f64| interpolate_quantile(&ranges, q).unwrap();
        assert!((p(0.5) - 65.0).abs() < 1e-9, "p50 = {}", p(0.5));
        assert!((p(0.9) - 233.3333333333).abs() < 1e-6, "p90 = {}", p(0.9));
        assert!((p(0.99) - 473.3333333333).abs() < 1e-6, "p99 = {}", p(0.99));
    }

    // Rank landing in the +Inf bucket cannot interpolate up → returns the highest finite bound.
    #[test]
    fn histogram_quantile_in_inf_bucket_returns_top_bound() {
        let bounds = [10.0, 50.0, 100.0, 500.0];
        let counts = [0.0, 0.0, 0.0, 0.0, 5.0]; // all mass in (500, +Inf]
        let ranges = hist_ranges(&bounds, &counts);
        assert_eq!(interpolate_quantile(&ranges, 0.5), Some(500.0));
    }

    #[test]
    fn empty_distribution_is_none() {
        let ranges = hist_ranges(&[10.0], &[0.0, 0.0]);
        assert_eq!(interpolate_quantile(&ranges, 0.9), None);
    }

    #[test]
    fn quantile_of_maps_aggs() {
        assert_eq!(quantile_of(Agg::P50), Some(0.5));
        assert_eq!(quantile_of(Agg::Median), Some(0.5));
        assert_eq!(quantile_of(Agg::P90), Some(0.9));
        assert_eq!(quantile_of(Agg::P99), Some(0.99));
        assert_eq!(quantile_of(Agg::Avg), None);
    }

    #[test]
    fn hist_payload_parses() {
        let p: HistPayload = serde_json::from_str(
            r#"{"count":3,"sum":42.0,"bucket_counts":[1,2],"explicit_bounds":[10.0]}"#,
        )
        .unwrap();
        assert_eq!(p.count, 3);
        assert_eq!(p.bucket_counts, vec![1, 2]);
        assert_eq!(p.explicit_bounds, vec![10.0]);
    }
}

#[cfg(test)]
mod exp_math_tests {
    use super::*;

    // scale 0 → base 2. positive offset 0 counts [3,4,2] → buckets (1,2],(2,4],(4,8].
    // zero_count 1. total 10. Cumulative: zero(1),(3)→4,(4)→8,(2)→10.
    //   p50 rank 5 → bucket (2,4]: 2 + 2*((5-4)/4) = 2.5
    //   p90 rank 9 → bucket (4,8]: 4 + 4*((9-8)/2) = 6.0
    #[test]
    fn exp_quantile_scale0_known() {
        let p: ExpPayload = serde_json::from_str(
            r#"{"count":10,"sum":0,"scale":0,"zero_count":1,"positive":{"offset":0,"bucket_counts":[3,4,2]}}"#,
        ).unwrap();
        let mut acc = ExpAccum::new();
        acc.add(&p);
        let r = acc.ranges();
        assert!((interpolate_quantile(&r, 0.5).unwrap() - 2.5).abs() < 1e-9);
        assert!((interpolate_quantile(&r, 0.9).unwrap() - 6.0).abs() < 1e-9);
    }

    // Downscale by one level merges adjacent buckets: indices [0,1,2,3]→[0,0,1,1].
    #[test]
    fn exp_downscale_merges_adjacent() {
        let (offset, counts) = exp_downscale(0, &[1.0, 1.0, 1.0, 1.0], 1);
        assert_eq!(offset, 0);
        assert_eq!(counts, vec![2.0, 2.0]);
    }

    // Merging two payloads at different scales aligns to the coarser (lower) scale.
    #[test]
    fn exp_accum_merges_mixed_scales() {
        let fine: ExpPayload = serde_json::from_str(
            r#"{"count":4,"sum":0,"scale":1,"zero_count":0,"positive":{"offset":0,"bucket_counts":[1,1,1,1]}}"#,
        ).unwrap();
        let coarse: ExpPayload = serde_json::from_str(
            r#"{"count":2,"sum":0,"scale":0,"zero_count":0,"positive":{"offset":0,"bucket_counts":[2,0]}}"#,
        ).unwrap();
        let mut acc = ExpAccum::new();
        acc.add(&fine);
        acc.add(&coarse);
        assert_eq!(acc.scale, 0, "aligned to the coarser scale");
        // fine downscaled: [1,1,1,1]@s1 → [2,2]@s0; + coarse [2,0] → [4,2].
        assert_eq!(acc.pos_offset, 0);
        assert_eq!(acc.pos, vec![4.0, 2.0]);
    }
}

#[cfg(test)]
mod hist_tests {
    use super::*;
    use crate::metric_query::{MetricSeriesRequest, TEMPORALITY_DELTA};
    use crate::MetricsQueryEngine;
    use photon_core::metric_agg::Agg;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::metric_schema::{metric_type, MetricSchema};

    fn hpoint(svc: &str, ts: i64, temporality: i32, st: i64, hist_json: &str) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        MetricPoint {
            metric_name: "lat".to_string(),
            metric_type: metric_type::HISTOGRAM,
            temporality: Some(temporality),
            start_timestamp_nanos: Some(st),
            timestamp_nanos: ts,
            histogram: Some(hist_json.to_string()),
            attributes,
            ..Default::default()
        }
    }

    fn engine(points: Vec<MetricPoint>) -> MetricsQueryEngine {
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        for p in &points {
            b.append(p);
        }
        MetricsQueryEngine::from_batch(schema, b.finish().unwrap())
    }

    // Pure roll-up: two DELTA points in one bucket, element-wise summed → known p50.
    #[test]
    fn histogram_series_delta_sums_and_interpolates() {
        // bounds [10,50,100,500]; point A counts [1,2,5,1,0], point B [1,3,5,2,0]
        // summed [2,5,10,3,0] → the Task-1 distribution → p50 = 65.
        let a =
            r#"{"count":9,"sum":0,"bucket_counts":[1,2,5,1,0],"explicit_bounds":[10,50,100,500]}"#;
        let b =
            r#"{"count":11,"sum":0,"bucket_counts":[1,3,5,2,0],"explicit_bounds":[10,50,100,500]}"#;
        let rows = vec![
            DistRow {
                ts: 10,
                st: Some(0),
                json: a.into(),
            },
            DistRow {
                ts: 20,
                st: Some(0),
                json: b.into(),
            },
        ];
        // window [0,100], 1 bucket → both land in bucket 0.
        let pts = histogram_series(&rows, false, Agg::P50, 0, 100, 1);
        assert_eq!(pts.len(), 1);
        assert!((pts[0].v.unwrap() - 65.0).abs() < 1e-9);
    }

    // CUMULATIVE reset-aware delta: first sample contributes 0; the delta is [1,3,5,2,0].
    #[test]
    fn histogram_series_cumulative_deltas_consecutive() {
        let s0 =
            r#"{"count":0,"sum":0,"bucket_counts":[0,0,0,0,0],"explicit_bounds":[10,50,100,500]}"#;
        let s1 = r#"{"count":20,"sum":0,"bucket_counts":[2,5,10,3,0],"explicit_bounds":[10,50,100,500]}"#;
        let rows = vec![
            DistRow {
                ts: 10,
                st: Some(0),
                json: s0.into(),
            },
            DistRow {
                ts: 20,
                st: Some(0),
                json: s1.into(),
            },
        ];
        let pts = histogram_series(&rows, true, Agg::P50, 0, 100, 1);
        // delta counts = [2,5,10,3,0] → p50 = 65.
        assert!((pts[0].v.unwrap() - 65.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn query_series_histogram_p99_grouped() {
        let h = |c: &str| {
            format!(
                r#"{{"count":20,"sum":0,"bucket_counts":{c},"explicit_bounds":[10,50,100,500]}}"#
            )
        };
        let engine = engine(vec![
            hpoint("a", 10, TEMPORALITY_DELTA, 0, &h("[2,5,10,3,0]")),
            hpoint("b", 10, TEMPORALITY_DELTA, 0, &h("[0,0,0,0,20]")),
        ]);
        let res = engine
            .query_series(MetricSeriesRequest {
                metric: "lat".into(),
                agg: None, // smart default for histogram = p99
                group_by: vec!["service".into()],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 100,
                buckets: 1,
            })
            .await
            .unwrap();
        assert_eq!(res.default_agg, Agg::P99);
        assert_eq!(res.chosen_agg, Agg::P99);
        assert_eq!(res.series.len(), 2);
        // service "a": p99 of [2,5,10,3,0] over [10,50,100,500] = 473.333…
        let a = res
            .series
            .iter()
            .find(|s| s.labels.get("service").unwrap() == "a")
            .unwrap();
        assert!((a.points[0].v.unwrap() - 473.3333333).abs() < 1e-4);
        // service "b": all mass in +Inf bucket → 500 (top bound).
        let b = res
            .series
            .iter()
            .find(|s| s.labels.get("service").unwrap() == "b")
            .unwrap();
        assert_eq!(b.points[0].v, Some(500.0));
    }

    #[tokio::test]
    async fn query_series_histogram_count_and_sum() {
        let j = r#"{"count":20,"sum":123.5,"bucket_counts":[2,5,10,3,0],"explicit_bounds":[10,50,100,500]}"#;
        let engine = engine(vec![hpoint("a", 10, TEMPORALITY_DELTA, 0, j)]);
        let run = |agg: Agg| {
            let e = &engine;
            async move {
                e.query_series(MetricSeriesRequest {
                    metric: "lat".into(),
                    agg: Some(agg),
                    group_by: vec![],
                    filter: None,
                    start_ts_nanos: 0,
                    end_ts_nanos: 100,
                    buckets: 1,
                })
                .await
                .unwrap()
            }
        };
        assert_eq!(run(Agg::Count).await.series[0].points[0].v, Some(20.0));
        assert_eq!(run(Agg::Sum).await.series[0].points[0].v, Some(123.5));
        // avg = sum/count = 123.5/20 = 6.175
        assert!((run(Agg::Avg).await.series[0].points[0].v.unwrap() - 6.175).abs() < 1e-9);
    }
}

#[cfg(test)]
mod exp_tests {
    use crate::metric_query::{MetricSeriesRequest, TEMPORALITY_CUMULATIVE, TEMPORALITY_DELTA};
    use crate::MetricsQueryEngine;
    use photon_core::metric_agg::Agg;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::metric_schema::{metric_type, MetricSchema};

    fn epoint(svc: &str, ts: i64, json: &str) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        MetricPoint {
            metric_name: "elat".to_string(),
            metric_type: metric_type::EXP_HISTOGRAM,
            temporality: Some(TEMPORALITY_DELTA),
            timestamp_nanos: ts,
            exp_histogram: Some(json.to_string()),
            attributes,
            ..Default::default()
        }
    }

    // Same shape as `epoint` but CUMULATIVE temporality, for the reset-aware-delta path.
    fn epoint_cumulative(svc: &str, ts: i64, json: &str) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        MetricPoint {
            metric_name: "elat".to_string(),
            metric_type: metric_type::EXP_HISTOGRAM,
            temporality: Some(TEMPORALITY_CUMULATIVE),
            timestamp_nanos: ts,
            exp_histogram: Some(json.to_string()),
            attributes,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn query_series_exp_histogram_p50() {
        let j = r#"{"count":10,"sum":0,"scale":0,"zero_count":1,"positive":{"offset":0,"bucket_counts":[3,4,2]}}"#;
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        b.append(&epoint("a", 10, j));
        let engine = MetricsQueryEngine::from_batch(schema, b.finish().unwrap());
        let res = engine
            .query_series(MetricSeriesRequest {
                metric: "elat".into(),
                agg: Some(Agg::P50),
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 100,
                buckets: 1,
            })
            .await
            .unwrap();
        // p50 of the Task-3 corpus = 2.5.
        assert!((res.series[0].points[0].v.unwrap() - 2.5).abs() < 1e-9);
    }

    // CUMULATIVE reset-aware delta on `count` (genuinely new logic, unlike the approximated
    // quantile payload): two consecutive same-bucket points, count 4 then 10. The first sample
    // contributes 0 (no `prev` to diff against); the second contributes 10-4=6. Pins the
    // reset-aware-delta + first-sample-zero behavior without asserting the approximate quantile.
    #[tokio::test]
    async fn query_series_exp_histogram_cumulative_count_deltas_consecutive() {
        let p1 = r#"{"count":4,"sum":0,"scale":0,"zero_count":0,"positive":{"offset":0,"bucket_counts":[1,2,1]}}"#;
        let p2 = r#"{"count":10,"sum":0,"scale":0,"zero_count":0,"positive":{"offset":0,"bucket_counts":[3,4,3]}}"#;
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        b.append(&epoint_cumulative("a", 10, p1));
        b.append(&epoint_cumulative("a", 20, p2));
        let engine = MetricsQueryEngine::from_batch(schema, b.finish().unwrap());
        let res = engine
            .query_series(MetricSeriesRequest {
                metric: "elat".into(),
                agg: Some(Agg::Count),
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 100,
                buckets: 1,
            })
            .await
            .unwrap();
        assert_eq!(res.series[0].points[0].v, Some(6.0));
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct QuantilePayload {
    pub quantile: f64,
    pub value: f64,
}

#[derive(serde::Deserialize)]
pub(crate) struct SummaryPayload {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub sum: f64,
    #[serde(default)]
    pub quantiles: Vec<QuantilePayload>,
}

/// The stored quantile nearest `target`. Summaries carry client-precomputed quantiles that cannot
/// be re-aggregated (a documented "full fidelity" limit) — we display, never interpolate across
/// series.
pub(crate) fn summary_pick(quantiles: &[QuantilePayload], target: f64) -> Option<f64> {
    quantiles
        .iter()
        .min_by(|a, b| {
            (a.quantile - target)
                .abs()
                .partial_cmp(&(b.quantile - target).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|q| q.value)
}

/// One-group summary roll-up: the latest sample per bucket (summaries are cumulative snapshots),
/// answering the requested quantile via `summary_pick`, or count/sum/avg from `count`/`sum`.
pub(crate) fn summary_series(
    rows: &[DistRow],
    agg: Agg,
    start: i64,
    end: i64,
    buckets: usize,
) -> Vec<SeriesPoint> {
    // rows are time-sorted ⇒ the last write per bucket is the freshest snapshot.
    let mut latest: Vec<Option<SummaryPayload>> = (0..buckets).map(|_| None).collect();
    for r in rows {
        if let Ok(p) = serde_json::from_str::<SummaryPayload>(&r.json) {
            latest[bucket_of(r.ts, start, end, buckets)] = Some(p);
        }
    }
    (0..buckets)
        .map(|i| {
            let v = latest[i].as_ref().and_then(|p| match agg {
                Agg::Count => Some(p.count as f64),
                Agg::Sum => Some(p.sum),
                Agg::Avg => (p.count > 0).then(|| p.sum / p.count as f64),
                _ => quantile_of(agg).and_then(|q| summary_pick(&p.quantiles, q)),
            });
            SeriesPoint {
                t: bucket_start(start, end, buckets, i),
                v,
            }
        })
        .collect()
}

impl MetricsQueryEngine {
    /// Summary series query: display precomputed quantiles (median/p50/p90/p99) + count/sum/avg.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn query_series_summary(
        &self,
        base: &MetricRequest,
        req: &MetricSeriesRequest,
        meta: ProbeMeta,
        default: Agg,
        chosen: Agg,
        step_nanos: i64,
        buckets: usize,
    ) -> Result<QuerySeriesResult, PhotonError> {
        let _ = meta;
        if !matches!(
            chosen,
            Agg::P50 | Agg::P90 | Agg::P99 | Agg::Median | Agg::Count | Agg::Sum | Agg::Avg
        ) {
            return Err(PhotonError::Query(format!(
                "aggregation `{}` is not supported for summary metrics (use median/p50/p90/p99/count/sum/avg)",
                chosen.as_str()
            )));
        }
        let (groups, capped) = self
            .collect_dist_series(base, &req.group_by, metric_schema::SUMMARY)
            .await?;
        let series = groups
            .into_iter()
            .map(|(labels, rows)| SeriesResult {
                labels,
                points: summary_series(
                    &rows,
                    chosen,
                    req.start_ts_nanos,
                    req.end_ts_nanos,
                    buckets,
                ),
            })
            .collect();
        Ok(QuerySeriesResult {
            series,
            default_agg: default,
            chosen_agg: chosen,
            step_nanos,
            capped,
        })
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    use crate::metric_query::MetricSeriesRequest;
    use crate::MetricsQueryEngine;
    use photon_core::metric_agg::Agg;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::metric_schema::{metric_type, MetricSchema};

    #[test]
    fn summary_pick_nearest() {
        let qs = vec![
            QuantilePayload {
                quantile: 0.5,
                value: 10.0,
            },
            QuantilePayload {
                quantile: 0.9,
                value: 40.0,
            },
            QuantilePayload {
                quantile: 0.99,
                value: 90.0,
            },
        ];
        assert_eq!(summary_pick(&qs, 0.5), Some(10.0));
        assert_eq!(summary_pick(&qs, 0.99), Some(90.0));
        assert_eq!(summary_pick(&[], 0.5), None);
    }

    fn spoint(svc: &str, ts: i64, json: &str) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        MetricPoint {
            metric_name: "slat".to_string(),
            metric_type: metric_type::SUMMARY,
            timestamp_nanos: ts,
            summary: Some(json.to_string()),
            attributes,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn query_series_summary_median_takes_latest_per_bucket() {
        let j = |v: f64| {
            format!(
                r#"{{"count":5,"sum":100.0,"quantiles":[{{"quantile":0.5,"value":{v}}},{{"quantile":0.99,"value":99.0}}]}}"#
            )
        };
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        // two points in the same bucket: latest (ts=20) wins.
        b.append(&spoint("a", 10, &j(10.0)));
        b.append(&spoint("a", 20, &j(15.0)));
        let engine = MetricsQueryEngine::from_batch(schema, b.finish().unwrap());
        let res = engine
            .query_series(MetricSeriesRequest {
                metric: "slat".into(),
                agg: None, // default for summary = median
                group_by: vec![],
                filter: None,
                start_ts_nanos: 0,
                end_ts_nanos: 100,
                buckets: 1,
            })
            .await
            .unwrap();
        assert_eq!(res.default_agg, Agg::Median);
        assert_eq!(res.series[0].points[0].v, Some(15.0));
    }
}

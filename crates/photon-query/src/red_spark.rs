//! `red_spark`: every row's sparkline for a Backend board, in ONE pass.
//!
//! The Services index and the Operations tab both draw two mini-charts per row (requests over
//! time, errors over time). They used to get them the obvious way — one [`crate::red_timeseries`]
//! call per row, fired concurrently and capped at 40 — which makes a page load `1 + N` full scans
//! of the same spans. On 86.6M spans that is the difference between the board being usable and not:
//! measured over 24h, the fleet RED aggregate is one scan and the index was doing eight.
//!
//! So this engine groups by **series and time bucket at once**, the way [`crate::red_rollup`]
//! already does for the background pass, and returns every row's series from a single scan.
//!
//! It is deliberately narrower than `red_timeseries`, because a sparkline is narrower: the board
//! plots `count` and `error_count` and nothing else. Dropping the rest is most of the saving —
//! there are no `approx_percentile_cont` t-digests (nine of them, per row, in the general engine)
//! and no HTTP status counters, which is what keeps the wide `attributes` map out of the plan
//! (see [`crate::span_engine::AttrKeys`]). The Apdex bands go too; the row's own Apdex comes from
//! the RED aggregate beside it, over the same window.
//!
//! ADR-0003's entry-set cascade still decides what a request is, and — like `red_timeseries` —
//! it is resolved ONCE per series rather than per bucket, so a quiet bucket holding only INTERNAL
//! spans cannot fall through to rule 3 and spike the chart against its neighbours.
use std::collections::HashMap;

use arrow::array::{Array, Int64Array, StringArray};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::sum;
use datafusion::prelude::{cast, lit, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::red::{entry_mask, root_mask, RedBasis, RedGroup, ENVIRONMENT};
use crate::span_engine::{attr_lookup, span_base_predicate, AttrKeys};
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// Identity of one sparkline series — the same tuple the matching RED row is keyed by, so the API
/// can line the two up without re-deriving either. `operation` is `Some` only for
/// [`RedGroup::Operation`]; `tenant`/`environment` only for [`RedGroup::Service`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct RedSeriesKey {
    pub service: String,
    pub tenant: Option<String>,
    pub environment: Option<String>,
    pub operation: Option<String>,
}

/// One point of a sparkline: bucket start (epoch nanos) plus the two counts the board plots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SparkPoint {
    pub ts: i64,
    pub count: u64,
    pub error_count: u64,
}

/// Cap on distinct series. Matches `crate::red`'s `MAX_RED_GROUPS`: the caller only draws the rows
/// that aggregate returned, so anything past the cap has no row to attach to anyway.
const MAX_SPARK_SERIES: usize = 1000;

impl SpanQueryEngine {
    /// Every series' sparkline over `[start, end]`, in one scan. Keyed exactly like the RED rows
    /// for the same `group`, so the caller looks up rather than re-queries.
    pub async fn red_spark_series(
        &self,
        req: SpanQueryRequest,
        start: i64,
        end: i64,
        buckets: usize,
        group: RedGroup,
    ) -> Result<HashMap<RedSeriesKey, Vec<SparkPoint>>, PhotonError> {
        // Mirrors `photon-api`'s `MAX_BUCKETS`; restated as a literal because `photon-query`
        // cannot depend on `photon-api`.
        let buckets = buckets.clamp(1, 3000);
        let promoted = self.promoted_attributes();
        let tenant_promoted = promoted.iter().any(|p| p == "tenant");
        let env_promoted = promoted.iter().any(|p| p == ENVIRONMENT);
        match self.span_survivors(&req).await? {
            None => Ok(HashMap::new()),
            Some((df, attr_keys)) => {
                spark_over(
                    df,
                    span_base_predicate(&req),
                    start,
                    end,
                    buckets,
                    group,
                    tenant_promoted,
                    env_promoted,
                    &attr_keys,
                )
                .await
            }
        }
    }
}

/// Width (nanos) of one of `buckets` equal-width buckets over `[start, end]`. Identical to
/// `red_timeseries::bucket_width` so a row's sparkline and its detail chart share a grid.
fn bucket_width(start: i64, end: i64, buckets: usize) -> i64 {
    ((end - start).max(1) / buckets as i64).max(1)
}

/// Per-candidate counters for one (series, bucket) cell.
#[derive(Default, Clone, Copy)]
struct Cell {
    n: u64,
    errors: u64,
}

/// The three cascade candidates for one (series, bucket) cell.
#[derive(Default, Clone, Copy)]
struct Cands {
    all: Cell,
    entry: Cell,
    root: Cell,
}

/// The two summed 0/1 flags for one candidate entry set, aliased with `suffix`.
fn count_aggs(mask: Option<&Expr>, suffix: &str) -> Result<Vec<Expr>, PhotonError> {
    let flag = |cond: Expr, what: &str| -> Result<Expr, PhotonError> {
        let in_set = match mask {
            Some(m) => m.clone().and(cond),
            None => cond,
        };
        when(in_set, lit(1_i64))
            .otherwise(lit(0_i64))
            .map_err(|e| PhotonError::Query(format!("red_spark {what} case: {e}")))
    };
    Ok(vec![
        sum(flag(lit(true), "count")?).alias(format!("n{suffix}")),
        sum(flag(
            col_ref(span_schema::STATUS_CODE).eq(lit(2_i32)),
            "error-flag",
        )?)
        .alias(format!("errors{suffix}")),
    ])
}

#[allow(clippy::too_many_arguments)]
async fn spark_over(
    df: DataFrame,
    predicate: Expr,
    start: i64,
    end: i64,
    buckets: usize,
    group: RedGroup,
    tenant_promoted: bool,
    env_promoted: bool,
    attr_keys: &AttrKeys,
) -> Result<HashMap<RedSeriesKey, Vec<SparkPoint>>, PhotonError> {
    let width = bucket_width(start, end, buckets);

    // bucket = (start_time_nanos - start) / width. A row landing exactly on `end` maps to
    // `buckets` and is clamped into the last bucket — same rule as `span_histogram`.
    let start_time = cast(col_ref(span_schema::START_TIME), DataType::Int64);
    let raw = (start_time - lit(start)) / lit(width);
    let bucket = when(
        raw.clone().gt_eq(lit(buckets as i64)),
        lit(buckets as i64 - 1),
    )
    .otherwise(raw)
    .map_err(|e| PhotonError::Query(format!("red_spark bucket case: {e}")))?;

    let mut group_exprs: Vec<Expr> = vec![col_ref("service.name").alias("service")];
    match group {
        RedGroup::Operation => {
            group_exprs.push(col_ref(span_schema::NAME).alias("operation"));
        }
        RedGroup::Service => {
            group_exprs.push(attr_lookup("tenant", tenant_promoted, attr_keys).alias("tenant"));
            group_exprs
                .push(attr_lookup(ENVIRONMENT, env_promoted, attr_keys).alias("environment"));
        }
    }
    group_exprs.push(bucket.alias("bucket"));

    let mut aggs = count_aggs(None, "")?;
    aggs.extend(count_aggs(Some(&entry_mask()), "_e")?);
    aggs.extend(count_aggs(Some(&root_mask()), "_r")?);

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("red_spark filter: {e}")))?
        .aggregate(group_exprs, aggs)
        .map_err(|e| PhotonError::Query(format!("red_spark aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("red_spark collect: {e}")))?;

    // (series, bucket) cells, accumulated in Rust. One entry per series; the vec is bucket-indexed
    // and zero-filled, because a quiet bucket is a real fact the chart must draw.
    let mut cells: HashMap<RedSeriesKey, Vec<Cands>> = HashMap::new();
    let mut capped = false;
    for b in &batches {
        let service = str_col(b, "service")?;
        let operation = match group {
            RedGroup::Operation => Some(str_col(b, "operation")?),
            RedGroup::Service => None,
        };
        let tenant = match group {
            RedGroup::Service => Some(str_col(b, "tenant")?),
            RedGroup::Operation => None,
        };
        let environment = match group {
            RedGroup::Service => Some(str_col(b, "environment")?),
            RedGroup::Operation => None,
        };
        let bucket_col = i64_col(b, "bucket")?;
        let all = CellCols::read(b, "")?;
        let entry = CellCols::read(b, "_e")?;
        let root = CellCols::read(b, "_r")?;

        for i in 0..b.num_rows() {
            if service.is_null(i) || bucket_col.is_null(i) {
                continue; // `service.name` and `start_time_nanos` are both required columns
            }
            let opt = |a: Option<&StringArray>| {
                a.and_then(|c| (!c.is_null(i)).then(|| c.value(i).to_string()))
            };
            let key = RedSeriesKey {
                service: service.value(i).to_string(),
                tenant: opt(tenant),
                environment: opt(environment),
                operation: opt(operation),
            };
            let idx = bucket_col.value(i).clamp(0, buckets as i64 - 1) as usize;
            if !cells.contains_key(&key) && cells.len() >= MAX_SPARK_SERIES {
                capped = true;
                continue;
            }
            let series = cells
                .entry(key)
                .or_insert_with(|| vec![Cands::default(); buckets]);
            series[idx].all.absorb(&all, i);
            series[idx].entry.absorb(&entry, i);
            series[idx].root.absorb(&root, i);
        }
    }
    if capped {
        eprintln!(
            "photon-query: red_spark hit the {MAX_SPARK_SERIES}-series cap; \
             rows past it render without a sparkline"
        );
    }

    Ok(cells
        .into_iter()
        .map(|(key, series)| {
            // Cascade resolved once per series — see the module header.
            let total =
                |pick: fn(&Cands) -> &Cell| -> u64 { series.iter().map(|c| pick(c).n).sum() };
            let basis = if total(|c| &c.entry) > 0 {
                RedBasis::Entry
            } else if total(|c| &c.root) > 0 {
                RedBasis::TraceRoot
            } else {
                RedBasis::AllSpans
            };
            let points = series
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let won = match basis {
                        RedBasis::Entry => &c.entry,
                        RedBasis::TraceRoot => &c.root,
                        RedBasis::AllSpans => &c.all,
                    };
                    SparkPoint {
                        ts: start + i as i64 * width,
                        count: won.n,
                        error_count: won.errors,
                    }
                })
                .collect();
            (key, points)
        })
        .collect())
}

/// The two count columns for one candidate suffix.
struct CellCols<'a> {
    n: &'a Int64Array,
    errors: &'a Int64Array,
}

impl<'a> CellCols<'a> {
    fn read(b: &'a RecordBatch, suffix: &str) -> Result<CellCols<'a>, PhotonError> {
        Ok(CellCols {
            n: i64_col(b, &format!("n{suffix}"))?,
            errors: i64_col(b, &format!("errors{suffix}"))?,
        })
    }
}

impl Cell {
    fn absorb(&mut self, cols: &CellCols<'_>, i: usize) {
        let get = |a: &Int64Array| {
            if a.is_null(i) {
                0
            } else {
                a.value(i).max(0) as u64
            }
        };
        self.n += get(cols.n);
        self.errors += get(cols.errors);
    }
}

fn i64_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| PhotonError::Query(format!("red_spark: missing i64 column `{name}`")))
}

fn str_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a StringArray, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| PhotonError::Query(format!("red_spark: missing string column `{name}`")))
}

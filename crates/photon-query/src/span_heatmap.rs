//! `latency_heatmap`: the duration distribution **over time** — one column per time bucket, one
//! row per latency band.
//!
//! This is the one shape three percentile lines genuinely cannot show. A p50/p90/p99 chart draws
//! three summaries of a distribution; when a distribution splits in two — half the requests served
//! from cache in 2ms, half falling through to a cold path at 800ms — all three lines sit in the
//! empty gap between the modes, describing a latency no request actually experienced. The heatmap
//! shows the two bands.
//!
//! **The latency ladder is fixed, not derived from the data.** `crate::red_rollup`'s
//! `LATENCY_BOUNDS_NS` is reused verbatim. Two reasons, and both matter more than the tighter fit
//! a data-derived ladder would give:
//!
//!   1. Columns must be comparable. Geometric bins sized from `min`/`max` of the matched set (what
//!      `crate::span_latency` does, correctly, for a single distribution) would re-scale the whole
//!      y-axis the moment one slow outlier entered the window, and a cell would mean something
//!      different at 10:00 than at 10:05.
//!   2. It is the same ladder the rollup stores, so a wide-window heatmap can later be answered
//!      from `photon.service.latency` without changing what a row means.
use arrow::array::{Array, Int64Array};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::prelude::{cast, lit, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::red_rollup::LATENCY_BOUNDS_NS;
use crate::span_engine::span_base_predicate;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// One cell: `count` spans in time bucket `t` and latency band `band`.
#[derive(Debug, Clone, PartialEq)]
pub struct HeatCell {
    /// Index into the returned `columns`.
    pub t: usize,
    /// Index into the returned `bounds`, `0..=bounds.len()`. The last index is the `+Inf` band.
    pub band: usize,
    pub count: u64,
}

/// A latency heatmap. `columns` are bucket START timestamps (epoch nanos), `bounds` the ladder's
/// upper edges (nanoseconds); band `i` holds durations in `(bounds[i-1], bounds[i]]`, and band
/// `bounds.len()` is everything above the last bound.
///
/// Cells are SPARSE — an empty cell is simply absent. A heatmap of 48 columns over 14 bands is 672
/// cells, most of them zero on any real service, and sending the zeros triples the payload to say
/// nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyHeatmap {
    pub columns: Vec<i64>,
    pub bounds: Vec<i64>,
    pub cells: Vec<HeatCell>,
    /// Spans with no `duration_nanos` — they never closed, so they belong in no band. Reported
    /// rather than dropped: an operation whose spans stop closing is exactly what a heatmap should
    /// not silently hide.
    pub open_spans: u64,
}

impl SpanQueryEngine {
    /// `buckets` equal-width time buckets over `[start, end]`, each holding the ladder's latency
    /// bands. `req` should already be scoped (a `service.name:… name:…` grammar query) — this
    /// method buckets, it does not group by service or operation.
    pub async fn latency_heatmap(
        &self,
        req: SpanQueryRequest,
        start: i64,
        end: i64,
        buckets: usize,
    ) -> Result<LatencyHeatmap, PhotonError> {
        // Defense-in-depth: mirrors `photon-api`'s `MAX_BUCKETS`; `photon-query` cannot depend on
        // `photon-api`, so the value is restated as a literal (same as every sibling engine).
        let buckets = buckets.clamp(1, 3000);
        let width = bucket_width(start, end, buckets);
        let columns: Vec<i64> = (0..buckets).map(|i| start + i as i64 * width).collect();
        match self.span_survivors_df(&req).await? {
            None => Ok(LatencyHeatmap {
                columns,
                bounds: LATENCY_BOUNDS_NS.to_vec(),
                cells: Vec::new(),
                open_spans: 0,
            }),
            Some(df) => {
                heatmap_over(df, span_base_predicate(&req), start, end, buckets, columns).await
            }
        }
    }
}

/// The width (nanos) of each of `buckets` equal-width buckets spanning `[start, end]`. Floored to
/// 1 so a degenerate window never divides by zero. Mirrors `red_timeseries::bucket_width` — the
/// two must agree or a heatmap and the latency chart above it would not line up.
fn bucket_width(start: i64, end: i64, buckets: usize) -> i64 {
    ((end - start).max(1) / buckets as i64).max(1)
}

/// `CASE` ladder mapping `duration_nanos` to a band index. Built as a chain rather than
/// arithmetic because the ladder is explicit bounds, not a formula — the same bounds the rollup's
/// histogram stores.
fn band_expr(dur: &Expr) -> Result<Expr, PhotonError> {
    let mut builder = when(dur.clone().lt_eq(lit(LATENCY_BOUNDS_NS[0])), lit(0_i64));
    for (i, bound) in LATENCY_BOUNDS_NS.iter().enumerate().skip(1) {
        builder = builder.when(dur.clone().lt_eq(lit(*bound)), lit(i as i64));
    }
    builder
        .otherwise(lit(LATENCY_BOUNDS_NS.len() as i64))
        .map_err(|e| PhotonError::Query(format!("heatmap band case: {e}")))
}

async fn heatmap_over(
    df: DataFrame,
    predicate: Expr,
    start: i64,
    end: i64,
    buckets: usize,
    columns: Vec<i64>,
) -> Result<LatencyHeatmap, PhotonError> {
    let width = bucket_width(start, end, buckets);
    let dur = col_ref(span_schema::DURATION);

    // bucket = (start_time_nanos - start) / width, integer division. A row at exactly `end` maps
    // to `buckets` and is clamped down — same treatment as `red_timeseries` and `span_histogram`.
    let start_time = cast(col_ref(span_schema::START_TIME), DataType::Int64);
    let raw = (start_time - lit(start)) / lit(width);
    let time_bucket = when(
        raw.clone().gt_eq(lit(buckets as i64)),
        lit(buckets as i64 - 1),
    )
    .otherwise(raw)
    .map_err(|e| PhotonError::Query(format!("heatmap bucket case: {e}")))?;

    let filtered = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("heatmap filter: {e}")))?;

    // A span that never closed has no duration and belongs in no band. It is counted separately
    // rather than folded into band 0 — "under 1ms" and "never finished" are opposite facts, and an
    // operation whose spans stop closing is precisely what this chart must not hide.
    let open_batches = filtered
        .clone()
        .filter(dur.clone().is_null())
        .map_err(|e| PhotonError::Query(format!("heatmap null filter: {e}")))?
        .aggregate(vec![], vec![count(lit(1_i64)).alias("n")])
        .map_err(|e| PhotonError::Query(format!("heatmap open aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("heatmap open collect: {e}")))?;
    let mut open_spans = 0u64;
    for b in &open_batches {
        let n = i64_col(b, "n")?;
        for i in 0..b.num_rows() {
            open_spans += nonneg_u64(n, i);
        }
    }

    // The GROUP BY filters nulls out explicitly: a NULL band expression would otherwise form its
    // own group and decode as a band index there is no row for.
    let batches = filtered
        .filter(dur.clone().is_not_null())
        .map_err(|e| PhotonError::Query(format!("heatmap not-null filter: {e}")))?
        .aggregate(
            vec![time_bucket.alias("bucket"), band_expr(&dur)?.alias("band")],
            vec![count(lit(1_i64)).alias("n")],
        )
        .map_err(|e| PhotonError::Query(format!("heatmap aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("heatmap collect: {e}")))?;

    let max_band = LATENCY_BOUNDS_NS.len() as i64;
    let mut cells = Vec::new();
    for b in &batches {
        let bucket_col = i64_col(b, "bucket")?;
        let band_col = i64_col(b, "band")?;
        let n = i64_col(b, "n")?;
        for i in 0..b.num_rows() {
            let count = nonneg_u64(n, i);
            if bucket_col.is_null(i) || band_col.is_null(i) || count == 0 {
                continue;
            }
            cells.push(HeatCell {
                t: bucket_col.value(i).clamp(0, buckets as i64 - 1) as usize,
                band: band_col.value(i).clamp(0, max_band) as usize,
                count,
            });
        }
    }
    // Sorted so the payload is stable across runs (DataFusion's group order is not guaranteed) and
    // a diff of two responses is readable.
    cells.sort_by_key(|c| (c.t, c.band));

    Ok(LatencyHeatmap {
        columns,
        bounds: LATENCY_BOUNDS_NS.to_vec(),
        cells,
        open_spans,
    })
}

fn i64_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| PhotonError::Query(format!("heatmap column `{name}` missing or not Int64")))
}

/// A non-null, non-negative count cell as `u64` (null / negative → 0).
fn nonneg_u64(col: &Int64Array, i: usize) -> u64 {
    if col.is_null(i) {
        0
    } else {
        col.value(i).max(0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;

    const MS: i64 = 1_000_000;

    fn span(start: i64, duration: Option<i64>) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), "api".to_string());
        SpanRecord {
            trace_id: "t1".into(),
            span_id: format!("s{start}-{}", duration.unwrap_or(-1)),
            name: Some("op".into()),
            start_time_nanos: start,
            duration_nanos: duration,
            attributes,
            ..Default::default()
        }
    }

    async fn df_of(records: &[SpanRecord]) -> DataFrame {
        let schema = SpanSchema::new(&["service.name".into()]);
        let mut b = SpanBatchBuilder::new(&schema);
        for r in records {
            b.append(r);
        }
        let ctx = SessionContext::new();
        ctx.register_table(
            "spans",
            Arc::new(
                MemTable::try_new(schema.arrow.clone(), vec![vec![b.finish().unwrap()]]).unwrap(),
            ),
        )
        .unwrap();
        ctx.table("spans").await.unwrap()
    }

    async fn run(records: &[SpanRecord], start: i64, end: i64, buckets: usize) -> LatencyHeatmap {
        let df = df_of(records).await;
        heatmap_over(
            df,
            datafusion::prelude::lit(true),
            start,
            end,
            buckets,
            (0..buckets)
                .map(|i| start + i as i64 * bucket_width(start, end, buckets))
                .collect(),
        )
        .await
        .unwrap()
    }

    fn total(h: &LatencyHeatmap) -> u64 {
        h.cells.iter().map(|c| c.count).sum()
    }

    #[tokio::test]
    async fn every_closed_span_lands_in_exactly_one_cell() {
        let records: Vec<SpanRecord> = (1..=50).map(|i| span(i, Some(i * MS))).collect();
        let h = run(&records, 0, 100, 10).await;
        assert_eq!(total(&h), 50, "cells must partition the matched set");
        assert_eq!(h.open_spans, 0);
    }

    #[tokio::test]
    async fn a_duration_lands_in_the_band_whose_upper_bound_first_covers_it() {
        // The ladder's edges are INCLUSIVE upper bounds: 1ms is band 0, 1ms+1ns is band 1 (<=2ms).
        let h = run(
            &[
                span(1, Some(MS)),
                span(2, Some(MS + 1)),
                span(3, Some(2 * MS)),
            ],
            0,
            100,
            1,
        )
        .await;
        let band_of = |n: usize| h.cells.iter().find(|c| c.band == n).map(|c| c.count);
        assert_eq!(band_of(0), Some(1), "exactly 1ms belongs to the <=1ms band");
        assert_eq!(band_of(1), Some(2), "1ms+1ns and 2ms both belong to <=2ms");
    }

    #[tokio::test]
    async fn anything_past_the_last_bound_lands_in_the_overflow_band() {
        let over = LATENCY_BOUNDS_NS[LATENCY_BOUNDS_NS.len() - 1] + 1;
        let h = run(&[span(1, Some(over))], 0, 100, 1).await;
        assert_eq!(h.cells.len(), 1);
        assert_eq!(
            h.cells[0].band,
            LATENCY_BOUNDS_NS.len(),
            "the +Inf band is index bounds.len(), one past the last edge"
        );
    }

    #[tokio::test]
    async fn an_open_span_is_reported_separately_not_folded_into_the_fastest_band() {
        // "Under 1ms" and "never finished" are opposite facts. Folding the second into the first
        // would make an operation whose spans stop closing look like it got faster.
        let h = run(&[span(1, None), span(2, Some(MS))], 0, 100, 1).await;
        assert_eq!(h.open_spans, 1);
        assert_eq!(total(&h), 1);
    }

    #[tokio::test]
    async fn a_span_at_exactly_the_window_end_is_clamped_into_the_last_column() {
        let h = run(&[span(100, Some(MS))], 0, 100, 4).await;
        assert_eq!(h.cells.len(), 1);
        assert_eq!(h.cells[0].t, 3);
    }

    #[tokio::test]
    async fn two_modes_stay_two_bands_instead_of_averaging_into_one() {
        // The whole reason this chart exists: a bimodal distribution that p50/p90/p99 would
        // describe with three lines sitting in the empty gap between the modes.
        let mut records: Vec<SpanRecord> = (0..20).map(|i| span(i, Some(2 * MS))).collect();
        records.extend((20..40).map(|i| span(i, Some(500 * MS))));
        let h = run(&records, 0, 100, 1).await;
        let bands: Vec<usize> = h.cells.iter().map(|c| c.band).collect();
        assert_eq!(bands.len(), 2, "two modes must remain two occupied bands");
        assert_eq!(total(&h), 40);
    }

    #[tokio::test]
    async fn empty_cells_are_absent_rather_than_zero_filled() {
        let h = run(&[span(1, Some(MS))], 0, 100, 48).await;
        assert_eq!(
            h.cells.len(),
            1,
            "47 columns of zeros are not worth sending"
        );
        assert_eq!(h.columns.len(), 48, "the x-axis is still complete");
    }

    #[tokio::test]
    async fn cells_come_back_in_a_stable_order() {
        let records: Vec<SpanRecord> = (0..40).map(|i| span(i * 2, Some((i + 1) * MS))).collect();
        let h = run(&records, 0, 100, 8).await;
        let mut sorted = h.cells.clone();
        sorted.sort_by_key(|c| (c.t, c.band));
        assert_eq!(
            h.cells, sorted,
            "DataFusion's group order is not guaranteed"
        );
    }
}

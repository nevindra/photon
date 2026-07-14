//! `search_spans`: prune (via `SpanQueryEngine::span_prune`) then read the surviving spans
//! Parquet with DataFusion, sorted per `SpanSort`. `count_matching_spans`: `COUNT(*)` over the
//! same pruned + predicated set, ignoring `limit`/`offset` — mirrors `photon-query/src/count.rs`
//! but for spans.
//!
//! ## Two-pass late materialization — all three `SpanSort`s
//!
//! Every sort reuses the timestamp-cutoff trick from the logs `QueryEngine::search` (`lib.rs`):
//! pass 1 projects only the sort key(s) (plus the tiebreaker columns, see below), filters, sorts,
//! and takes the top `offset + limit` rows to find the cutoff — the primary key value(s) of the
//! last row of the requested page. Pass 2 re-applies the predicate plus a `>= cutoff` filter on
//! the primary key (a much smaller candidate set than the full survivors) and re-runs
//! `sort + limit(offset, limit)` against it, so the wide `attributes` map is decoded only for
//! rows at or above the cutoff instead of every scanned row.
//!
//! - **`Recent`** (`search_recent`) — primary key `start_time_nanos` (never null).
//! - **`Slowest`** (`search_slowest`) — primary key `duration_nanos` (nullable: unmeasured spans
//!   sort last). Cutoff is `duration_nanos >= cutoff`.
//! - **`Errors`** (`search_errors`) — COMPOSITE primary key `(status_code DESC nulls-last,
//!   start_time DESC)`. The cutoff is a pair `(cs, cts)` and pass 2's filter must be
//!   `(status_code > cs) OR (status_code = cs AND start_time_nanos >= cts)` — a single-column
//!   `>=` would wrongly admit (or drop) spans in a different status tier.
//!
//! `duration_nanos` and `status_code` are both nullable, unlike `start_time_nanos`. When the
//! pass-1 window's *lowest* tier (by the primary sort key) contains a null, `duration_cutoff` /
//! `status_cutoff` return `NullTier` and pass 2 falls back to the **unfiltered** predicate —
//! mathematically equivalent to `key >= cutoff OR key IS NULL` (see those functions' docs for the
//! proof), just simpler to express and easier to prove correct than threading a secondary cutoff
//! through the null tier. This only forgoes the optimization on the rare page whose boundary
//! lands among null-valued spans; every other page still avoids materializing `attributes`.
//!
//! ## Deterministic tie order: the `(span_id, trace_id)` total-order tiebreaker
//!
//! All three sorts append `span_id ASC, trace_id ASC` as the FINAL key of the ORDER BY, in BOTH
//! passes, so pagination is fully deterministic even when the primary key (or composite key, for
//! `Errors`) is exactly tied across rows. `span_id` alone is only unique WITHIN a trace per the
//! OTLP spec — two spans in different traces may legitimately share a `span_id` — but the *pair*
//! `(span_id, trace_id)` is unique per row: a span's identity IS `(trace_id, span_id)`, and both
//! columns are required, non-nullable `Utf8` fields in the span schema (`span_schema::SPAN_ID` /
//! `TRACE_ID`), so lexicographic string comparison over the pair is a genuine total order with no
//! ties left to break.
//!
//! This is a pure ordering refinement layered on top of the existing two-pass structure, not a
//! change to it: the pass-1 cutoff extraction and the pass-2 filter predicate are UNCHANGED —
//! still keyed only on the primary/composite key — because the pass-2 filter (`key >= cutoff`, or
//! the `Errors` composite) already admits a superset that includes the *entire* tie group at the
//! cutoff, regardless of which specific tied rows pass 1's (still primary-key-sorted) window
//! happened to select. Pass 2's re-sort, now carrying the total-order tiebreaker, is what turns
//! that superset into one deterministic page — `limit(offset, limit)` against a total order always
//! selects the same page a single-pass query with the same total order would. Non-tied rows are
//! unaffected: the tiebreaker only orders rows whose primary/composite key is exactly equal. See
//! `slowest_sort_ties_at_cutoff_partition_correctly`, `errors_sort_composite_cutoff_straddles_status_tiers`,
//! and `recent_sort_ties_at_cutoff_partition_correctly` for the tests that pin down the exact
//! deterministic page contents this guarantees.

use arrow::array::{Array, Int32Array, Int64Array, TimestampNanosecondArray};
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::SortExpr;
use datafusion::prelude::{lit, lit_timestamp_nano, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest, SpanSort};

impl SpanQueryEngine {
    /// Prune → read → sort (per `req.sort`) → page. Returns an empty vec when no file survives
    /// pruning, or when nothing matches the predicate.
    pub async fn search_spans(
        &self,
        req: SpanQueryRequest,
    ) -> Result<Vec<RecordBatch>, PhotonError> {
        let df = match self.span_survivors_df(&req).await? {
            Some(df) => df,
            None => return Ok(Vec::new()),
        };
        let predicate = span_base_predicate(&req);
        match req.sort {
            SpanSort::Recent => search_recent(df, predicate, &req).await,
            SpanSort::Slowest => search_slowest(df, predicate, &req).await,
            SpanSort::Errors => search_errors(df, predicate, &req).await,
        }
    }

    /// Total spans matching `req` across the full (pruned) candidate set — not limited/paged.
    /// Mirrors `QueryEngine::count_matching` (`count.rs`) for spans.
    pub async fn count_matching_spans(&self, req: &SpanQueryRequest) -> Result<u64, PhotonError> {
        match self.span_survivors_df(req).await? {
            None => Ok(0),
            Some(df) => count_over(df, span_base_predicate(req)).await,
        }
    }

    /// `search_spans` plus the true `matched_count` over the full pruned set, from ONE
    /// prune/open instead of two. Mirrors `QueryEngine::search_with_count` (`lib.rs`) for spans:
    /// shares one `span_survivors_df` between the same per-sort search body (`search_recent` /
    /// `search_slowest` / `search_errors`, used by `search_spans` itself) and
    /// `count_matching_spans`'s `COUNT(*)` aggregate (`count_over`), instead of each
    /// independently re-pruning and re-opening the candidate set.
    pub async fn search_spans_with_count(
        &self,
        req: SpanQueryRequest,
    ) -> Result<(Vec<RecordBatch>, u64), PhotonError> {
        let df = match self.span_survivors_df(&req).await? {
            Some(df) => df,
            None => return Ok((Vec::new(), 0)),
        };
        let predicate = span_base_predicate(&req);
        let rows = match req.sort {
            SpanSort::Recent => search_recent(df.clone(), predicate.clone(), &req).await?,
            SpanSort::Slowest => search_slowest(df.clone(), predicate.clone(), &req).await?,
            SpanSort::Errors => search_errors(df.clone(), predicate.clone(), &req).await?,
        };
        let matched = count_over(df, predicate).await?;
        Ok((rows, matched))
    }
}

/// `SpanSort::Recent` — two-pass late materialization. See module docs.
async fn search_recent(
    df: DataFrame,
    predicate: Expr,
    req: &SpanQueryRequest,
) -> Result<Vec<RecordBatch>, PhotonError> {
    let ts_sort = || col_ref(span_schema::START_TIME).sort(false, false);
    let full_sort = || {
        let mut keys = vec![ts_sort()];
        keys.extend(tiebreak_sort());
        keys
    };
    let window = req.offset + req.limit;

    // Pass 1 — cheap cutoff probe. Project only `start_time_nanos` plus the `(span_id,
    // trace_id)` tiebreaker columns — still far short of the wide `attributes` map, which is
    // never decoded for rows we won't return.
    let cutoff_batches = df
        .clone()
        .filter(predicate.clone())
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .select({
            let mut cols = vec![col_ref(span_schema::START_TIME)];
            cols.extend(tiebreak_columns());
            cols
        })
        .map_err(|e| PhotonError::Query(format!("failed to project start_time: {e}")))?
        .sort(full_sort())
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(0, Some(window))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect cutoff: {e}")))?;

    // The cutoff is the smallest start_time among the newest `offset + limit` matches — the
    // last row of the requested page. Nothing matched (or the window was 0) → no rows. This
    // extraction is unchanged by the tiebreaker: it only reads column 0 (`start_time_nanos`),
    // and remains correct regardless of which specific tied rows landed in the pass-1 window
    // (see the module docs' "Deterministic tie order" section for the proof).
    let cutoff = match min_start_time(&cutoff_batches) {
        Some(c) => c,
        None => return Ok(Vec::new()),
    };

    // Pass 2 — full rows, but only from `[cutoff, end]`. Re-applying the predicate plus
    // `start_time_nanos >= cutoff`, then re-sorting (now by the total order: start_time DESC,
    // span_id ASC, trace_id ASC) and paging, yields the single-pass result deterministically —
    // ties at `cutoff` are broken the same way every time — while decoding the heavy columns
    // for only the rows at or above the cutoff.
    let predicate =
        predicate.and(col_ref(span_schema::START_TIME).gt_eq(lit_timestamp_nano(cutoff)));
    df.filter(predicate)
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .sort(full_sort())
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(req.offset, Some(req.limit))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect results: {e}")))
}

/// `SpanSort::Slowest` — two-pass late materialization keyed on `duration_nanos` (nullable;
/// unmeasured spans sort last). See module docs for the `NullTier` fallback.
async fn search_slowest(
    df: DataFrame,
    predicate: Expr,
    req: &SpanQueryRequest,
) -> Result<Vec<RecordBatch>, PhotonError> {
    let dur_sort = || col_ref(span_schema::DURATION).sort(false, false);
    let full_sort = || {
        let mut keys = vec![dur_sort()];
        keys.extend(tiebreak_sort());
        keys
    };
    let window = req.offset + req.limit;

    // Pass 1 — cheap cutoff probe. Project only `duration_nanos` plus the `(span_id, trace_id)`
    // tiebreaker columns.
    let cutoff_batches = df
        .clone()
        .filter(predicate.clone())
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .select({
            let mut cols = vec![col_ref(span_schema::DURATION)];
            cols.extend(tiebreak_columns());
            cols
        })
        .map_err(|e| PhotonError::Query(format!("failed to project duration_nanos: {e}")))?
        .sort(full_sort())
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(0, Some(window))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect cutoff: {e}")))?;

    // Cutoff extraction is unchanged by the tiebreaker: it only reads column 0
    // (`duration_nanos`), and remains correct regardless of which specific tied rows landed in
    // the pass-1 window (see the module docs).
    let pass2_predicate = match duration_cutoff(&cutoff_batches) {
        DurationCutoff::Empty => return Ok(Vec::new()),
        DurationCutoff::NullTier => predicate,
        DurationCutoff::Value(cutoff) => {
            predicate.and(col_ref(span_schema::DURATION).gt_eq(lit(cutoff)))
        }
    };

    // Pass 2 — full rows, re-filtered, re-sorted (by the total order: duration DESC, span_id
    // ASC, trace_id ASC), re-paged: every row at or above the cutoff is a candidate, and ties at
    // the cutoff are now broken deterministically by the tiebreaker before the limit trims the
    // page (see the module docs and `slowest_sort_ties_at_cutoff_partition_correctly`).
    df.filter(pass2_predicate)
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .sort(full_sort())
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(req.offset, Some(req.limit))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect results: {e}")))
}

/// `SpanSort::Errors` — two-pass late materialization keyed on the COMPOSITE
/// `(status_code DESC nulls-last, start_time DESC)`. See module docs for the composite-cutoff
/// predicate and the `NullTier` fallback.
async fn search_errors(
    df: DataFrame,
    predicate: Expr,
    req: &SpanQueryRequest,
) -> Result<Vec<RecordBatch>, PhotonError> {
    let composite_sort = || {
        vec![
            col_ref(span_schema::STATUS_CODE).sort(false, false),
            col_ref(span_schema::START_TIME).sort(false, false),
        ]
    };
    let full_sort = || {
        let mut keys = composite_sort();
        keys.extend(tiebreak_sort());
        keys
    };
    let window = req.offset + req.limit;

    // Pass 1 — cheap cutoff probe. Project the two composite sort-key columns plus the
    // `(span_id, trace_id)` tiebreaker columns.
    let cutoff_batches = df
        .clone()
        .filter(predicate.clone())
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .select({
            let mut cols = vec![
                col_ref(span_schema::STATUS_CODE),
                col_ref(span_schema::START_TIME),
            ];
            cols.extend(tiebreak_columns());
            cols
        })
        .map_err(|e| PhotonError::Query(format!("failed to project status_code/start_time: {e}")))?
        .sort(full_sort())
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(0, Some(window))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect cutoff: {e}")))?;

    // Cutoff extraction is unchanged by the tiebreaker: it only reads columns 0 and 1
    // (`status_code`, `start_time_nanos`), and remains correct regardless of which specific
    // tied rows landed in the pass-1 window (see the module docs).
    let pass2_predicate = match status_cutoff(&cutoff_batches) {
        StatusCutoff::Empty => return Ok(Vec::new()),
        StatusCutoff::NullTier => predicate,
        StatusCutoff::Value { status, start } => {
            // The crux predicate: a single-column `start_time >= cts` would wrongly admit spans
            // in a lower status tier whose start_time happens to be >= cts, or drop spans in a
            // higher status tier whose start_time happens to be < cts. Must gate on the tier
            // first, and only compare start_time *within* the boundary tier.
            let higher_tier = col_ref(span_schema::STATUS_CODE).gt(lit(status));
            let same_tier_at_or_after_cutoff = col_ref(span_schema::STATUS_CODE)
                .eq(lit(status))
                .and(col_ref(span_schema::START_TIME).gt_eq(lit_timestamp_nano(start)));
            predicate.and(higher_tier.or(same_tier_at_or_after_cutoff))
        }
    };

    // Pass 2 — full rows, re-filtered, re-sorted (by the total order: status_code DESC,
    // start_time DESC, span_id ASC, trace_id ASC), re-paged.
    df.filter(pass2_predicate)
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .sort(full_sort())
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(req.offset, Some(req.limit))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect results: {e}")))
}

/// The deterministic total-order tiebreaker appended as the FINAL key of every `SpanSort`'s
/// ORDER BY, in both passes. `span_id` is unique only WITHIN a trace per the OTLP spec, but the
/// pair `(span_id, trace_id)` is unique per row (a span's identity IS `(trace_id, span_id)`), and
/// both columns are required, non-nullable `Utf8` — so lexicographic comparison over the pair is
/// a genuine total order with no ties left to break. See the module docs' "Deterministic tie
/// order" section for the full argument.
fn tiebreak_sort() -> Vec<SortExpr> {
    vec![
        col_ref(span_schema::SPAN_ID).sort(true, false),
        col_ref(span_schema::TRACE_ID).sort(true, false),
    ]
}

/// The `(span_id, trace_id)` columns backing `tiebreak_sort`, for projecting them into a pass-1
/// cutoff probe so its `sort(full_sort())` has the columns it needs.
fn tiebreak_columns() -> Vec<Expr> {
    vec![
        col_ref(span_schema::SPAN_ID),
        col_ref(span_schema::TRACE_ID),
    ]
}

/// Apply `predicate`, then a global `COUNT(*)`; read back the single scalar. Mirrors
/// `count.rs::count_over` for spans.
async fn count_over(df: DataFrame, predicate: Expr) -> Result<u64, PhotonError> {
    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("count filter: {e}")))?
        .aggregate(vec![], vec![count(lit(1i64)).alias("n")])
        .map_err(|e| PhotonError::Query(format!("count aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("count collect: {e}")))?;
    let n = batches
        .first()
        .and_then(|b| b.column(0).as_any().downcast_ref::<Int64Array>())
        .filter(|c| !c.is_empty())
        .map(|c| c.value(0))
        .unwrap_or(0);
    Ok(n.max(0) as u64)
}

/// Smallest `start_time_nanos` across batches, or `None` when there are no rows. Reads back the
/// pass-1 cutoff (the last row's timestamp in the requested page). Only column 0
/// (`start_time_nanos`) is read — the pass-1 batches for `search_recent` carry trailing
/// `(span_id, trace_id)` tiebreaker columns too, but those are irrelevant to this extraction.
fn min_start_time(batches: &[RecordBatch]) -> Option<i64> {
    let mut min: Option<i64> = None;
    for batch in batches {
        let col = batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()?;
        for i in 0..col.len() {
            if !col.is_null(i) {
                let v = col.value(i);
                min = Some(min.map_or(v, |m| m.min(v)));
            }
        }
    }
    min
}

/// Classification of the pass-1 cutoff for a nullable single-key sort (`Slowest`).
enum DurationCutoff {
    /// No rows matched at all — the caller should return an empty result.
    Empty,
    /// The pass-1 window's lowest tier contains at least one null `duration_nanos`. Since
    /// `duration_nanos` has no secondary sort key to order the null tier by, the true cutoff
    /// predicate would be `duration_nanos >= min_non_null OR duration_nanos IS NULL` — but
    /// `min_non_null` here is provably the GLOBAL minimum non-null `duration_nanos` among all
    /// matches (nulls only start appearing in the window once every non-null match is already
    /// included, because they sort strictly after all non-null values), so that predicate
    /// admits every matching row, non-null and null alike. In other words it's equivalent to no
    /// filter at all. The caller should therefore fall back to the unfiltered predicate.
    NullTier,
    /// No null was found in the pass-1 window: `duration_nanos >= Value` is a valid, tighter
    /// cutoff filter for pass 2.
    Value(i64),
}

/// Reads the pass-1 `duration_nanos` cutoff batch for `search_slowest` and classifies it. Only
/// column 0 (`duration_nanos`, nullable) is read — trailing `(span_id, trace_id)` tiebreaker
/// columns are ignored. See `DurationCutoff` for what each variant means.
fn duration_cutoff(batches: &[RecordBatch]) -> DurationCutoff {
    let mut min: Option<i64> = None;
    let mut has_null = false;
    let mut saw_row = false;
    for batch in batches {
        let Some(col) = batch.column(0).as_any().downcast_ref::<Int64Array>() else {
            continue;
        };
        for i in 0..col.len() {
            saw_row = true;
            if col.is_null(i) {
                has_null = true;
            } else {
                let v = col.value(i);
                min = Some(min.map_or(v, |m| m.min(v)));
            }
        }
    }
    if !saw_row {
        DurationCutoff::Empty
    } else if has_null {
        DurationCutoff::NullTier
    } else {
        // Every row was non-null (saw_row is true), so `min` is guaranteed `Some`.
        DurationCutoff::Value(min.expect("saw_row && !has_null implies a numeric min"))
    }
}

/// Classification of the pass-1 cutoff for the composite `(status_code, start_time)` sort
/// (`Errors`). Mirrors `DurationCutoff` but carries a `(status, start)` pair for `Value`.
enum StatusCutoff {
    /// No rows matched at all — the caller should return an empty result.
    Empty,
    /// The pass-1 window's lowest status tier is `NULL` (unset). By the same reasoning as
    /// `DurationCutoff::NullTier`, every non-null-status match is already fully captured, and
    /// falling back to the unfiltered predicate is a correct (if less optimized) superset for
    /// pass 2. Unlike `Slowest`, `Errors` DOES have a secondary key (`start_time`) inside the
    /// null tier that a tighter cutoff could exploit, but the unfiltered fallback is simpler to
    /// prove correct and only costs the optimization on the rare page whose boundary lands among
    /// unset-status spans.
    NullTier,
    /// No null `status_code` was found in the pass-1 window: `status` is the boundary tier and
    /// `start` is the smallest `start_time_nanos` within that tier — the composite cutoff
    /// `(status_code > status) OR (status_code = status AND start_time_nanos >= start)` is a
    /// valid, tighter filter for pass 2.
    Value { status: i32, start: i64 },
}

/// Reads the pass-1 `(status_code, start_time_nanos)` cutoff batch for `search_errors` and
/// classifies it. See `StatusCutoff` for what each variant means. Column 0 must be
/// `status_code` (nullable `Int32`), column 1 `start_time_nanos` (non-nullable timestamp) — the
/// same projection order `search_errors` selects; trailing `(span_id, trace_id)` tiebreaker
/// columns are ignored.
fn status_cutoff(batches: &[RecordBatch]) -> StatusCutoff {
    let mut min_status: Option<i32> = None;
    let mut has_null = false;
    let mut saw_row = false;
    for batch in batches {
        let Some(status_col) = batch.column(0).as_any().downcast_ref::<Int32Array>() else {
            continue;
        };
        for i in 0..status_col.len() {
            saw_row = true;
            if status_col.is_null(i) {
                has_null = true;
            } else {
                let v = status_col.value(i);
                min_status = Some(min_status.map_or(v, |m| m.min(v)));
            }
        }
    }
    if !saw_row {
        return StatusCutoff::Empty;
    }
    if has_null {
        return StatusCutoff::NullTier;
    }
    // Every row was non-null, so `min_status` is guaranteed `Some` here.
    let status = min_status.expect("saw_row && !has_null implies a numeric min_status");

    // Second scan: the smallest `start_time_nanos` among rows in the boundary status tier.
    let mut start: Option<i64> = None;
    for batch in batches {
        let Some(status_col) = batch.column(0).as_any().downcast_ref::<Int32Array>() else {
            continue;
        };
        let Some(start_col) = batch
            .column(1)
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
        else {
            continue;
        };
        for i in 0..status_col.len() {
            if !status_col.is_null(i) && status_col.value(i) == status && !start_col.is_null(i) {
                let v = start_col.value(i);
                start = Some(start.map_or(v, |m| m.min(v)));
            }
        }
    }
    StatusCutoff::Value {
        status,
        start: start.expect("boundary status tier has at least one row with a start_time"),
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

    fn schema() -> SpanSchema {
        SpanSchema::new(&["service.name".into()])
    }

    #[allow(clippy::too_many_arguments)]
    fn span(
        span_id: &str,
        start: i64,
        duration_nanos: Option<i64>,
        status_code: Option<i32>,
    ) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), "checkout".to_string());
        SpanRecord {
            trace_id: "t1".into(),
            span_id: span_id.into(),
            name: Some("op".into()),
            start_time_nanos: start,
            duration_nanos,
            status_code,
            attributes,
            ..Default::default()
        }
    }

    /// Like `span`, but overrides `trace_id`. Every other fixture in this module leaves
    /// `trace_id` at `span`'s hardcoded `"t1"`, so `trace_id`'s role as the tiebreaker's
    /// TERTIARY discriminator (after the primary/composite sort key, then `span_id`) is never
    /// exercised by them — see `slowest_sort_tiebreak_falls_through_to_trace_id` below, which
    /// uses this to construct spans that share the SAME `span_id` across DIFFERENT `trace_id`s.
    fn span_with_trace(
        trace_id: &str,
        span_id: &str,
        start: i64,
        duration_nanos: Option<i64>,
        status_code: Option<i32>,
    ) -> SpanRecord {
        SpanRecord {
            trace_id: trace_id.into(),
            ..span(span_id, start, duration_nanos, status_code)
        }
    }

    /// `trace_id` for each row, in order — for asserting tie order among spans that share a
    /// `span_id` (so `span_ids` alone can't distinguish them).
    fn trace_ids(batches: &[RecordBatch]) -> Vec<String> {
        use arrow::array::StringArray;
        let mut ids = Vec::new();
        for batch in batches {
            let col = batch
                .column_by_name("trace_id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for i in 0..col.len() {
                ids.push(col.value(i).to_string());
            }
        }
        ids
    }

    async fn df_of(records: &[SpanRecord]) -> DataFrame {
        let schema = schema();
        let mut b = SpanBatchBuilder::new(&schema);
        for r in records {
            b.append(r);
        }
        let batch: RecordBatch = b.finish().unwrap();
        let ctx = SessionContext::new();
        ctx.register_table(
            "spans",
            Arc::new(MemTable::try_new(schema.arrow.clone(), vec![vec![batch]]).unwrap()),
        )
        .unwrap();
        ctx.table("spans").await.unwrap()
    }

    fn req(sort: SpanSort, limit: usize, offset: usize) -> SpanQueryRequest {
        SpanQueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: i64::MAX,
            query: None,
            sort,
            limit,
            offset,
            projected_attributes: Vec::new(),
        }
    }

    fn span_ids(batches: &[RecordBatch]) -> Vec<String> {
        use arrow::array::StringArray;
        let mut ids = Vec::new();
        for batch in batches {
            let col = batch
                .column_by_name("span_id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for i in 0..col.len() {
                ids.push(col.value(i).to_string());
            }
        }
        ids
    }

    #[tokio::test]
    async fn recent_sort_returns_newest_first() {
        let records = vec![
            span("s1", 100, Some(10), Some(1)),
            span("s2", 300, Some(10), Some(1)),
            span("s3", 200, Some(10), Some(1)),
        ];
        let df = df_of(&records).await;
        let r = req(SpanSort::Recent, 10, 0);
        let predicate = span_base_predicate(&r);
        let batches = search_recent(df, predicate, &r).await.unwrap();
        assert_eq!(
            span_ids(&batches),
            vec!["s2".to_string(), "s3".to_string(), "s1".to_string()]
        );
    }

    #[tokio::test]
    async fn recent_sort_pages_with_limit_and_offset() {
        let records = vec![
            span("s1", 100, Some(10), Some(1)),
            span("s2", 300, Some(10), Some(1)),
            span("s3", 200, Some(10), Some(1)),
            span("s4", 400, Some(10), Some(1)),
        ];
        let df = df_of(&records).await;
        // Newest-first order is s4(400), s2(300), s3(200), s1(100). Page 1 (limit=2, offset=0)
        // is [s4, s2]; page 2 (limit=2, offset=2) is [s3, s1].
        let r1 = req(SpanSort::Recent, 2, 0);
        let batches1 = search_recent(df.clone(), span_base_predicate(&r1), &r1)
            .await
            .unwrap();
        assert_eq!(
            span_ids(&batches1),
            vec!["s4".to_string(), "s2".to_string()]
        );

        let r2 = req(SpanSort::Recent, 2, 2);
        let batches2 = search_recent(df, span_base_predicate(&r2), &r2)
            .await
            .unwrap();
        assert_eq!(
            span_ids(&batches2),
            vec!["s3".to_string(), "s1".to_string()]
        );
    }

    #[tokio::test]
    async fn recent_sort_empty_when_nothing_matches() {
        let df = df_of(&[]).await;
        let r = req(SpanSort::Recent, 10, 0);
        let batches = search_recent(df, span_base_predicate(&r), &r)
            .await
            .unwrap();
        assert!(batches.is_empty());
    }

    /// `SpanSort::Recent` also carries the `(span_id, trace_id)` total-order tiebreaker (this is
    /// the behavior change accepted for this fix — Recent's shipped tie output is no longer
    /// plan-dependent). Several spans share the SAME `start_time_nanos`, and a page boundary
    /// lands squarely inside that tied group; the tiebreaker orders the trio by `span_id ASC`
    /// (`"s1" < "s2" < "s3"`), giving the total order
    /// `[s5(300), s1(200), s2(200), s3(200), s4(100)]`. Asserts the EXACT contents of every page
    /// — this would fail if the tiebreaker were missing from either pass.
    #[tokio::test]
    async fn recent_sort_ties_at_cutoff_partition_correctly() {
        let records = vec![
            span("s1", 200, Some(10), Some(1)),
            span("s2", 200, Some(10), Some(1)),
            span("s3", 200, Some(10), Some(1)),
            span("s4", 100, Some(10), Some(1)),
            span("s5", 300, Some(10), Some(1)),
        ];
        let df = df_of(&records).await;

        let pages: Vec<(usize, usize, Vec<&str>)> = vec![
            (0, 2, vec!["s5", "s1"]),
            (2, 2, vec!["s2", "s3"]),
            (4, 2, vec!["s4"]),
        ];
        let mut seen = Vec::new();
        for (offset, limit, expected) in pages {
            let r = req(SpanSort::Recent, limit, offset);
            let batches = search_recent(df.clone(), span_base_predicate(&r), &r)
                .await
                .unwrap();
            assert_eq!(
                span_ids(&batches),
                expected.into_iter().map(String::from).collect::<Vec<_>>(),
                "offset={offset} limit={limit}"
            );
            seen.extend(span_ids(&batches));
        }

        assert_eq!(
            seen,
            vec![
                "s5".to_string(),
                "s1".to_string(),
                "s2".to_string(),
                "s3".to_string(),
                "s4".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn slowest_sort_returns_longest_first() {
        let records = vec![
            span("s1", 100, Some(50), Some(1)),
            span("s2", 200, Some(900), Some(1)),
            span("s3", 300, Some(200), Some(1)),
            span("s4", 400, None, Some(1)),
        ];
        let df = df_of(&records).await;
        let r = req(SpanSort::Slowest, 10, 0);
        let batches = search_slowest(df, span_base_predicate(&r), &r)
            .await
            .unwrap();
        // Longest first; the null-duration span sorts last.
        assert_eq!(
            span_ids(&batches),
            vec![
                "s2".to_string(),
                "s3".to_string(),
                "s1".to_string(),
                "s4".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn slowest_sort_pages_with_limit_and_offset() {
        let records = vec![
            span("s1", 100, Some(50), Some(1)),
            span("s2", 200, Some(900), Some(1)),
            span("s3", 300, Some(200), Some(1)),
        ];
        let df = df_of(&records).await;
        let r = req(SpanSort::Slowest, 1, 1);
        let batches = search_slowest(df, span_base_predicate(&r), &r)
            .await
            .unwrap();
        // Order by duration DESC is s2(900), s3(200), s1(50); offset=1, limit=1 -> s3.
        assert_eq!(span_ids(&batches), vec!["s3".to_string()]);
    }

    /// Several spans share the SAME `duration_nanos`, and successive page boundaries land inside
    /// that tied group. With the `(span_id, trace_id)` total-order tiebreaker, tie placement is
    /// now fully DETERMINISTIC: the tied trio `s1`/`s2`/`s3` (all `duration_nanos = 100`, all
    /// `trace_id = "t1"`) is ordered by `span_id ASC` — `"s1" < "s2" < "s3"` — giving the total
    /// order `[s5(200), s1(100), s2(100), s3(100), s4(50)]`. This test sweeps every page boundary
    /// with `limit=2` (so the second page starts squarely inside the tied group) and asserts the
    /// EXACT contents of every page — this would fail if the tiebreaker were missing from either
    /// pass, or if the passes ever disagreed on the tie order.
    #[tokio::test]
    async fn slowest_sort_ties_at_cutoff_partition_correctly() {
        let records = vec![
            span("s1", 100, Some(100), Some(1)),
            span("s2", 200, Some(100), Some(1)),
            span("s3", 300, Some(100), Some(1)),
            span("s4", 400, Some(50), Some(1)),
            span("s5", 500, Some(200), Some(1)),
        ];
        // Total order (duration DESC, span_id ASC, trace_id ASC):
        // s5(200), s1(100), s2(100), s3(100), s4(50).
        let df = df_of(&records).await;

        let pages: Vec<(usize, usize, Vec<&str>)> = vec![
            (0, 2, vec!["s5", "s1"]),
            (2, 2, vec!["s2", "s3"]),
            (4, 2, vec!["s4"]),
        ];
        let mut seen = Vec::new();
        for (offset, limit, expected) in pages {
            let r = req(SpanSort::Slowest, limit, offset);
            let batches = search_slowest(df.clone(), span_base_predicate(&r), &r)
                .await
                .unwrap();
            assert_eq!(
                span_ids(&batches),
                expected.into_iter().map(String::from).collect::<Vec<_>>(),
                "offset={offset} limit={limit}"
            );
            seen.extend(span_ids(&batches));
        }

        // No drops, no duplicates across the full sweep: exactly the 5 matching spans, each
        // exactly once, in the total order.
        assert_eq!(
            seen,
            vec![
                "s5".to_string(),
                "s1".to_string(),
                "s2".to_string(),
                "s3".to_string(),
                "s4".to_string()
            ]
        );
    }

    /// Coverage gap in the tests above: EVERY fixture there hardcodes `trace_id: "t1"` and uses
    /// distinct `span_id`s, so `span_id` alone always resolves the tie — `trace_id`'s role as the
    /// TERTIARY discriminator in `tiebreak_sort`/`tiebreak_columns` is never exercised. This test
    /// closes that gap: three spans share the SAME `span_id` ("dup") across DIFFERENT `trace_id`s
    /// AND are tied on the primary sort key (`duration_nanos = 100`), so `span_id ASC` is a no-op
    /// tiebreaker between them and only `trace_id ASC` can order them. A regression that dropped
    /// `trace_id` from the tiebreaker, reversed its direction, or reordered it ahead of `span_id`
    /// would pass every other test in this module but fail this one.
    ///
    /// Total order (duration DESC, span_id ASC [ties: "dup"="dup"="dup"], trace_id ASC):
    /// hi(500,t9), dup(100,tA), dup(100,tB), dup(100,tC), lo(10,t0).
    /// The sweep's page boundary at offset=2 lands squarely inside the tied trio (splitting
    /// dup@tA from dup@tB/tC), so both pass 1's cutoff window AND pass 2's re-sort must honor the
    /// `trace_id` order for the page contents below to be exactly right.
    #[tokio::test]
    async fn slowest_sort_tiebreak_falls_through_to_trace_id() {
        let records = vec![
            span_with_trace("t9", "hi", 1000, Some(500), Some(1)),
            span_with_trace("tC", "dup", 300, Some(100), Some(1)),
            span_with_trace("tA", "dup", 100, Some(100), Some(1)),
            span_with_trace("tB", "dup", 200, Some(100), Some(1)),
            span_with_trace("t0", "lo", 50, Some(10), Some(1)),
        ];
        let df = df_of(&records).await;

        let pages: Vec<(usize, usize, Vec<&str>)> = vec![
            (0, 2, vec!["t9", "tA"]),
            (2, 2, vec!["tB", "tC"]),
            (4, 2, vec!["t0"]),
        ];
        let mut seen = Vec::new();
        for (offset, limit, expected) in pages {
            let r = req(SpanSort::Slowest, limit, offset);
            let batches = search_slowest(df.clone(), span_base_predicate(&r), &r)
                .await
                .unwrap();
            assert_eq!(
                trace_ids(&batches),
                expected.into_iter().map(String::from).collect::<Vec<_>>(),
                "offset={offset} limit={limit}"
            );
            seen.extend(trace_ids(&batches));
        }

        // No drops, no duplicates across the full sweep: exactly the 5 matching spans, each
        // exactly once, in the total order — ordered by trace_id ASC within the span_id tie.
        assert_eq!(
            seen,
            vec![
                "t9".to_string(),
                "tA".to_string(),
                "tB".to_string(),
                "tC".to_string(),
                "t0".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn errors_sort_puts_error_spans_first() {
        let records = vec![
            span("s1", 100, Some(10), Some(1)), // ok
            span("s2", 200, Some(10), Some(2)), // error
            span("s3", 300, Some(10), None),    // unset
            span("s4", 50, Some(10), Some(2)),  // error, older than s2
        ];
        let df = df_of(&records).await;
        let r = req(SpanSort::Errors, 10, 0);
        let batches = search_errors(df, span_base_predicate(&r), &r)
            .await
            .unwrap();
        // status_code DESC (2s first, newest-first among ties), then 1, then null last.
        assert_eq!(
            span_ids(&batches),
            vec![
                "s2".to_string(),
                "s4".to_string(),
                "s1".to_string(),
                "s3".to_string()
            ]
        );
    }

    /// The composite `(status_code, start_time)` cutoff, exercised at TWO page boundaries in one
    /// sweep: one that straddles two DIFFERENT status tiers (crux of the `Errors` two-pass
    /// rewrite: a naive single-column `start_time >= cts` would get this wrong), and one that
    /// lands squarely inside a group of rows EXACTLY tied on the composite key (crux of this
    /// fix: the `(span_id, trace_id)` tiebreaker must be applied identically in both passes for
    /// pagination to be deterministic). Asserts the EXACT contents of every page.
    #[tokio::test]
    async fn errors_sort_composite_cutoff_straddles_status_tiers() {
        let records = vec![
            span("s1", 100, Some(10), Some(2)), // error tier, middle start_time
            span("s2", 200, Some(10), Some(2)), // error tier, newest
            span("s3", 150, Some(10), Some(2)), // error tier, tied with s6/s7
            span("s6", 150, Some(10), Some(2)), // error tier, tied with s3/s7
            span("s7", 150, Some(10), Some(2)), // error tier, tied with s3/s6
            span("s4", 500, Some(10), Some(1)), // ok tier, but started AFTER every error span
            span("s5", 50, Some(10), Some(1)),  // ok tier, oldest overall
        ];
        let df = df_of(&records).await;
        // Total order (status_code DESC, start_time DESC, span_id ASC, trace_id ASC):
        //   s2(2,200), then the tied trio at (2,150) ordered by span_id ASC — s3, s6, s7 — then
        //   s1(2,100), then s4(1,500), then s5(1,50).
        // This exercises BOTH crux cases in one sweep:
        //  - a page boundary landing INSIDE the tied (2,150) trio (offset=2, splitting s3 from
        //    s6/s7) — proving the `(span_id, trace_id)` tiebreaker deterministically partitions
        //    exactly-tied composite keys the same way in both passes;
        //  - a page boundary straddling two DIFFERENT status tiers (offset=4, landing on
        //    s1(2,100)/s4(1,500)) — proving the composite predicate `(status_code > cs) OR
        //    (status_code = cs AND start_time >= cts)` still gates on the tier first, not a
        //    naive single-column `start_time >= cts` (which would wrongly include s5 or exclude
        //    s4).
        let total_order = [
            "s2".to_string(),
            "s3".to_string(),
            "s6".to_string(),
            "s7".to_string(),
            "s1".to_string(),
            "s4".to_string(),
            "s5".to_string(),
        ];

        let pages: Vec<(usize, usize, Vec<&str>)> = vec![
            (0, 2, vec!["s2", "s3"]),
            (2, 2, vec!["s6", "s7"]),
            (4, 2, vec!["s1", "s4"]),
            (6, 2, vec!["s5"]),
        ];
        let mut seen = Vec::new();
        for (offset, limit, expected) in pages {
            let r = req(SpanSort::Errors, limit, offset);
            let predicate = span_base_predicate(&r);

            // Reference: a single-pass sort over the FULL total order (composite key +
            // tiebreaker), independent of the two-pass cutoff machinery.
            let reference = df
                .clone()
                .filter(predicate.clone())
                .unwrap()
                .sort(vec![
                    col_ref(span_schema::STATUS_CODE).sort(false, false),
                    col_ref(span_schema::START_TIME).sort(false, false),
                    col_ref(span_schema::SPAN_ID).sort(true, false),
                    col_ref(span_schema::TRACE_ID).sort(true, false),
                ])
                .unwrap()
                .limit(offset, Some(limit))
                .unwrap()
                .collect()
                .await
                .unwrap();

            let two_pass = search_errors(df.clone(), predicate, &r).await.unwrap();
            let expected: Vec<String> = expected.into_iter().map(String::from).collect();
            assert_eq!(
                span_ids(&two_pass),
                expected,
                "offset={offset} limit={limit}"
            );
            assert_eq!(
                span_ids(&two_pass),
                span_ids(&reference),
                "two-pass must match a reference single-pass total-order sort at offset={offset} limit={limit}"
            );
            seen.extend(span_ids(&two_pass));
        }

        // No drops, no duplicates across the full sweep: exactly the 7 matching spans, each
        // exactly once, in the total order.
        assert_eq!(seen, total_order);
    }

    #[tokio::test]
    async fn count_matching_spans_ignores_limit() {
        let records = vec![
            span("s1", 100, Some(10), Some(1)),
            span("s2", 200, Some(10), Some(1)),
            span("s3", 300, Some(10), Some(1)),
        ];
        let df = df_of(&records).await;
        let r = req(SpanSort::Recent, 1, 0); // limit=1, deliberately tiny
        let n = count_over(df, span_base_predicate(&r)).await.unwrap();
        assert_eq!(n, 3);
    }

    #[tokio::test]
    async fn count_matching_spans_is_zero_when_nothing_matches() {
        let df = df_of(&[]).await;
        let r = req(SpanSort::Recent, 10, 0);
        let n = count_over(df, span_base_predicate(&r)).await.unwrap();
        assert_eq!(n, 0);
    }
}

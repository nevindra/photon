//! `search_traces`: the trace-list view. For a time window (plus an optional grammar query, a
//! sort mode, and paging), find the traces whose spans match, then roll each trace up in Rust to
//! a [`TraceSummary`] (span/error counts, distinct services, a representative root span).
//!
//! ## v1 algorithm — breadth-first, correctness over speed
//!
//! Precomputed per-trace rollups are deferred (design brief); v1 recomputes them per query:
//! 1. Prune + open the surviving spans Parquet (`span_survivors_df`); `None` → empty result.
//! 2. **Matched trace ids (ranked in DataFusion, bounded memory):** apply the grammar predicate,
//!    then from that one filtered handle run two bounded queries — `COUNT(DISTINCT trace_id)` for
//!    `matched_count` (the full distinct total, independent of the cap/paging), and
//!    `GROUP BY trace_id → min(start_time)` sorted `min_start DESC` (id-tiebroken) `LIMIT`
//!    [`MAX_CANDIDATE_TRACES`] for the ranked page. Only the cap-many ranked rows ever reach Rust,
//!    instead of collecting every distinct id into a `Vec` (a documented v1 limitation:
//!    sort/slowest accuracy is bounded by the cap; a capped scan is logged, never silent).
//! 3. **Whole-trace spans:** fetch every span of the capped trace ids (`trace_id IN (...)`, no
//!    grammar filter) so the rollups reflect the entire trace, not only the matching spans — over a
//!    window narrowed to the kept traces' earliest-matching-span span ± [`STEP3_WINDOW_PAD_NANOS`],
//!    so min/max pruning drops files outside that range (the old code rescanned the ENTIRE original
//!    window for one page) while the ±1h pad still admits every whole-trace span, including
//!    window-straddling ones the old un-padded step 3 undercounted (audit F10).
//! 4. **Rust rollup** per trace: `span_count`, `error_count` (`status_code == 2`), distinct sorted
//!    `services`, and a representative span (the parent-less root if present, else the earliest by
//!    start) supplying `root_service` / `root_name` / `start_ts_nanos` / `duration_nanos` (falling
//!    back to `max(end_time) - min(start_time)` when the representative carries no duration).
//! 5. **Sort** the summaries per `req.sort` and page with `offset` / `limit`.
//!
//! Only bound literals reach DataFusion (the `trace_id IN (...)` list is built from `lit`), so
//! there is no SQL-injection surface.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use arrow::array::{
    Array, Int32Array, Int64Array, MapArray, StringArray, TimestampNanosecondArray,
};
use arrow::record_batch::RecordBatch;
use datafusion::functions_aggregate::expr_fn::{count_distinct, min};
use datafusion::prelude::{col, lit, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest, SpanSort};

/// The rollup scan is capped at this many traces. `matched_count` still reports the full distinct
/// count, but only the newest `MAX_CANDIDATE_TRACES` are rolled up / sorted / paged. A documented
/// v1 limitation (sort/slowest accuracy is bounded by the cap); the future optimization is
/// precomputed per-trace rollups so the whole match set can be ranked cheaply.
const MAX_CANDIDATE_TRACES: usize = 2000;

/// ±window added around the kept traces' earliest-matching-span span when narrowing step 3's
/// whole-trace scan (see `search_traces`). Mirrors `span_engine::TRACE_TIME_HINT_PADDING_NANOS`
/// (±1h) — defined locally so this fix stays confined to `trace_list.rs`.
///
/// **Pad-safety (why narrowing cannot drop a real span — the conservative-pruning invariant):**
/// step 2 ranks each kept trace by its earliest *matching* span start; the narrowed window is
/// `[min(kept starts) − PAD, max(kept starts) + PAD]`. For a kept trace `T` with earliest-matching
/// start `m` (so `min(kept starts) ≤ m ≤ max(kept starts)`), every span of `T` starts within `T`'s
/// wall-clock extent, i.e. in `[m − dur(T), m + dur(T)]`. As long as `dur(T) ≤ PAD`, that whole
/// range sits inside `[m − PAD, m + PAD] ⊆ [min(kept starts) − PAD, max(kept starts) + PAD]`, so
/// the file holding any of `T`'s spans overlaps the narrowed window and survives min/max pruning —
/// nothing real is dropped. A trace longer than 1h is the *same* documented v1 edge that
/// `get_trace`'s ±1h hint already accepts; a real span never starts more than a trace's own
/// duration before its earliest matching span, so 1h is comfortably conservative for typical
/// request/RED traces. (If a workload were known to routinely span >1h, widen this or keep the
/// original bound — do NOT under-pad.)
const STEP3_WINDOW_PAD_NANOS: i64 = 3_600_000_000_000; // ±1h; mirrors span_engine::TRACE_TIME_HINT_PADDING_NANOS

/// One trace's rollup: identity, a representative root span's fields, and per-trace aggregates.
pub struct TraceSummary {
    /// The trace id.
    pub trace_id: String,
    /// The representative span's `service.name` (the parent-less root if present, else the
    /// earliest-start span), if it carries one.
    pub root_service: Option<String>,
    /// The representative span's `name`, if it carries one.
    pub root_name: Option<String>,
    /// The representative span's `start_time_nanos` — the trace's start.
    pub start_ts_nanos: i64,
    /// The representative span's `duration_nanos`, falling back to `max(end_time) -
    /// min(start_time)` across the trace when the representative has no duration. `None` when
    /// neither is available.
    pub duration_nanos: Option<i64>,
    /// Total spans in the trace (of the fetched, capped set).
    pub span_count: u64,
    /// Spans with `status_code == 2` (OTEL `ERROR`).
    pub error_count: u64,
    /// Distinct `service.name` values across the trace, sorted ascending.
    pub services: Vec<String>,
    /// The representative span's attributes, projected to the request's `projected_attributes`
    /// keys (only keys present on that span appear). Empty when no keys were requested.
    pub root_attributes: BTreeMap<String, String>,
}

/// The trace-list result: the (sorted + paged) summaries plus the full distinct-trace match count
/// (independent of paging and of the rollup cap).
pub struct TraceSearchResult {
    /// The page of trace summaries.
    pub traces: Vec<TraceSummary>,
    /// Distinct traces matching the request across the full pruned set (not limited by paging or
    /// [`MAX_CANDIDATE_TRACES`]).
    pub matched_count: u64,
}

/// A minimal decoded span, used only for the in-Rust rollup.
struct RawSpan {
    parent_span_id: Option<String>,
    service: Option<String>,
    name: Option<String>,
    start: i64,
    end: Option<i64>,
    duration: Option<i64>,
    status: Option<i32>,
    /// Projected attributes for this span (only the request's `projected_attributes` keys that
    /// are present). Always empty when no keys were requested — the map is never decoded then.
    attributes: BTreeMap<String, String>,
}

impl SpanQueryEngine {
    /// Prune → aggregate matched trace ids → fetch their whole-trace spans → roll up in Rust →
    /// sort + page. Returns an empty result (with `matched_count == 0`) when nothing survives
    /// pruning or nothing matches the predicate.
    pub async fn search_traces(
        &self,
        req: SpanQueryRequest,
    ) -> Result<TraceSearchResult, PhotonError> {
        let df = match self.span_survivors_df(&req).await? {
            Some(df) => df,
            None => {
                return Ok(TraceSearchResult {
                    traces: Vec::new(),
                    matched_count: 0,
                })
            }
        };

        // Step 2 — matched trace ids, ranked, plus the full distinct match count. Both derive from
        // one filtered handle (`filtered`) over the pruned survivors, so the grammar predicate and
        // the file set are shared (one prune/open, two bounded collects). Instead of collecting
        // every distinct id into a Rust `Vec`:
        //   * `matched_count` = `COUNT(DISTINCT trace_id)` — one row. SQL `COUNT(DISTINCT)` already
        //     skips null trace_ids, matching the old null-skipping loop.
        //   * the ranked page = `GROUP BY trace_id → min(start_time)`, `min_start DESC` (id-tiebroken)
        //     `LIMIT MAX_CANDIDATE_TRACES` — at most the cap-many rows ever reach Rust.
        // The ranking's `min_start DESC, id ASC` order + `LIMIT` selects the identical *set* of ids
        // the old Rust `sort_by` + `truncate` did (a total order on distinct ids), so results are
        // unchanged; the two bounded collects just replace the unbounded `Vec`.
        let filtered = df
            .clone()
            .filter(span_base_predicate(&req))
            .map_err(|e| PhotonError::Query(format!("trace match filter: {e}")))?;

        let count_batches = filtered
            .clone()
            .aggregate(
                vec![],
                vec![count_distinct(col_ref(span_schema::TRACE_ID)).alias("n")],
            )
            .map_err(|e| PhotonError::Query(format!("trace match count aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("trace match count collect: {e}")))?;
        let matched_count = match count_batches.first() {
            Some(b) if b.num_rows() > 0 => b
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .filter(|c| !c.is_null(0))
                .map(|c| c.value(0) as u64)
                .unwrap_or(0),
            _ => 0,
        };
        if matched_count == 0 {
            return Ok(TraceSearchResult {
                traces: Vec::new(),
                matched_count: 0,
            });
        }

        // A capped rollup scan is logged, never silent; `matched_count` still reports the full total.
        if matched_count > MAX_CANDIDATE_TRACES as u64 {
            eprintln!(
                "photon-query: search_traces rolling up only the newest {MAX_CANDIDATE_TRACES} of \
                 {matched_count} matched traces (v1 cap); matched_count reports the full total"
            );
        }

        // Rank + cap in DataFusion — newest traces first (`min_start DESC`), id-tiebroken for
        // determinism, `LIMIT MAX_CANDIDATE_TRACES`. Only these ≤ cap rows are materialized.
        let ranked_batches = filtered
            .aggregate(
                vec![col_ref(span_schema::TRACE_ID).alias("trace_id")],
                vec![min(col_ref(span_schema::START_TIME)).alias("ts")],
            )
            .map_err(|e| PhotonError::Query(format!("trace match aggregate: {e}")))?
            .filter(col("trace_id").is_not_null())
            .map_err(|e| PhotonError::Query(format!("trace match not-null: {e}")))?
            .sort(vec![
                col("ts").sort(false, false),      // min(start) DESC — newest traces first
                col("trace_id").sort(true, false), // id ASC — deterministic tiebreak
            ])
            .map_err(|e| PhotonError::Query(format!("trace match sort: {e}")))?
            .limit(0, Some(MAX_CANDIDATE_TRACES))
            .map_err(|e| PhotonError::Query(format!("trace match limit: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("trace match collect: {e}")))?;

        // Collect the capped ids and the min/max of their earliest-matching-span starts — the two
        // bounds that narrow step 3's whole-trace window (see `STEP3_WINDOW_PAD_NANOS`).
        let mut capped_ids: Vec<String> = Vec::new();
        let mut kept_start_min = i64::MAX;
        let mut kept_start_max = i64::MIN;
        for b in &ranked_batches {
            let ids = b
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PhotonError::Query("trace_id group column not Utf8".into()))?;
            let ts = b
                .column(1)
                .as_any()
                .downcast_ref::<TimestampNanosecondArray>()
                .ok_or_else(|| {
                    PhotonError::Query("min(start_time) column not Timestamp(nanos)".into())
                })?;
            for i in 0..b.num_rows() {
                if ids.is_null(i) {
                    continue;
                }
                capped_ids.push(ids.value(i).to_string());
                let start = if ts.is_null(i) { 0 } else { ts.value(i) };
                kept_start_min = kept_start_min.min(start);
                kept_start_max = kept_start_max.max(start);
            }
        }
        if capped_ids.is_empty() {
            // Unreachable when matched_count > 0 (`COUNT(DISTINCT)` counts exactly the non-null ids
            // the not-null filter keeps), but guard rather than build a bogus window from the
            // sentinel min/max.
            return Ok(TraceSearchResult {
                traces: Vec::new(),
                matched_count,
            });
        }

        // Whole-trace rollups must see EVERY span of the capped traces, including spans in files the
        // free-text name-bloom pruned out of `df` AND spans that straddle the request window's edges
        // (audit F10). Re-open survivors with the grammar dropped (time-window pruning only) over a
        // window NARROWED to the kept traces' earliest-matching-span span ± STEP3_WINDOW_PAD_NANOS:
        // narrowing lets min/max pruning drop files outside the kept traces' actual time range (the
        // win — the old code rescanned the ENTIRE original window for one page), while the ±1h pad
        // still admits every whole-trace span of the kept traces (see the pad-safety note on
        // STEP3_WINDOW_PAD_NANOS), incl. window-straddling ones the old un-padded original-window
        // step 3 dropped — so straddling traces are now rolled up completely. `df` (step 2's match
        // detection) stays free-text-pruned; matching is still complete because the bloom never
        // false-negatives.
        let whole_req = SpanQueryRequest {
            start_ts_nanos: kept_start_min.saturating_sub(STEP3_WINDOW_PAD_NANOS),
            end_ts_nanos: kept_start_max.saturating_add(STEP3_WINDOW_PAD_NANOS),
            query: None,
            sort: SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
        };
        let whole_df = match self.span_survivors_df(&whole_req).await? {
            Some(d) => d,
            // Unreachable: the file holding each kept trace's earliest-matching span overlaps the
            // narrowed window and passes query:None pruning, so survivors are non-empty.
            None => df,
        };

        // Requested root-span attribute keys. Empty ⇒ the wide `attributes` map is neither
        // projected nor decoded below — the hot path pays zero extra cost.
        let requested: BTreeSet<String> = req.projected_attributes.iter().cloned().collect();

        // Step 3 — every span of the capped traces (no grammar filter → whole-trace rollups).
        let in_list: Vec<Expr> = capped_ids.iter().map(|id| lit(id.clone())).collect();
        let mut select_cols = vec![
            col_ref(span_schema::TRACE_ID),
            col_ref(span_schema::PARENT_SPAN_ID),
            col_ref("service.name"),
            col_ref(span_schema::NAME),
            col_ref(span_schema::START_TIME),
            col_ref(span_schema::END_TIME),
            col_ref(span_schema::DURATION),
            col_ref(span_schema::STATUS_CODE),
        ];
        // Requested keys that are *promoted* attributes live in their own top-level Utf8 column —
        // `SpanBatchBuilder::append` explicitly excludes promoted keys from the `attributes` Map
        // (`span_record.rs`), so decoding only the Map would silently miss them (e.g. the default
        // config promotes `host.name`). Project those columns too. `service.name` is already
        // projected above (surfaced as `root_service`), so skip it to avoid a duplicate column.
        // Empty when nothing was requested, so the hot path still projects NOTHING extra.
        let promoted_requested: Vec<String> = self
            .promoted_attributes()
            .iter()
            .filter(|k| k.as_str() != "service.name" && requested.contains(k.as_str()))
            .cloned()
            .collect();
        if !requested.is_empty() {
            select_cols.push(col_ref(span_schema::ATTRIBUTES));
        }
        for key in &promoted_requested {
            select_cols.push(col_ref(key));
        }
        let span_batches = whole_df
            .filter(col_ref(span_schema::TRACE_ID).in_list(in_list, false))
            .map_err(|e| PhotonError::Query(format!("trace rollup filter: {e}")))?
            .select(select_cols)
            .map_err(|e| PhotonError::Query(format!("trace rollup select: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("trace rollup collect: {e}")))?;

        // Step 4 — group by trace_id, roll up in Rust.
        let mut by_trace: BTreeMap<String, Vec<RawSpan>> = BTreeMap::new();
        for b in &span_batches {
            let trace_id = str_col(b, span_schema::TRACE_ID)?;
            let parent = str_col(b, span_schema::PARENT_SPAN_ID)?;
            let service = str_col(b, "service.name")?;
            let name = str_col(b, span_schema::NAME)?;
            let start = ts_col(b, span_schema::START_TIME)?;
            let end = i64_col(b, span_schema::END_TIME)?;
            let duration = i64_col(b, span_schema::DURATION)?;
            let status = i32_col(b, span_schema::STATUS_CODE)?;
            // Decode the attributes Map only when keys were requested (else it isn't even projected).
            let attrs = if requested.is_empty() {
                None
            } else {
                Some(map_col(b, span_schema::ATTRIBUTES)?)
            };
            // The requested promoted keys are their own Utf8 columns (disjoint from the Map). Empty
            // when nothing was requested → still zero extra decode on the hot path.
            let promoted_cols: Vec<(&str, &StringArray)> = promoted_requested
                .iter()
                .map(|k| str_col(b, k).map(|c| (k.as_str(), c)))
                .collect::<Result<Vec<_>, PhotonError>>()?;
            for i in 0..b.num_rows() {
                if trace_id.is_null(i) {
                    continue;
                }
                let mut attributes = match attrs {
                    Some(m) => map_row_filtered(m, i, &requested),
                    None => BTreeMap::new(),
                };
                // Merge promoted-column values — no collision, the builder keeps them out of the Map.
                for (key, col) in &promoted_cols {
                    if !col.is_null(i) {
                        attributes.insert((*key).to_string(), col.value(i).to_string());
                    }
                }
                by_trace
                    .entry(trace_id.value(i).to_string())
                    .or_default()
                    .push(RawSpan {
                        parent_span_id: opt_str(parent, i),
                        service: opt_str(service, i),
                        name: opt_str(name, i),
                        start: if start.is_null(i) { 0 } else { start.value(i) },
                        end: opt_i64(end, i),
                        duration: opt_i64(duration, i),
                        status: opt_i32(status, i),
                        attributes,
                    });
            }
        }

        let mut summaries: Vec<TraceSummary> = by_trace
            .into_iter()
            .map(|(trace_id, spans)| rollup(trace_id, spans))
            .collect();

        // Step 5 — sort per req.sort, then page.
        sort_summaries(&mut summaries, req.sort);
        let traces = summaries
            .into_iter()
            .skip(req.offset)
            .take(req.limit)
            .collect();

        Ok(TraceSearchResult {
            traces,
            matched_count,
        })
    }
}

/// Roll one trace's spans up into a [`TraceSummary`]. `spans` is guaranteed non-empty by the
/// caller (a trace only exists here because it had at least one span).
fn rollup(trace_id: String, spans: Vec<RawSpan>) -> TraceSummary {
    let span_count = spans.len() as u64;
    let error_count = spans.iter().filter(|s| s.status == Some(2)).count() as u64;

    let services: Vec<String> = spans
        .iter()
        .filter_map(|s| s.service.clone())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();

    // Representative: the parent-less root if present, else the earliest-start span. `min_by_key`
    // returns the first element on ties, so this is deterministic for a given span order.
    let rep = spans
        .iter()
        .filter(|s| s.parent_span_id.is_none())
        .min_by_key(|s| s.start)
        .or_else(|| spans.iter().min_by_key(|s| s.start))
        .expect("trace group is non-empty");

    let duration_nanos = rep.duration.or_else(|| {
        let max_end = spans.iter().filter_map(|s| s.end).max();
        let min_start = spans.iter().map(|s| s.start).min();
        match (max_end, min_start) {
            (Some(e), Some(m)) => Some(e - m),
            _ => None,
        }
    });

    TraceSummary {
        trace_id,
        root_service: rep.service.clone(),
        root_name: rep.name.clone(),
        start_ts_nanos: rep.start,
        duration_nanos,
        span_count,
        error_count,
        services,
        root_attributes: rep.attributes.clone(),
    }
}

/// Sort summaries in place per the requested order. All modes share a `start DESC, trace_id ASC`
/// tiebreak so paging is deterministic.
fn sort_summaries(summaries: &mut [TraceSummary], sort: SpanSort) {
    match sort {
        SpanSort::Recent => summaries.sort_by(cmp_start_desc),
        SpanSort::Slowest => summaries
            .sort_by(|a, b| cmp_duration_desc_nulls_last(a, b).then_with(|| cmp_start_desc(a, b))),
        SpanSort::Errors => summaries.sort_by(|a, b| {
            b.error_count
                .cmp(&a.error_count)
                .then_with(|| cmp_start_desc(a, b))
        }),
    }
}

/// Newest first, tie-broken by ascending `trace_id` for determinism.
fn cmp_start_desc(a: &TraceSummary, b: &TraceSummary) -> Ordering {
    b.start_ts_nanos
        .cmp(&a.start_ts_nanos)
        .then_with(|| a.trace_id.cmp(&b.trace_id))
}

/// Longest duration first; unknown (`None`) durations sort last.
fn cmp_duration_desc_nulls_last(a: &TraceSummary, b: &TraceSummary) -> Ordering {
    match (a.duration_nanos, b.duration_nanos) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn str_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a StringArray, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| PhotonError::Query(format!("spans column `{name}` missing or not Utf8")))
}

fn ts_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a TimestampNanosecondArray, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<TimestampNanosecondArray>())
        .ok_or_else(|| {
            PhotonError::Query(format!(
                "spans column `{name}` missing or not Timestamp(nanos)"
            ))
        })
}

fn i64_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| PhotonError::Query(format!("spans column `{name}` missing or not Int64")))
}

fn i32_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int32Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .ok_or_else(|| PhotonError::Query(format!("spans column `{name}` missing or not Int32")))
}

fn opt_str(col: &StringArray, i: usize) -> Option<String> {
    if col.is_null(i) {
        None
    } else {
        Some(col.value(i).to_string())
    }
}

fn opt_i64(col: &Int64Array, i: usize) -> Option<i64> {
    if col.is_null(i) {
        None
    } else {
        Some(col.value(i))
    }
}

fn opt_i32(col: &Int32Array, i: usize) -> Option<i32> {
    if col.is_null(i) {
        None
    } else {
        Some(col.value(i))
    }
}

fn map_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a MapArray, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<MapArray>())
        .ok_or_else(|| PhotonError::Query(format!("spans column `{name}` missing or not a Map")))
}

/// Decode the `attributes` Map at `row`, keeping only entries whose key is in `requested`
/// (mirrors the `span_attributes` MapArray walk in `photon-api`'s `traces.rs`). Null values are
/// skipped. `requested` is guaranteed non-empty by the caller, so this is never a no-op decode.
fn map_row_filtered(
    map: &MapArray,
    row: usize,
    requested: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if map.is_null(row) {
        return out;
    }
    let offsets = map.value_offsets();
    let entries = map.entries();
    if let (Some(keys), Some(values)) = (
        entries.column(0).as_any().downcast_ref::<StringArray>(),
        entries.column(1).as_any().downcast_ref::<StringArray>(),
    ) {
        let start = offsets[row] as usize;
        let end = offsets[row + 1] as usize;
        for i in start..end {
            let key = keys.value(i);
            if !values.is_null(i) && requested.contains(key) {
                out.insert(key.to_string(), values.value(i).to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(
        parent: Option<&str>,
        start: i64,
        end: Option<i64>,
        dur: Option<i64>,
        status: Option<i32>,
    ) -> RawSpan {
        RawSpan {
            parent_span_id: parent.map(|s| s.to_string()),
            service: Some("svc".to_string()),
            name: Some("op".to_string()),
            start,
            end,
            duration: dur,
            status,
            attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn rollup_uses_parentless_root_as_representative() {
        let spans = vec![
            raw(Some("root"), 200, Some(400), Some(200), Some(1)),
            raw(None, 100, Some(500), Some(400), Some(2)), // the root, though not earliest listed
        ];
        let s = rollup("t".into(), spans);
        assert_eq!(s.start_ts_nanos, 100);
        assert_eq!(s.duration_nanos, Some(400));
        assert_eq!(s.span_count, 2);
        assert_eq!(s.error_count, 1);
    }

    #[test]
    fn rollup_without_root_uses_earliest_and_falls_back_to_span_span_duration() {
        // No parentless span; the earliest (start=90) has no duration → fallback to max_end -
        // min_start = 600 - 90 = 510.
        let spans = vec![
            raw(Some("p1"), 100, Some(300), Some(200), Some(1)),
            raw(Some("p2"), 90, Some(600), None, Some(1)),
        ];
        let s = rollup("t".into(), spans);
        assert_eq!(s.start_ts_nanos, 90);
        assert_eq!(s.duration_nanos, Some(510));
    }

    #[test]
    fn sort_slowest_puts_null_durations_last() {
        let mut v = vec![
            summary("a", 10, Some(50)),
            summary("b", 20, None),
            summary("c", 30, Some(900)),
        ];
        sort_summaries(&mut v, SpanSort::Slowest);
        let ids: Vec<&str> = v.iter().map(|s| s.trace_id.as_str()).collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    fn summary(id: &str, start: i64, dur: Option<i64>) -> TraceSummary {
        TraceSummary {
            trace_id: id.to_string(),
            root_service: None,
            root_name: None,
            start_ts_nanos: start,
            duration_nanos: dur,
            span_count: 1,
            error_count: 0,
            services: Vec::new(),
            root_attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn rollup_captures_only_the_representative_spans_attributes() {
        // The parent-less root (start 100) is the representative; its attributes surface as
        // `root_attributes`. A non-representative child's attributes must NOT leak in.
        let mut root = raw(None, 100, Some(500), Some(400), Some(1));
        root.attributes = BTreeMap::from([("http.route".to_string(), "/checkout".to_string())]);
        let mut child = raw(Some("root"), 200, Some(400), Some(200), Some(1));
        child.attributes = BTreeMap::from([("db.system".to_string(), "postgres".to_string())]);

        let s = rollup("t".into(), vec![child, root]);
        assert_eq!(
            s.root_attributes.get("http.route").map(String::as_str),
            Some("/checkout")
        );
        assert!(
            !s.root_attributes.contains_key("db.system"),
            "a non-representative span's attributes must not be captured"
        );
    }
}

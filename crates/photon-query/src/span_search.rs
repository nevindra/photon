//! `search_spans`: prune (via `SpanQueryEngine::span_prune`) then read the surviving spans
//! Parquet with DataFusion, sorted per `SpanSort`. `count_matching_spans`: `COUNT(*)` over the
//! same pruned + predicated set, ignoring `limit`/`offset` — mirrors `photon-query/src/count.rs`
//! but for spans.
//!
//! ## Two-pass late materialization — `SpanSort::Recent` only
//!
//! `Recent` reuses the exact timestamp-cutoff trick from the logs `QueryEngine::search`
//! (`lib.rs`): pass 1 projects only `start_time_nanos`, filters, sorts DESC, and takes the top
//! `offset + limit` rows to find the cutoff — the smallest `start_time_nanos` among that window,
//! i.e. the last row of the requested page. Pass 2 re-applies the predicate plus
//! `start_time_nanos >= cutoff` (a much smaller candidate set than the full survivors) and
//! re-runs `sort + limit(offset, limit)` against it, so the wide `attributes` map is decoded
//! only for rows at or above the cutoff instead of every scanned row.
//!
//! `Slowest` and `Errors` take a **single pass** for v1 (filter → sort → `limit(offset, limit)`),
//! per the design brief: pass-1 late materialization matters most for `Recent`, the default and
//! highest-traffic view; projecting the full row set before sorting is simpler and adequate at
//! milestone-1 data volumes for the other two sorts. Revisit with a cutoff trick per-sort if
//! profiling shows they need it too.

use arrow::array::{Array, Int64Array, TimestampNanosecondArray};
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::count;
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
            SpanSort::Slowest | SpanSort::Errors => search_single_pass(df, predicate, &req).await,
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
    /// `search_single_pass`, used by `search_spans` itself) and `count_matching_spans`'s
    /// `COUNT(*)` aggregate (`count_over`), instead of each independently re-pruning and
    /// re-opening the candidate set.
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
            SpanSort::Slowest | SpanSort::Errors => {
                search_single_pass(df.clone(), predicate.clone(), &req).await?
            }
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
    let window = req.offset + req.limit;

    // Pass 1 — cheap cutoff probe. Project only `start_time_nanos`, so the wide `attributes`
    // map (and other columns) are never decoded for rows we won't return.
    let cutoff_batches = df
        .clone()
        .filter(predicate.clone())
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .select(vec![col_ref(span_schema::START_TIME)])
        .map_err(|e| PhotonError::Query(format!("failed to project start_time: {e}")))?
        .sort(vec![ts_sort()])
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(0, Some(window))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect cutoff: {e}")))?;

    // The cutoff is the smallest start_time among the newest `offset + limit` matches — the
    // last row of the requested page. Nothing matched (or the window was 0) → no rows.
    let cutoff = match min_start_time(&cutoff_batches) {
        Some(c) => c,
        None => return Ok(Vec::new()),
    };

    // Pass 2 — full rows, but only from `[cutoff, end]`. Re-applying the predicate plus
    // `start_time_nanos >= cutoff`, then re-sorting/paging, yields exactly the single-pass
    // result (ties at `cutoff` trimmed by the limit), while decoding the heavy columns for only
    // the rows at or above the cutoff.
    let predicate =
        predicate.and(col_ref(span_schema::START_TIME).gt_eq(lit_timestamp_nano(cutoff)));
    df.filter(predicate)
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .sort(vec![ts_sort()])
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(req.offset, Some(req.limit))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect results: {e}")))
}

/// `SpanSort::Slowest` / `SpanSort::Errors` — single pass (filter → sort → page). See module
/// docs for why this skips the two-pass cutoff trick for v1.
async fn search_single_pass(
    df: DataFrame,
    predicate: Expr,
    req: &SpanQueryRequest,
) -> Result<Vec<RecordBatch>, PhotonError> {
    let sort_exprs = match req.sort {
        // Longest spans first; nulls (unmeasured duration) last.
        SpanSort::Slowest => vec![col_ref(span_schema::DURATION).sort(false, false)],
        // Higher status codes (errors) first, newest-first within a status tier.
        SpanSort::Errors => vec![
            col_ref(span_schema::STATUS_CODE).sort(false, false),
            col_ref(span_schema::START_TIME).sort(false, false),
        ],
        SpanSort::Recent => unreachable!("search_recent handles SpanSort::Recent"),
    };
    df.filter(predicate)
        .map_err(|e| PhotonError::Query(format!("failed to apply predicate: {e}")))?
        .sort(sort_exprs)
        .map_err(|e| PhotonError::Query(format!("failed to sort: {e}")))?
        .limit(req.offset, Some(req.limit))
        .map_err(|e| PhotonError::Query(format!("failed to apply limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("failed to collect results: {e}")))
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

/// Smallest `start_time_nanos` across single-column batches, or `None` when there are no rows.
/// Reads back the pass-1 cutoff (the last row's timestamp in the requested page).
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
        let batches = search_single_pass(df, span_base_predicate(&r), &r)
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
        let batches = search_single_pass(df, span_base_predicate(&r), &r)
            .await
            .unwrap();
        // Order by duration DESC is s2(900), s3(200), s1(50); offset=1, limit=1 -> s3.
        assert_eq!(span_ids(&batches), vec!["s3".to_string()]);
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
        let batches = search_single_pass(df, span_base_predicate(&r), &r)
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

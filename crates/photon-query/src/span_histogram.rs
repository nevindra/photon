//! `histogram` for spans: status-stacked volume over the FULL match set (not just the returned
//! rows). Mirrors `crate::histogram` (the logs histogram): the heavy work of bucketing, grouping,
//! and counting happens in DataFusion; the tiny grouped result (≤ buckets × distinct-statuses
//! rows) is folded into fixed buckets in Rust.
use arrow::array::{Array, Int32Array, Int64Array};
use arrow::datatypes::DataType;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::prelude::{cast, lit, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// One time bucket with per-status counts. `t` is the bucket-start timestamp (epoch nanos).
#[derive(Debug, Clone, PartialEq)]
pub struct SpanHistogramBucket {
    pub t: i64,
    pub ok: u64,
    pub error: u64,
    pub unset: u64,
    pub total: u64,
}

impl SpanQueryEngine {
    /// `buckets` equal-width time buckets over `[start, end]` (by `start_time_nanos`), each
    /// split by `status_code`.
    pub async fn histogram(
        &self,
        req: SpanQueryRequest,
        buckets: usize,
    ) -> Result<Vec<SpanHistogramBucket>, PhotonError> {
        let buckets = buckets.max(1);
        let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);
        match self.span_survivors_df(&req).await? {
            None => Ok(empty_buckets(start, end, buckets)),
            Some(df) => histogram_over(df, span_base_predicate(&req), start, end, buckets).await,
        }
    }
}

/// The start timestamp (epoch nanos) of bucket `i` of `buckets` spanning `[start, end]`.
fn bucket_start(start: i64, end: i64, buckets: usize, i: usize) -> i64 {
    let span = (end - start) as i128;
    start + (span * i as i128 / buckets as i128) as i64
}

fn empty_buckets(start: i64, end: i64, buckets: usize) -> Vec<SpanHistogramBucket> {
    (0..buckets)
        .map(|i| SpanHistogramBucket {
            t: bucket_start(start, end, buckets, i),
            ok: 0,
            error: 0,
            unset: 0,
            total: 0,
        })
        .collect()
}

/// `status_code` → the status slot index used by `SpanHistogramBucket`. OTLP status codes:
/// 0 = Unset, 1 = Ok, 2 = Error. Anything else (incl. `Some(0)`/NULL/unrecognized) → unset.
fn status_slot(status: Option<i32>) -> usize {
    match status {
        Some(1) => 0, // ok
        Some(2) => 1, // error
        _ => 2,       // unset (incl. Some(0), NULL, out-of-range)
    }
}

async fn histogram_over(
    df: DataFrame,
    predicate: Expr,
    start: i64,
    end: i64,
    buckets: usize,
) -> Result<Vec<SpanHistogramBucket>, PhotonError> {
    let span = (end - start).max(1);
    // bucket = ((start_time_nanos - start) * buckets) / span, integer division. All rows satisfy
    // the predicate's `start_time_nanos BETWEEN start AND end`, so bucket ∈ [0, buckets]; a row
    // at exactly `end` maps to `buckets`, which we clamp down to the last bucket.
    let start_time = cast(col_ref(span_schema::START_TIME), DataType::Int64);
    let raw = (start_time - lit(start)) * lit(buckets as i64) / lit(span);
    let bucket = when(
        raw.clone().gt_eq(lit(buckets as i64)),
        lit(buckets as i64 - 1),
    )
    .otherwise(raw)
    .map_err(|e| PhotonError::Query(format!("span histogram bucket case: {e}")))?;

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("span histogram filter: {e}")))?
        .aggregate(
            vec![
                bucket.alias("bucket"),
                col_ref(span_schema::STATUS_CODE).alias("status"),
            ],
            vec![count(lit(1i64)).alias("n")],
        )
        .map_err(|e| PhotonError::Query(format!("span histogram aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("span histogram collect: {e}")))?;

    let mut out = empty_buckets(start, end, buckets);
    for b in &batches {
        let bucket_col = b
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| PhotonError::Query("span histogram bucket column not Int64".into()))?;
        let status_col = b
            .column(1)
            .as_any()
            .downcast_ref::<Int32Array>()
            .ok_or_else(|| PhotonError::Query("span histogram status column not Int32".into()))?;
        let n_col = b
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| PhotonError::Query("span histogram count column not Int64".into()))?;
        for i in 0..b.num_rows() {
            let idx = bucket_col.value(i).clamp(0, buckets as i64 - 1) as usize;
            let status = if status_col.is_null(i) {
                None
            } else {
                Some(status_col.value(i))
            };
            let n = n_col.value(i).max(0) as u64;
            let slot = status_slot(status);
            let hb = &mut out[idx];
            match slot {
                0 => hb.ok += n,
                1 => hb.error += n,
                _ => hb.unset += n,
            }
            hb.total += n;
        }
    }
    Ok(out)
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

    fn span(start: i64, status: Option<i32>) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), "api".to_string());
        SpanRecord {
            trace_id: "t1".into(),
            span_id: format!("s{start}"),
            name: Some("op".into()),
            start_time_nanos: start,
            status_code: status,
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

    fn lit_true() -> Expr {
        datafusion::prelude::lit(true)
    }

    #[tokio::test]
    async fn buckets_by_time_and_status() {
        // window [0, 100), 2 buckets → [0,50), [50,100]. Widths from bucket_start.
        let records = vec![
            span(10, Some(2)), // bucket 0, error
            span(20, Some(1)), // bucket 0, ok
            span(60, Some(2)), // bucket 1, error
            span(99, None),    // bucket 1, null → unset
        ];
        let df = df_of(&records).await;
        let out = histogram_over(df, lit_true(), 0, 100, 2).await.unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].t, 0);
        assert_eq!(out[0].error, 1);
        assert_eq!(out[0].ok, 1);
        assert_eq!(out[0].total, 2);
        assert_eq!(out[1].t, 50);
        assert_eq!(out[1].error, 1);
        assert_eq!(out[1].unset, 1); // the null-status row lands in unset
        assert_eq!(out[1].total, 2);
    }

    #[tokio::test]
    async fn status_zero_lands_in_unset() {
        let records = vec![span(10, Some(0))];
        let df = df_of(&records).await;
        let out = histogram_over(df, lit_true(), 0, 100, 1).await.unwrap();
        assert_eq!(out[0].unset, 1);
        assert_eq!(out[0].ok, 0);
        assert_eq!(out[0].error, 0);
        assert_eq!(out[0].total, 1);
    }

    #[tokio::test]
    async fn empty_window_yields_all_zero_buckets() {
        let df = df_of(&[]).await;
        let out = histogram_over(df, lit_true(), 0, 100, 4).await.unwrap();
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|b| b.total == 0));
        assert_eq!(
            out.iter().map(|b| b.t).collect::<Vec<_>>(),
            vec![0, 25, 50, 75]
        );
    }
}

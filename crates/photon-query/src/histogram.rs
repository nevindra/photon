//! `histogram`: severity-stacked volume over the FULL match set (not just the returned rows).
//! The heavy work — bucketing + grouping + counting — happens in DataFusion; the tiny grouped
//! result (≤ buckets × distinct-severities rows) is folded into fixed buckets in Rust.
use arrow::array::{Array, Int32Array, Int64Array};
use arrow::datatypes::DataType;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::prelude::{cast, lit, when, Expr};

use photon_core::schema;
use photon_core::PhotonError;

use crate::{base_predicate, col_ref, QueryEngine, QueryRequest};

/// One time bucket with per-severity counts. `t` is the bucket-start timestamp (epoch nanos).
#[derive(Debug, Clone, PartialEq)]
pub struct HistogramBucket {
    pub t: i64,
    pub debug: u64,
    pub info: u64,
    pub warn: u64,
    pub error: u64,
    pub fatal: u64,
    pub total: u64,
}

impl QueryEngine {
    /// `buckets` equal-width time buckets over `[start, end]`, each split by severity.
    pub async fn histogram(
        &self,
        req: QueryRequest,
        buckets: usize,
    ) -> Result<Vec<HistogramBucket>, PhotonError> {
        let buckets = buckets.max(1);
        let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);
        match self.survivors_df(&req).await? {
            None => Ok(empty_buckets(start, end, buckets)),
            Some(df) => histogram_over(df, base_predicate(&req), start, end, buckets).await,
        }
    }
}

/// The start timestamp (epoch nanos) of bucket `i` of `buckets` spanning `[start, end]`.
fn bucket_start(start: i64, end: i64, buckets: usize, i: usize) -> i64 {
    let span = (end - start) as i128;
    start + (span * i as i128 / buckets as i128) as i64
}

fn empty_buckets(start: i64, end: i64, buckets: usize) -> Vec<HistogramBucket> {
    (0..buckets)
        .map(|i| HistogramBucket {
            t: bucket_start(start, end, buckets, i),
            debug: 0,
            info: 0,
            warn: 0,
            error: 0,
            fatal: 0,
            total: 0,
        })
        .collect()
}

/// `severity_number` → the level slot index used by `HistogramBucket`. Ranges match the resolver:
/// debug 1-8, info 9-12, warn 13-16, error 17-20, fatal 21-24; anything else (incl. NULL/0) → info.
fn level_slot(sev: Option<i32>) -> usize {
    match sev {
        Some(n) if (1..=8).contains(&n) => 0,   // debug
        Some(n) if (13..=16).contains(&n) => 2, // warn
        Some(n) if (17..=20).contains(&n) => 3, // error
        Some(n) if (21..=24).contains(&n) => 4, // fatal
        _ => 1,                                 // info (incl. 9-12, NULL, out-of-range)
    }
}

pub(crate) async fn histogram_over(
    df: DataFrame,
    predicate: Expr,
    start: i64,
    end: i64,
    buckets: usize,
) -> Result<Vec<HistogramBucket>, PhotonError> {
    let span = (end - start).max(1);
    // bucket = ((ts_nanos - start) * buckets) / span, integer division. All rows satisfy the
    // predicate's `ts BETWEEN start AND end`, so bucket ∈ [0, buckets]; ts == end maps to
    // `buckets`, which we clamp down to the last bucket.
    let ts_nanos = cast(col_ref(schema::TIMESTAMP), DataType::Int64);
    let raw = (ts_nanos - lit(start)) * lit(buckets as i64) / lit(span);
    let bucket = when(
        raw.clone().gt_eq(lit(buckets as i64)),
        lit(buckets as i64 - 1),
    )
    .otherwise(raw)
    .map_err(|e| PhotonError::Query(format!("histogram bucket case: {e}")))?;

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("histogram filter: {e}")))?
        .aggregate(
            vec![
                bucket.alias("bucket"),
                col_ref(schema::SEVERITY_NUMBER).alias("sev"),
            ],
            vec![count(lit(1i64)).alias("n")],
        )
        .map_err(|e| PhotonError::Query(format!("histogram aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("histogram collect: {e}")))?;

    let mut out = empty_buckets(start, end, buckets);
    for b in &batches {
        let bucket_col = b
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| PhotonError::Query("histogram bucket column not Int64".into()))?;
        let sev_col = b
            .column(1)
            .as_any()
            .downcast_ref::<Int32Array>()
            .ok_or_else(|| PhotonError::Query("histogram sev column not Int32".into()))?;
        let n_col = b
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| PhotonError::Query("histogram count column not Int64".into()))?;
        for i in 0..b.num_rows() {
            let idx = bucket_col.value(i).clamp(0, buckets as i64 - 1) as usize;
            let sev = if sev_col.is_null(i) {
                None
            } else {
                Some(sev_col.value(i))
            };
            let n = n_col.value(i).max(0) as u64;
            let slot = level_slot(sev);
            let hb = &mut out[idx];
            match slot {
                0 => hb.debug += n,
                2 => hb.warn += n,
                3 => hb.error += n,
                4 => hb.fatal += n,
                _ => hb.info += n,
            }
            hb.total += n;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;
    use std::collections::BTreeMap;

    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;

    fn rec(ts: i64, sev: Option<i32>) -> LogRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), "api".to_string());
        LogRecord {
            timestamp_nanos: ts,
            severity_number: sev,
            body: Some("x".into()),
            attributes,
            ..Default::default()
        }
    }

    async fn df_of(records: &[LogRecord]) -> datafusion::dataframe::DataFrame {
        let schema = LogSchema::new(&["service.name".into()]);
        let mut b = RecordBatchBuilder::new(&schema);
        for r in records {
            b.append(r);
        }
        let ctx = SessionContext::new();
        ctx.register_table(
            "logs",
            Arc::new(
                MemTable::try_new(schema.arrow.clone(), vec![vec![b.finish().unwrap()]]).unwrap(),
            ),
        )
        .unwrap();
        ctx.table("logs").await.unwrap()
    }

    #[tokio::test]
    async fn buckets_by_time_and_severity() {
        // window [0, 100), 2 buckets → [0,50), [50,100]. Widths from bucket_start.
        let records = vec![
            rec(10, Some(18)), // bucket 0, error
            rec(20, Some(9)),  // bucket 0, info
            rec(60, Some(18)), // bucket 1, error
            rec(99, None),     // bucket 1, null → info
        ];
        let df = df_of(&records).await;
        let out = histogram_over(df, lit_true(), 0, 100, 2).await.unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].t, 0);
        assert_eq!(out[0].error, 1);
        assert_eq!(out[0].info, 1);
        assert_eq!(out[0].total, 2);
        assert_eq!(out[1].t, 50);
        assert_eq!(out[1].error, 1);
        assert_eq!(out[1].info, 1); // the null-severity row lands in info
        assert_eq!(out[1].total, 2);
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

    fn lit_true() -> datafusion::prelude::Expr {
        datafusion::prelude::lit(true)
    }
}

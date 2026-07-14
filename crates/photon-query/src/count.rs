//! `count_matching`: a `COUNT(*)` over the pruned candidate set — the true `matched_count`,
//! independent of any row limit. Reuses the same pruning + predicate as `search`.
use arrow::array::{Array, Int64Array};
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::prelude::{lit, Expr};

use photon_core::PhotonError;

use crate::{base_predicate, QueryEngine, QueryRequest};

impl QueryEngine {
    /// Total rows matching `req` across the full (pruned) candidate set — not limited.
    pub async fn count_matching(&self, req: QueryRequest) -> Result<u64, PhotonError> {
        match self.survivors_df(&req).await? {
            None => Ok(0),
            Some(df) => count_over(df, base_predicate(&req)).await,
        }
    }
}

/// Apply `predicate`, then a global `COUNT(*)`; read back the single scalar. `pub(crate)` so
/// `QueryEngine::search_with_count` (`lib.rs`) can reuse it against a `DataFrame` it already
/// pruned/opened, instead of `search` and `count_matching` each re-pruning independently.
pub(crate) async fn count_over(df: DataFrame, predicate: Expr) -> Result<u64, PhotonError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use arrow::record_batch::RecordBatch;
    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;
    use std::collections::BTreeMap;

    use photon_core::query::{parse, FieldResolver};
    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;

    fn schema() -> LogSchema {
        LogSchema::new(&["service.name".into(), "host.name".into()])
    }

    fn rec(ts: i64, service: &str, sev: Option<i32>, body: &str) -> LogRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        LogRecord {
            timestamp_nanos: ts,
            severity_number: sev,
            body: Some(body.into()),
            attributes,
            ..Default::default()
        }
    }

    async fn df_of(records: &[LogRecord]) -> datafusion::dataframe::DataFrame {
        let schema = schema();
        let mut b = RecordBatchBuilder::new(&schema);
        for r in records {
            b.append(r);
        }
        let batch: RecordBatch = b.finish().unwrap();
        let ctx = SessionContext::new();
        ctx.register_table(
            "logs",
            Arc::new(MemTable::try_new(schema.arrow.clone(), vec![vec![batch]]).unwrap()),
        )
        .unwrap();
        ctx.table("logs").await.unwrap()
    }

    fn req(query: &str) -> QueryRequest {
        let promoted = ["service.name".to_string(), "host.name".to_string()];
        let rq = FieldResolver::new(&promoted)
            .resolve(&parse(query).unwrap())
            .unwrap();
        QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: i64::MAX,
            services: vec![],
            severities: vec![],
            text: None,
            query: Some(rq),
            limit: 1, // deliberately tiny — count must ignore it
        }
    }

    #[tokio::test]
    async fn counts_full_match_set_ignoring_limit() {
        let records = vec![
            rec(1, "api", Some(18), "boom"),
            rec(2, "api", Some(10), "ok"),
            rec(3, "web", Some(18), "boom"),
        ];
        let df = df_of(&records).await;
        // "service:api" matches records 1 and 2 → 2, even though limit = 1.
        let n = count_over(df, crate::base_predicate(&req("service:api")))
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    #[tokio::test]
    async fn empty_match_is_zero() {
        let df = df_of(&[rec(1, "api", Some(18), "boom")]).await;
        let n = count_over(df, crate::base_predicate(&req("service:nope")))
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}

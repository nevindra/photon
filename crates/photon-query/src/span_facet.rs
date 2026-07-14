//! `facet` for spans: top values + counts for one field over the pruned+filtered set. Mirrors
//! `crate::facet` (the logs facet) but resolves fields via `SpanFieldResolver` and reuses
//! `span_field_col` so a facet groups by exactly the same value expression the spans grammar
//! filters on (fixed column, promoted column, or `get_field` map access for long-tail keys).
use arrow::array::{Array, Int64Array, StringArray};
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::prelude::{col, lit, Expr};

use photon_core::query::SpanFieldResolver;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::{FacetResult, FacetValue, SpanQueryEngine, SpanQueryRequest};

impl SpanQueryEngine {
    /// Top `limit` values of `field` (by count) among spans matching `req`.
    pub async fn facet(
        &self,
        field: &str,
        req: SpanQueryRequest,
        limit: usize,
    ) -> Result<FacetResult, PhotonError> {
        let value = self.facet_value_expr(field)?;
        match self.span_survivors_df(&req).await? {
            None => Ok(FacetResult {
                values: Vec::new(),
                capped: false,
            }),
            Some(df) => facet_over(df, span_base_predicate(&req), value, limit).await,
        }
    }

    /// Resolve a facet field name to its value `Expr` via the same rules the spans grammar uses.
    fn facet_value_expr(&self, field: &str) -> Result<Expr, PhotonError> {
        let fr = SpanFieldResolver::new(self.promoted_attributes())
            .resolve_field_name(field)
            .map_err(|e| PhotonError::Query(format!("cannot facet on `{field}`: {}", e.message)))?;
        Ok(crate::span_predicate::span_field_col(&fr))
    }
}

/// GROUP BY `value_expr`, COUNT, drop the NULL (absent-field) group, order by count desc, and
/// fetch `limit + 1` so the caller can tell whether the field's cardinality exceeded `limit`.
async fn facet_over(
    df: DataFrame,
    predicate: Expr,
    value_expr: Expr,
    limit: usize,
) -> Result<FacetResult, PhotonError> {
    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("facet filter: {e}")))?
        .aggregate(
            vec![value_expr.alias("value")],
            vec![count(lit(1i64)).alias("n")],
        )
        .map_err(|e| PhotonError::Query(format!("facet aggregate: {e}")))?
        .filter(col("value").is_not_null())
        .map_err(|e| PhotonError::Query(format!("facet not-null: {e}")))?
        .sort(vec![
            col("n").sort(false, false),    // count desc
            col("value").sort(true, false), // value asc — stable tiebreak
        ])
        .map_err(|e| PhotonError::Query(format!("facet sort: {e}")))?
        .limit(0, Some(limit + 1))
        .map_err(|e| PhotonError::Query(format!("facet limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("facet collect: {e}")))?;

    let mut values = Vec::new();
    for b in &batches {
        let v = b
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| PhotonError::Query("facet value column not Utf8".into()))?;
        let n = b
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| PhotonError::Query("facet count column not Int64".into()))?;
        for i in 0..b.num_rows() {
            values.push(FacetValue {
                value: v.value(i).to_string(),
                count: n.value(i).max(0) as u64,
            });
        }
    }
    let capped = values.len() > limit;
    values.truncate(limit);
    Ok(FacetResult { values, capped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    use photon_core::query::parse;
    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;

    use crate::SpanSort;

    fn schema() -> SpanSchema {
        SpanSchema::new(&["service.name".into()])
    }

    fn span(start: i64, service: &str, attrs: &[(&str, &str)]) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        SpanRecord {
            trace_id: "t1".into(),
            span_id: format!("s{start}"),
            name: Some("op".into()),
            start_time_nanos: start,
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

    fn req() -> SpanQueryRequest {
        SpanQueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: i64::MAX,
            query: Some(
                SpanFieldResolver::new(&["service.name".to_string()])
                    .resolve(&parse("").unwrap())
                    .unwrap(),
            ),
            sort: SpanSort::Recent,
            limit: 500,
            offset: 0,
            projected_attributes: Vec::new(),
        }
    }

    fn value_expr(field: &str) -> Expr {
        let fr = SpanFieldResolver::new(&["service.name".to_string()])
            .resolve_field_name(field)
            .unwrap();
        crate::span_predicate::span_field_col(&fr)
    }

    #[tokio::test]
    async fn facets_promoted_column_by_count_desc() {
        let records = vec![
            span(1, "api", &[]),
            span(2, "api", &[]),
            span(3, "web", &[]),
        ];
        let df = df_of(&records).await;
        let r = facet_over(
            df,
            span_base_predicate(&req()),
            value_expr("service.name"),
            50,
        )
        .await
        .unwrap();
        assert!(!r.capped);
        assert_eq!(r.values[0].value, "api");
        assert_eq!(r.values[0].count, 2);
        assert_eq!(r.values[1].value, "web");
        assert_eq!(r.values[1].count, 1);
    }

    #[tokio::test]
    async fn facets_long_tail_map_attr_and_skips_absent() {
        let records = vec![
            span(1, "api", &[("region", "us")]),
            span(2, "api", &[("region", "us")]),
            span(3, "api", &[]), // no region → NULL group is dropped
        ];
        let df = df_of(&records).await;
        let r = facet_over(df, span_base_predicate(&req()), value_expr("region"), 50)
            .await
            .unwrap();
        assert_eq!(r.values.len(), 1);
        assert_eq!(r.values[0].value, "us");
        assert_eq!(r.values[0].count, 2);
    }

    #[tokio::test]
    async fn caps_when_more_than_limit_values() {
        let records = vec![span(1, "a", &[]), span(2, "b", &[]), span(3, "c", &[])];
        let df = df_of(&records).await;
        let r = facet_over(
            df,
            span_base_predicate(&req()),
            value_expr("service.name"),
            2,
        )
        .await
        .unwrap();
        assert!(r.capped);
        assert_eq!(r.values.len(), 2);
    }
}

//! `facet`: top values + counts for one field over the pruned+filtered set. Reuses `field_col`
//! so a facet groups by exactly the same value expression the grammar filters on (fixed column,
//! promoted column, or `get_field` map access for long-tail keys).
use arrow::array::{Array, Int64Array, StringArray};
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::prelude::{col, lit, Expr};

use photon_core::query::FieldResolver;
use photon_core::PhotonError;

use crate::{base_predicate, QueryEngine, QueryRequest};

/// One facet bucket: a distinct field value and how many matching rows have it.
pub struct FacetValue {
    pub value: String,
    pub count: u64,
}

/// Facet result: values sorted by count desc, plus `capped` = there were more than `limit`.
pub struct FacetResult {
    pub values: Vec<FacetValue>,
    pub capped: bool,
}

impl QueryEngine {
    /// Top `limit` values of `field` (by count) among rows matching `req`.
    pub async fn facet(
        &self,
        field: &str,
        req: QueryRequest,
        limit: usize,
    ) -> Result<FacetResult, PhotonError> {
        // Defense-in-depth: mirrors `photon-api`'s `MAX_LIMIT` (`crates/photon-api/src/
        // query_params.rs`); `photon-query` can't depend on `photon-api`, so the value is
        // restated here as a literal. No floor at 1 (unlike the `buckets` clamps elsewhere in
        // this crate) — `limit=0` is a pre-existing, legitimate input (zero values back, not
        // "unlimited"), so only the upper bound is new.
        let limit = limit.min(1000);
        let value = self.facet_value_expr(field)?;
        match self.survivors_df(&req).await? {
            None => Ok(FacetResult {
                values: Vec::new(),
                capped: false,
            }),
            Some(df) => facet_over(df, base_predicate(&req), value, limit).await,
        }
    }

    /// Resolve a facet field name to its value `Expr` via the same rules the grammar uses.
    fn facet_value_expr(&self, field: &str) -> Result<Expr, PhotonError> {
        let fr = FieldResolver::new(self.promoted_attributes())
            .resolve_field_name(field)
            .map_err(|e| PhotonError::Query(format!("cannot facet on `{field}`: {}", e.message)))?;
        Ok(crate::predicate::field_col(&fr))
    }
}

/// GROUP BY `value_expr`, COUNT, drop the NULL (absent-field) group, order by count desc, and
/// fetch `limit + 1` so the caller can tell whether the field's cardinality exceeded `limit`.
pub(crate) async fn facet_over(
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
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;
    use std::collections::BTreeMap;

    use photon_core::query::{parse, FieldResolver};
    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;

    fn schema() -> LogSchema {
        LogSchema::new(&["service.name".into(), "host.name".into()])
    }

    fn rec(ts: i64, service: &str, attrs: &[(&str, &str)]) -> LogRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        LogRecord {
            timestamp_nanos: ts,
            severity_number: Some(9),
            body: Some("x".into()),
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

    fn req() -> QueryRequest {
        QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: i64::MAX,
            services: vec![],
            severities: vec![],
            text: None,
            query: Some(
                FieldResolver::new(&["service.name".to_string(), "host.name".to_string()])
                    .resolve(&parse("").unwrap())
                    .unwrap(),
            ),
            limit: 500,
        }
    }

    fn value_expr(field: &str) -> Expr {
        let fr = FieldResolver::new(&["service.name".to_string(), "host.name".to_string()])
            .resolve_field_name(field)
            .unwrap();
        crate::predicate::field_col(&fr)
    }

    #[tokio::test]
    async fn facets_promoted_column_by_count_desc() {
        let records = vec![rec(1, "api", &[]), rec(2, "api", &[]), rec(3, "web", &[])];
        let df = df_of(&records).await;
        let r = facet_over(
            df,
            crate::base_predicate(&req()),
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
            rec(1, "api", &[("region", "us")]),
            rec(2, "api", &[("region", "us")]),
            rec(3, "api", &[]), // no region → NULL group is dropped
        ];
        let df = df_of(&records).await;
        let r = facet_over(df, crate::base_predicate(&req()), value_expr("region"), 50)
            .await
            .unwrap();
        assert_eq!(r.values.len(), 1);
        assert_eq!(r.values[0].value, "us");
        assert_eq!(r.values[0].count, 2);
    }

    #[tokio::test]
    async fn caps_when_more_than_limit_values() {
        let records = vec![rec(1, "a", &[]), rec(2, "b", &[]), rec(3, "c", &[])];
        let df = df_of(&records).await;
        let r = facet_over(
            df,
            crate::base_predicate(&req()),
            value_expr("service.name"),
            2,
        )
        .await
        .unwrap();
        assert!(r.capped);
        assert_eq!(r.values.len(), 2);
    }

    /// Fail-loud OOM backstop under the [`FairSpillPool`] (`crate::session`). The facet
    /// `GROUP BY value` is the canonical unbounded-RAM path (the grouped hash table holds every
    /// distinct value before the limit trims it). Run over a context whose DataFusion memory pool
    /// is 1 byte, it must ERROR with a `ResourcesExhausted` (surfaced as `PhotonError::Query`)
    /// instead of building an unbounded table / OOMing. Proves `session_with_memory_pool_bytes`
    /// actually caps the path, globally, at the engine seam.
    ///
    /// The swap to `FairSpillPool` does **not** make this path spill-and-succeed at 1 byte: the
    /// aggregation plan contains an **unspillable** `RepartitionExec` (it buffers batches for
    /// parallel aggregation and cannot spill), whose first `try_grow` (~6 KiB) already exceeds the
    /// whole 1-byte budget → `ResourcesExhausted`. That is exactly the unspillable-overflow backstop
    /// FairSpillPool retains: spillable operators degrade to disk when they exceed their fair share,
    /// but truly-unspillable ones that overflow the remaining budget still fail loud, so the pool
    /// stays the final guard against OOM even when nothing can spill.
    #[tokio::test]
    async fn facet_over_tiny_memory_pool_fails_loud_not_oom() {
        // 300 distinct services ⇒ a grouped hash table that cannot fit in a 1-byte pool.
        let schema = schema();
        let mut b = RecordBatchBuilder::new(&schema);
        for i in 0..300i64 {
            b.append(&rec(i, &format!("svc-{i}"), &[]));
        }
        let ctx = crate::session_with_memory_pool_bytes(1);
        ctx.register_table(
            "logs",
            Arc::new(
                MemTable::try_new(schema.arrow.clone(), vec![vec![b.finish().unwrap()]]).unwrap(),
            ),
        )
        .unwrap();
        let df = ctx.table("logs").await.unwrap();

        let result = facet_over(
            df,
            crate::base_predicate(&req()),
            value_expr("service.name"),
            50,
        )
        .await;
        let Err(err) = result else {
            panic!("a 1-byte memory pool must make the GROUP BY fail loud, not OOM");
        };
        let msg = format!("{err}").to_lowercase();
        assert!(
            msg.contains("resources exhausted"),
            "expected a DataFusion ResourcesExhausted error, got: {msg}"
        );
    }
}

/// End-to-end coverage of the public `QueryEngine::facet` entry point against a real compacted
/// store with more than `MAX_LIMIT` distinct values — proves the defensive clamp actually
/// truncates, not just that it compiles.
#[cfg(test)]
mod engine_tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use arrow::record_batch::RecordBatch;
    use object_store::local::LocalFileSystem;
    use photon_compact::Compactor;
    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;
    use photon_core::segment::SegmentId;
    use photon_core::PhotonError;
    use photon_storage::{Replicator, Storage};
    use photon_wal::Wal;

    use crate::{QueryEngine, QueryRequest};

    /// Minimal in-memory WAL handing the compactor one pre-built segment, mirroring the
    /// `FakeWal` fixtures in `infra.rs`/`metric_classic_hist.rs`/`span_latency.rs`.
    struct FakeWal {
        segments: Mutex<Vec<(SegmentId, Vec<RecordBatch>)>>,
    }
    #[allow(clippy::manual_async_fn)]
    impl Wal for FakeWal {
        fn append(
            &self,
            _b: RecordBatch,
        ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!() }
        }
        fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!() }
        }
        fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
            let mut ids: Vec<SegmentId> = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .map(|(id, _)| *id)
                .collect();
            ids.sort();
            Ok(ids)
        }
        fn read_segment(
            &self,
            id: SegmentId,
        ) -> impl std::future::Future<Output = Result<Vec<RecordBatch>, PhotonError>> + Send
        {
            let batches = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .find(|(sid, _)| *sid == id)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            async move { Ok(batches) }
        }
        fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError> {
            self.segments.lock().unwrap().retain(|(sid, _)| *sid != id);
            Ok(())
        }
    }

    #[tokio::test]
    async fn engine_clamps_a_dos_sized_limit() {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = LogSchema::new(&["service.name".to_string()]);

        // 1200 distinct `service.name` values — more than MAX_LIMIT (1000), so an unclamped
        // `limit=10_000_000` would return all 1200 and an actually-clamped one returns <=1000.
        let mut b = RecordBatchBuilder::new(&schema);
        for i in 0..1200 {
            let mut attributes = BTreeMap::new();
            attributes.insert("service.name".to_string(), format!("svc-{i}"));
            b.append(&LogRecord {
                timestamp_nanos: i as i64,
                severity_number: Some(9),
                body: Some("x".into()),
                attributes,
                ..Default::default()
            });
        }
        let batch = b.finish().unwrap();

        let storage = Storage {
            hot: Arc::new(LocalFileSystem::new_with_prefix(&hot).unwrap()),
            durable: None,
            hot_dir: Some(hot.clone()),
        };
        let wal = Arc::new(FakeWal {
            segments: Mutex::new(vec![(SegmentId(0), vec![batch])]),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = Compactor::new(wal, storage, replicator, schema.clone());
        while compactor.run_once().await.unwrap().is_some() {}

        let engine = QueryEngine::new(hot, schema).unwrap();
        let req = QueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 2000,
            services: Vec::new(),
            severities: Vec::new(),
            text: None,
            query: None,
            limit: 0,
        };
        let r = engine.facet("service.name", req, 10_000_000).await.unwrap();
        assert!(
            r.values.len() <= 1000,
            "limit must be clamped to MAX_LIMIT, got {}",
            r.values.len()
        );
        assert!(r.capped, "1200 distinct values exceeds the 1000 cap");
    }
}

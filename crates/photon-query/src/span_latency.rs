//! `latency`: duration distribution (percentiles + a log-scale (geometric) histogram) over the
//! FULL matched span set. Mirrors `crate::span_histogram` in spirit — bucketing/grouping/counting
//! happens in DataFusion, only the small grouped result is folded into fixed buckets in Rust —
//! but the bucket domain here is `duration_nanos` rather than time, and percentiles are computed
//! with DataFusion's `approx_percentile_cont` (t-digest) aggregate.
use arrow::array::{Array, Int64Array};
use arrow::datatypes::DataType;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::{approx_percentile_cont, count, max, min};
use datafusion::prelude::{cast, floor, lit, ln, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// One duration bucket: `bucket_ns` is the bucket's lower-bound duration (nanoseconds).
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyBucket {
    pub bucket_ns: i64,
    pub count: u64,
}

/// A duration distribution: a log-scale (geometric) histogram plus the p50/p90/p99 (nanoseconds,
/// rounded).
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyHistogram {
    pub buckets: Vec<LatencyBucket>,
    pub p50: i64,
    pub p90: i64,
    pub p99: i64,
}

impl SpanQueryEngine {
    /// Duration distribution over spans matching `req`: `buckets` geometric (log-scale) bins over
    /// `[min(duration_nanos), max(duration_nanos)]` — bucket 0 starts at the smallest matched
    /// duration and each subsequent bucket is a fixed ratio wider, so one slow outlier no longer
    /// stretches the domain until every other span collapses into bucket 0 — plus p50/p90/p99
    /// computed with the approximate t-digest aggregate over the same matched set. `duration_nanos`
    /// is nullable (a span that never closed has none); nulls are excluded from both the
    /// percentiles and the bucket counts — DataFusion's aggregates already skip nulls, so only the
    /// bucket GROUP BY needs an explicit `IS NOT NULL` filter (a NULL bucket expression would
    /// otherwise form its own group).
    pub async fn latency(
        &self,
        req: SpanQueryRequest,
        buckets: usize,
    ) -> Result<LatencyHistogram, PhotonError> {
        // Defense-in-depth: this is THE [P1 · DoS] repro (`GET /api/traces/latency?buckets=…`) —
        // a huge `buckets` here would otherwise drive `vec![0u64; buckets]` below to a
        // multi-gigabyte allocation. Mirrors `photon-api`'s `MAX_BUCKETS`
        // (`crates/photon-api/src/query_params.rs`); `photon-query` can't depend on `photon-api`,
        // so the value is restated here as a literal.
        let buckets = buckets.clamp(1, 3000);
        let df = match self.span_survivors_df(&req).await? {
            None => return Ok(empty_latency_histogram()),
            Some(df) => df,
        };
        latency_over(df, span_base_predicate(&req), buckets).await
    }
}

fn empty_latency_histogram() -> LatencyHistogram {
    LatencyHistogram {
        buckets: Vec::new(),
        p50: 0,
        p90: 0,
        p99: 0,
    }
}

/// The lower-bound duration (ns) of geometric bucket `i` of `buckets` spanning `[lo, hi]`, where
/// `log_span = ln(hi/lo)`. Bucket 0 starts at `lo`; each subsequent bucket is a fixed ratio wider.
fn log_bucket_start(lo: f64, log_span: f64, buckets: usize, i: usize) -> i64 {
    (lo * (i as f64 * log_span / buckets as f64).exp()).round() as i64
}

/// First non-empty-batch scalar of an Int64 column, or `None` if every row is null / there are
/// no rows at all. Global (no-GROUP-BY) aggregates always yield exactly one output row, so this
/// only ever needs to look at the first row of the first non-empty batch.
fn first_i64(
    batches: &[arrow::record_batch::RecordBatch],
    col: usize,
) -> Result<Option<i64>, PhotonError> {
    for b in batches {
        if b.num_rows() == 0 {
            continue;
        }
        let arr = b
            .column(col)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| PhotonError::Query("latency stats column not Int64".into()))?;
        return Ok(if arr.is_null(0) {
            None
        } else {
            Some(arr.value(0))
        });
    }
    Ok(None)
}

async fn latency_over(
    df: DataFrame,
    predicate: Expr,
    buckets: usize,
) -> Result<LatencyHistogram, PhotonError> {
    let filtered = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("latency filter: {e}")))?;
    let dur = col_ref(span_schema::DURATION);

    // One global aggregate for max(duration)/min(duration) (sizes the geometric bins) and the
    // three percentiles. `approx_percentile_cont`'s return type mirrors its input type (Int64
    // here), so all Int64 columns come back Int64 and rounding to i64 nanoseconds is already done
    // internally.
    let stats = filtered
        .clone()
        .aggregate(
            vec![],
            vec![
                max(dur.clone()).alias("max_dur"),
                approx_percentile_cont(dur.clone(), lit(0.5_f64), None).alias("p50"),
                approx_percentile_cont(dur.clone(), lit(0.9_f64), None).alias("p90"),
                approx_percentile_cont(dur.clone(), lit(0.99_f64), None).alias("p99"),
                min(dur.clone()).alias("min_dur"),
            ],
        )
        .map_err(|e| PhotonError::Query(format!("latency stats aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("latency stats collect: {e}")))?;

    let max_duration = first_i64(&stats, 0)?.unwrap_or(0);
    if max_duration <= 0 {
        // No spans with a positive duration matched (including no matches at all): there is no
        // meaningful bucket domain, so report an empty histogram with zeroed percentiles rather
        // than a single degenerate [0,0) bucket.
        return Ok(empty_latency_histogram());
    }
    let p50 = first_i64(&stats, 1)?.unwrap_or(0);
    let p90 = first_i64(&stats, 2)?.unwrap_or(0);
    let p99 = first_i64(&stats, 3)?.unwrap_or(0);
    let min_duration = first_i64(&stats, 4)?.unwrap_or(0);

    // Log-scale (geometric) bins over [lo, hi]. Latency is long-tailed, so linear bins collapse ~all
    // spans into bucket 0 while one slow outlier stretches the domain 10x past where the data lives.
    // `lo` is the smallest matched duration, floored to 1ns so ln() is finite and the log axis has a
    // positive origin; durations <= lo fold into bucket 0. `hi` is forced strictly above `lo` so the
    // log span is positive even when every matched span shares one duration.
    let lo = min_duration.max(1);
    let lo_f = lo as f64;
    let hi_f = (max_duration as f64).max(lo_f * (1.0 + 1e-9));
    let log_span = (hi_f / lo_f).ln(); // > 0

    // bucket(d) = floor( buckets * ln(d/lo) / log_span ), clamped to [0, buckets-1]; d <= lo -> 0.
    // Compute in Float64 (every CASE arm Float64 so the result type is stable), then cast ONCE to
    // Int64 — a mixed Int64/Float64 CASE would coerce the GROUP BY key to Float64 and break the
    // Int64 downcast.
    let dur_f = dur.clone() * lit(1.0_f64); // promote Int64 -> Float64 without an explicit cast import
    let raw_f = floor(lit(buckets as f64) * ln(dur_f / lit(lo_f)) / lit(log_span));
    let bucket_f = when(dur.clone().lt_eq(lit(lo)), lit(0.0_f64))
        .when(
            raw_f.clone().gt_eq(lit(buckets as f64)),
            lit(buckets as f64 - 1.0),
        )
        .otherwise(raw_f)
        .map_err(|e| PhotonError::Query(format!("latency bucket case: {e}")))?;
    let bucket = cast(bucket_f, DataType::Int64);

    let bucket_batches = filtered
        .filter(dur.clone().is_not_null())
        .map_err(|e| PhotonError::Query(format!("latency not-null filter: {e}")))?
        .aggregate(
            vec![bucket.alias("bucket")],
            vec![count(lit(1i64)).alias("n")],
        )
        .map_err(|e| PhotonError::Query(format!("latency bucket aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("latency bucket collect: {e}")))?;

    let mut counts = vec![0u64; buckets];
    for b in &bucket_batches {
        let bucket_col = b
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| PhotonError::Query("latency bucket column not Int64".into()))?;
        let n_col = b
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| PhotonError::Query("latency count column not Int64".into()))?;
        for i in 0..b.num_rows() {
            let idx = bucket_col.value(i).clamp(0, buckets as i64 - 1) as usize;
            counts[idx] += n_col.value(i).max(0) as u64;
        }
    }

    let out_buckets = (0..buckets)
        .map(|i| LatencyBucket {
            bucket_ns: log_bucket_start(lo_f, log_span, buckets, i),
            count: counts[i],
        })
        .collect();

    Ok(LatencyHistogram {
        buckets: out_buckets,
        p50,
        p90,
        p99,
    })
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

    fn span(start: i64, duration: Option<i64>) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), "api".to_string());
        SpanRecord {
            trace_id: "t1".into(),
            span_id: format!("s{start}"),
            name: Some("op".into()),
            start_time_nanos: start,
            duration_nanos: duration,
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
    async fn percentiles_are_monotone_and_buckets_sum_to_matched_count() {
        let records: Vec<SpanRecord> = (1..=100)
            .map(|i| span(i, Some(i * 1_000_000))) // 1ms .. 100ms
            .collect();
        let matched = records.len() as u64;
        let df = df_of(&records).await;
        let out = latency_over(df, lit_true(), 10).await.unwrap();

        assert!(out.p50 <= out.p90);
        assert!(out.p90 <= out.p99);
        assert!(out.p50 > 0);

        let bucket_total: u64 = out.buckets.iter().map(|b| b.count).sum();
        assert_eq!(bucket_total, matched);
        assert_eq!(out.buckets.len(), 10);
        // Bucket starts are non-decreasing and bucket 0 begins at the smallest matched duration
        // (1ms), not 0 — geometric bins are anchored at `lo`, not the origin.
        assert_eq!(out.buckets[0].bucket_ns, 1_000_000);
        for w in out.buckets.windows(2) {
            assert!(w[0].bucket_ns <= w[1].bucket_ns);
        }
    }

    #[tokio::test]
    async fn long_tail_does_not_collapse_into_bucket_zero() {
        // 90 spans at 1ms + 10 spans at 1s: with the old linear bucketing every 1ms span would
        // collapse into bucket 0 because the 1s outliers stretch the domain 1000x past where
        // almost all the data lives. Geometric bins keep the 1ms cluster and the 1s outliers in
        // clearly separated buckets.
        let mut records: Vec<SpanRecord> = (0..90).map(|i| span(i, Some(1_000_000))).collect();
        records.extend((0..10).map(|i| span(90 + i, Some(1_000_000_000))));
        let df = df_of(&records).await;
        let out = latency_over(df, lit_true(), 10).await.unwrap();

        let bucket_total: u64 = out.buckets.iter().map(|b| b.count).sum();
        assert_eq!(bucket_total, 100);
        assert_eq!(out.buckets[0].count, 90);
        assert!(
            out.buckets[0].count < 100,
            "everything collapsed into bucket 0"
        );
        assert_eq!(out.buckets[9].count, 10);
        assert!(
            out.buckets[9].bucket_ns >= 400_000_000,
            "last bucket should start in the hundreds-of-ms range, got {}",
            out.buckets[9].bucket_ns
        );
    }

    #[tokio::test]
    async fn null_durations_are_excluded_from_buckets_and_percentiles() {
        let records = vec![
            span(1, Some(10_000_000)),
            span(2, Some(20_000_000)),
            span(3, None), // never closed — excluded
        ];
        let df = df_of(&records).await;
        let out = latency_over(df, lit_true(), 4).await.unwrap();
        let bucket_total: u64 = out.buckets.iter().map(|b| b.count).sum();
        assert_eq!(bucket_total, 2);
        assert!(out.p99 > 0);
    }

    #[tokio::test]
    async fn empty_match_yields_empty_buckets_and_zero_percentiles() {
        let df = df_of(&[]).await;
        let out = latency_over(df, lit_true(), 10).await.unwrap();
        assert!(out.buckets.is_empty());
        assert_eq!(out.p50, 0);
        assert_eq!(out.p90, 0);
        assert_eq!(out.p99, 0);
    }

    #[tokio::test]
    async fn all_durations_null_yields_empty_buckets_and_zero_percentiles() {
        let records = vec![span(1, None), span(2, None)];
        let df = df_of(&records).await;
        let out = latency_over(df, lit_true(), 10).await.unwrap();
        assert!(out.buckets.is_empty());
        assert_eq!(out.p50, 0);
        assert_eq!(out.p90, 0);
        assert_eq!(out.p99, 0);
    }
}

/// End-to-end coverage of the public `SpanQueryEngine::latency` entry point — the literal
/// `[P1 · DoS]` repro (`GET /api/traces/latency?buckets=2000000000`) — against a real compacted
/// span (not just the private `latency_over` helper, which the defensive clamp sits above). A
/// span with a positive duration is required: on an empty store `latency` short-circuits to an
/// empty histogram before the bucket allocation ever runs, which would hide a broken clamp.
#[cfg(test)]
mod engine_tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use arrow::record_batch::RecordBatch;
    use object_store::local::LocalFileSystem;
    use photon_compact::SpanCompactor;
    use photon_core::segment::SegmentId;
    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;
    use photon_core::PhotonError;
    use photon_storage::{Replicator, Storage};
    use photon_wal::Wal;

    use crate::{SpanQueryEngine, SpanQueryRequest, SpanSort};

    /// Minimal in-memory WAL handing the compactor one pre-built segment, mirroring the
    /// `FakeWal` fixtures in `infra.rs`/`metric_classic_hist.rs`.
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
    async fn engine_method_clamps_a_dos_sized_bucket_count() {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = SpanSchema::new(&["service.name".to_string()]);

        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), "api".to_string());
        let mut b = SpanBatchBuilder::new(&schema);
        b.append(&SpanRecord {
            trace_id: "t1".into(),
            span_id: "s1".into(),
            name: Some("op".into()),
            start_time_nanos: 10,
            duration_nanos: Some(5_000_000),
            attributes,
            ..Default::default()
        });
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
        let compactor = SpanCompactor::new(wal, storage, replicator, schema.clone());
        while compactor.run_once().await.unwrap().is_some() {}

        let engine = SpanQueryEngine::new(hot, schema).unwrap();
        let req = SpanQueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 1_000_000_000,
            query: None,
            sort: SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
        };
        let out = engine.latency(req, 10_000_000).await.unwrap();
        assert!(
            out.buckets.len() <= 3000,
            "buckets must be clamped to MAX_BUCKETS, got {}",
            out.buckets.len()
        );
    }
}

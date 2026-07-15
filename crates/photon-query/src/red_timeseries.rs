//! `red_timeseries`: per-service bucketed RED (Rate, Errors, Duration) + Apdex band counts over
//! time — powers the Services (APM) detail-page charts. Mirrors `crate::span_histogram`'s
//! time-bucketing (bucket width, index clamping, zero-filling empty buckets over
//! `span_schema::START_TIME`) and `crate::red`'s error/band flag construction, but groups only by
//! bucket index for a single caller-scoped service, using a single literal Apdex threshold (no
//! per-service CASE — the caller passes one `threshold_ms`).
use arrow::array::{Array, Int64Array};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::{approx_percentile_cont, count, sum};
use datafusion::prelude::{cast, lit, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// One time bucket of RED metrics + Apdex band counts for a single service's spans. `ts` is the
/// bucket-start timestamp (epoch nanos). Apdex itself is NOT computed here — only the raw
/// `satisfied`/`tolerating`/`frustrated` counts; the API layer derives Apdex from them.
#[derive(Debug, Clone, PartialEq)]
pub struct RedBucket {
    pub ts: i64,
    pub count: u64,
    pub error_count: u64,
    pub p50: i64,
    pub p90: i64,
    pub p99: i64,
    pub satisfied: u64,
    pub tolerating: u64,
    pub frustrated: u64,
}

impl SpanQueryEngine {
    /// `buckets` equal-width time buckets over `[start, end]` (by `start_time_nanos`) of RED +
    /// Apdex-band counts. `req` should already be scoped to one service (e.g. a
    /// `service.name:<svc>` grammar query) — this engine only buckets by time, it does not group
    /// by service. `threshold_ms` is a single per-service Apdex threshold.
    ///
    /// Returns a zero-filled series of length `buckets` (so the chart keeps an x-axis) when
    /// nothing survives pruning.
    pub async fn red_timeseries(
        &self,
        req: SpanQueryRequest,
        start: i64,
        end: i64,
        buckets: usize,
        threshold_ms: u32,
    ) -> Result<Vec<RedBucket>, PhotonError> {
        // Defense-in-depth: mirrors `photon-api`'s `MAX_BUCKETS`
        // (`crates/photon-api/src/query_params.rs`); `photon-query` can't depend on `photon-api`,
        // so the value is restated here as a literal.
        let buckets = buckets.clamp(1, 3000);
        match self.span_survivors_df(&req).await? {
            None => Ok(empty_buckets(start, end, buckets)),
            Some(df) => {
                red_over_time(
                    df,
                    span_base_predicate(&req),
                    start,
                    end,
                    buckets,
                    threshold_ms,
                )
                .await
            }
        }
    }
}

/// The width (nanos) of each of `buckets` equal-width buckets spanning `[start, end]`. Floored to
/// 1 so a degenerate `end <= start` or `buckets` window never divides by zero.
fn bucket_width(start: i64, end: i64, buckets: usize) -> i64 {
    ((end - start).max(1) / buckets as i64).max(1)
}

/// The start timestamp (epoch nanos) of bucket `i`, `width` nanos wide, starting at `start`.
fn bucket_ts(start: i64, width: i64, i: usize) -> i64 {
    start + i as i64 * width
}

fn empty_buckets(start: i64, end: i64, buckets: usize) -> Vec<RedBucket> {
    let width = bucket_width(start, end, buckets);
    (0..buckets)
        .map(|i| RedBucket {
            ts: bucket_ts(start, width, i),
            count: 0,
            error_count: 0,
            p50: 0,
            p90: 0,
            p99: 0,
            satisfied: 0,
            tolerating: 0,
            frustrated: 0,
        })
        .collect()
}

/// Filter → per-bucket grouped aggregate (count, error count, p50/p90/p99, Apdex band counts) →
/// decode into a zero-filled `Vec<RedBucket>` indexed by bucket. Split out from `red_timeseries`
/// so unit tests can drive it directly over a `MemTable` DataFrame.
async fn red_over_time(
    df: DataFrame,
    predicate: Expr,
    start: i64,
    end: i64,
    buckets: usize,
    threshold_ms: u32,
) -> Result<Vec<RedBucket>, PhotonError> {
    let width = bucket_width(start, end, buckets);

    // bucket = (start_time_nanos - start) / width, integer division. Rows satisfying the
    // predicate's `start_time_nanos BETWEEN start AND end` land in `[0, buckets]`; a row at
    // exactly `end` can map to `buckets`, clamped down to the last bucket (mirrors
    // `span_histogram::histogram_over`).
    let start_time = cast(col_ref(span_schema::START_TIME), DataType::Int64);
    let raw = (start_time - lit(start)) / lit(width);
    let bucket = when(
        raw.clone().gt_eq(lit(buckets as i64)),
        lit(buckets as i64 - 1),
    )
    .otherwise(raw)
    .map_err(|e| PhotonError::Query(format!("red_timeseries bucket case: {e}")))?;

    // 0/1 error flag per span (ERROR == OTEL status_code 2), summed per bucket below.
    let error_flag = when(col_ref(span_schema::STATUS_CODE).eq(lit(2_i32)), lit(1_i64))
        .otherwise(lit(0_i64))
        .map_err(|e| PhotonError::Query(format!("red_timeseries error-flag case: {e}")))?;

    // Single literal Apdex threshold — no per-service CASE (the caller already scoped `req` to
    // one service and passes that service's threshold).
    const MS: i64 = 1_000_000;
    let t_ns = lit(threshold_ms as i64 * MS);
    let four_t = lit(threshold_ms as i64 * 4 * MS);
    let dur = col_ref(span_schema::DURATION);
    let satisfied_flag = when(dur.clone().lt_eq(t_ns.clone()), lit(1_i64))
        .otherwise(lit(0_i64))
        .map_err(|e| PhotonError::Query(format!("red_timeseries satisfied case: {e}")))?;
    let tolerating_flag = when(
        dur.clone().gt(t_ns).and(dur.clone().lt_eq(four_t.clone())),
        lit(1_i64),
    )
    .otherwise(lit(0_i64))
    .map_err(|e| PhotonError::Query(format!("red_timeseries tolerating case: {e}")))?;
    let frustrated_flag = when(dur.clone().gt(four_t), lit(1_i64))
        .otherwise(lit(0_i64))
        .map_err(|e| PhotonError::Query(format!("red_timeseries frustrated case: {e}")))?;

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("red_timeseries filter: {e}")))?
        .aggregate(
            vec![bucket.alias("bucket")],
            vec![
                count(lit(1_i64)).alias("n"),
                sum(error_flag).alias("errors"),
                approx_percentile_cont(dur.clone(), lit(0.5_f64), None).alias("p50"),
                approx_percentile_cont(dur.clone(), lit(0.9_f64), None).alias("p90"),
                approx_percentile_cont(dur.clone(), lit(0.99_f64), None).alias("p99"),
                sum(satisfied_flag).alias("sat"),
                sum(tolerating_flag).alias("tol"),
                sum(frustrated_flag).alias("fru"),
            ],
        )
        .map_err(|e| PhotonError::Query(format!("red_timeseries aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("red_timeseries collect: {e}")))?;

    let mut out = empty_buckets(start, end, buckets);
    for b in &batches {
        let bucket_col = i64_col(b, "bucket")?;
        let n = i64_col(b, "n")?;
        let errors = i64_col(b, "errors")?;
        let p50 = i64_col(b, "p50")?;
        let p90 = i64_col(b, "p90")?;
        let p99 = i64_col(b, "p99")?;
        let sat = i64_col(b, "sat")?;
        let tol = i64_col(b, "tol")?;
        let fru = i64_col(b, "fru")?;
        for i in 0..b.num_rows() {
            if bucket_col.is_null(i) {
                continue; // start_time_nanos is a required column; a null group can't occur
            }
            let idx = bucket_col.value(i).clamp(0, buckets as i64 - 1) as usize;
            let rb = &mut out[idx];
            rb.count += nonneg_u64(n, i);
            rb.error_count += nonneg_u64(errors, i);
            rb.p50 = opt_i64(p50, i);
            rb.p90 = opt_i64(p90, i);
            rb.p99 = opt_i64(p99, i);
            rb.satisfied += nonneg_u64(sat, i);
            rb.tolerating += nonneg_u64(tol, i);
            rb.frustrated += nonneg_u64(fru, i);
        }
    }
    Ok(out)
}

fn i64_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| {
            PhotonError::Query(format!(
                "red_timeseries column `{name}` missing or not Int64"
            ))
        })
}

/// A non-null, non-negative count cell as `u64` (null / negative → 0).
fn nonneg_u64(col: &Int64Array, i: usize) -> u64 {
    if col.is_null(i) {
        0
    } else {
        col.value(i).max(0) as u64
    }
}

/// A nullable percentile cell as `i64` nanoseconds (null → 0).
fn opt_i64(col: &Int64Array, i: usize) -> i64 {
    if col.is_null(i) {
        0
    } else {
        col.value(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    use photon_core::query::{parse, SpanFieldResolver};
    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;

    use crate::SpanSort;

    fn schema() -> SpanSchema {
        SpanSchema::new(&["service.name".into()])
    }

    /// A span with an explicit `start_time_nanos`, for time-bucketing tests.
    fn span_at(
        service: &str,
        name: &str,
        start_ns: i64,
        dur: Option<i64>,
        status: Option<i32>,
    ) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        SpanRecord {
            trace_id: "t1".into(),
            span_id: format!("{service}-{name}-{start_ns}"),
            name: Some(name.into()),
            start_time_nanos: start_ns,
            duration_nanos: dur,
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

    /// A `SpanQueryRequest` over `[start, end]` with `query` resolved from the grammar `query`
    /// string (mirrors `crate::red::tests::req`, parameterized).
    fn req_for(query: &str, start: i64, end: i64) -> SpanQueryRequest {
        SpanQueryRequest {
            start_ts_nanos: start,
            end_ts_nanos: end,
            query: Some(
                SpanFieldResolver::new(&["service.name".to_string()])
                    .resolve(&parse(query).unwrap())
                    .unwrap(),
            ),
            sort: SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
        }
    }

    #[tokio::test]
    async fn buckets_spans_by_time_with_bands() {
        // window 0..200ns, 2 buckets (0..100, 100..200). Use start_time_nanos to place spans.
        let records = vec![
            span_at("svc", "op", 10, Some(100_000_000), Some(1)), // bucket 0, satisfied
            span_at("svc", "op", 20, Some(900_000_000), Some(2)), // bucket 0, error, frustrated
            span_at("svc", "op", 150, Some(300_000_000), Some(1)), // bucket 1
        ];
        let out = red_over_time(
            df_of(&records).await,
            span_base_predicate(&req_for("service.name:svc", 0, 200)),
            0,
            200,
            2,
            500,
        )
        .await
        .unwrap();

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].ts, 0);
        assert_eq!(out[0].count, 2);
        assert_eq!(out[0].error_count, 1);
        assert_eq!(out[1].ts, 100);
        assert_eq!(out[1].count, 1);
    }

    #[tokio::test]
    async fn bands_split_satisfied_tolerating_frustrated_at_threshold() {
        // threshold_ms = 500 -> T = 500ms, 4T = 2000ms.
        let records = vec![
            span_at("svc", "op", 10, Some(100_000_000), Some(1)), // 100ms <= T -> satisfied
            span_at("svc", "op", 10, Some(900_000_000), Some(1)), // T < 900ms <= 4T -> tolerating
            span_at("svc", "op", 10, Some(3_000_000_000), Some(1)), // > 4T -> frustrated
        ];
        let out = red_over_time(
            df_of(&records).await,
            span_base_predicate(&req_for("service.name:svc", 0, 100)),
            0,
            100,
            1,
            500,
        )
        .await
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].count, 3);
        assert_eq!(
            (out[0].satisfied, out[0].tolerating, out[0].frustrated),
            (1, 1, 1)
        );
    }

    #[tokio::test]
    async fn null_duration_counts_but_lands_in_no_band() {
        let records = vec![span_at("svc", "op", 10, None, Some(1))];
        let out = red_over_time(
            df_of(&records).await,
            span_base_predicate(&req_for("service.name:svc", 0, 100)),
            0,
            100,
            1,
            500,
        )
        .await
        .unwrap();
        assert_eq!(out[0].count, 1);
        assert_eq!(
            (out[0].satisfied, out[0].tolerating, out[0].frustrated),
            (0, 0, 0)
        );
    }

    #[tokio::test]
    async fn empty_window_yields_zero_filled_buckets_with_x_axis() {
        let df = df_of(&[]).await;
        let out = red_over_time(
            df,
            span_base_predicate(&req_for("service.name:svc", 0, 100)),
            0,
            100,
            4,
            500,
        )
        .await
        .unwrap();
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|b| b.count == 0));
        assert_eq!(
            out.iter().map(|b| b.ts).collect::<Vec<_>>(),
            vec![0, 25, 50, 75]
        );
    }

    #[tokio::test]
    async fn engine_method_clamps_a_dos_sized_bucket_count() {
        // Defense-in-depth for the public `SpanQueryEngine::red_timeseries` entry point itself
        // (powers the Services detail-page charts, e.g. `GET /api/services/:service/timeseries`).
        let dir = tempfile::tempdir().unwrap();
        let engine = SpanQueryEngine::new(dir.path().to_path_buf(), SpanSchema::new(&[])).unwrap();
        let req = SpanQueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: 100,
            query: None,
            sort: SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
        };
        let out = engine
            .red_timeseries(req, 0, 100, 10_000_000, 500)
            .await
            .unwrap();
        assert_eq!(out.len(), 3000, "buckets must be clamped to MAX_BUCKETS");
    }
}

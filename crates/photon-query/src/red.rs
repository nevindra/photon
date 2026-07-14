//! `red_metrics`: RED (Rate, Errors, Duration) per (service.name[, operation]) over the matched
//! span set. Mirrors `crate::span_latency` (percentiles via `approx_percentile_cont`) and
//! `crate::span_facet` (grouped aggregate over `span_survivors_df` + `span_base_predicate`), but
//! groups by service — and, for `RedGroup::Operation`, operation `name` — and additionally counts
//! errored spans (`status_code == 2`, OTEL ERROR).
//!
//! v1 is breadth-first / correctness-over-speed, like `search_traces`: the aggregate is recomputed
//! per query over the pruned candidate set (no precomputed rollups — the flagged optimization
//! target). Group cardinality (service × operation) is naturally bounded, but a `MAX_RED_GROUPS`
//! cap (ordered by count desc, logged — never silent) guards a pathological high-cardinality name.
use std::collections::HashMap;

use arrow::array::{Array, Int64Array, StringArray};
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::{approx_percentile_cont, count, sum};
use datafusion::prelude::{col, lit, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// How RED rows are grouped: per operation (service × operation `name`) or rolled up per service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedGroup {
    /// GROUP BY (service.name, name) — one row per operation.
    #[default]
    Operation,
    /// GROUP BY (service.name) — one row per service; `operation` is `None`.
    Service,
}

/// One RED row: identity + raw counts + duration percentiles (nanoseconds). Rate and error-rate
/// are DERIVED in the API handler from `count`/`error_count` and the query window — the engine
/// stays window-agnostic beyond the pruning it already applied.
#[derive(Debug, Clone, PartialEq)]
pub struct RedRow {
    /// `service.name`.
    pub service: String,
    /// Operation `name`; `Some` only for `RedGroup::Operation`, `None` for `RedGroup::Service`.
    pub operation: Option<String>,
    /// Spans matched in the group (the rate numerator).
    pub count: u64,
    /// Spans with `status_code == 2` (OTEL ERROR) in the group.
    pub error_count: u64,
    /// p50/p90/p99 of `duration_nanos` over the group (t-digest approx). 0 when the group has no
    /// non-null duration (e.g. every span is still open).
    pub p50: i64,
    pub p90: i64,
    pub p99: i64,
    /// Spans whose duration ≤ the service's Apdex T (satisfied). Null-duration spans count in none.
    pub satisfied: u64,
    /// Spans with T < duration ≤ 4T (tolerating).
    pub tolerating: u64,
    /// Spans with duration > 4T (frustrated).
    pub frustrated: u64,
}

/// Cap on RED rows returned. Ordered by `count` DESC before the cap so the busiest groups survive;
/// a capped result is logged (never silent), matching `search_traces`'s cap convention.
const MAX_RED_GROUPS: usize = 1000;

impl SpanQueryEngine {
    /// RED metrics grouped per `group` over spans matching `req`. Empty vec when nothing survives
    /// pruning / matches the predicate.
    pub async fn red_metrics(
        &self,
        req: SpanQueryRequest,
        group: RedGroup,
        thresholds: &HashMap<String, u32>,
        default_ms: u32,
    ) -> Result<Vec<RedRow>, PhotonError> {
        match self.span_survivors_df(&req).await? {
            None => Ok(Vec::new()),
            Some(df) => {
                red_over(df, span_base_predicate(&req), group, thresholds, default_ms).await
            }
        }
    }
}

/// Per-row Apdex threshold `T` in nanoseconds as a DataFusion expression: a CASE keyed on
/// `service.name`, falling back to `default_ms`. Group cardinality is bounded (≤ MAX_RED_GROUPS),
/// so an N-branch CASE is cheap and avoids a join.
fn threshold_ns_expr(thresholds: &HashMap<String, u32>, default_ms: u32) -> Expr {
    const MS: i64 = 1_000_000;
    let mut e = lit(default_ms as i64 * MS);
    for (svc, ms) in thresholds {
        e = when(
            col_ref("service.name").eq(lit(svc.clone())),
            lit(*ms as i64 * MS),
        )
        .otherwise(e)
        .expect("nested CASE for apdex threshold");
    }
    e
}

/// Filter → grouped aggregate (count, error count, p50/p90/p99) → order by count desc → cap →
/// decode. Split out from `red_metrics` so the unit tests can drive it over a `MemTable` DataFrame.
async fn red_over(
    df: DataFrame,
    predicate: Expr,
    group: RedGroup,
    thresholds: &HashMap<String, u32>,
    default_ms: u32,
) -> Result<Vec<RedRow>, PhotonError> {
    // 0/1 error flag per span (ERROR == OTEL status_code 2), summed per group below.
    let error_flag = when(col_ref(span_schema::STATUS_CODE).eq(lit(2_i32)), lit(1_i64))
        .otherwise(lit(0_i64))
        .map_err(|e| PhotonError::Query(format!("red error-flag case: {e}")))?;

    let t_ns = threshold_ns_expr(thresholds, default_ms);
    let four_t = t_ns.clone() * lit(4_i64);
    let dur = col_ref(span_schema::DURATION);

    let satisfied_flag = when(dur.clone().lt_eq(t_ns.clone()), lit(1_i64))
        .otherwise(lit(0_i64))
        .map_err(|e| PhotonError::Query(format!("red satisfied case: {e}")))?;
    let tolerating_flag = when(
        dur.clone().gt(t_ns).and(dur.clone().lt_eq(four_t.clone())),
        lit(1_i64),
    )
    .otherwise(lit(0_i64))
    .map_err(|e| PhotonError::Query(format!("red tolerating case: {e}")))?;
    let frustrated_flag = when(dur.clone().gt(four_t), lit(1_i64))
        .otherwise(lit(0_i64))
        .map_err(|e| PhotonError::Query(format!("red frustrated case: {e}")))?;

    let group_exprs: Vec<Expr> = match group {
        RedGroup::Operation => vec![
            col_ref("service.name").alias("service"),
            col_ref(span_schema::NAME).alias("operation"),
        ],
        RedGroup::Service => vec![col_ref("service.name").alias("service")],
    };

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("red filter: {e}")))?
        .aggregate(
            group_exprs,
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
        .map_err(|e| PhotonError::Query(format!("red aggregate: {e}")))?
        .sort(vec![
            col("n").sort(false, false), // count desc — keep the busiest groups under the cap
            col("service").sort(true, false), // stable tiebreak
        ])
        .map_err(|e| PhotonError::Query(format!("red sort: {e}")))?
        .limit(0, Some(MAX_RED_GROUPS + 1))
        .map_err(|e| PhotonError::Query(format!("red limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("red collect: {e}")))?;

    let mut rows: Vec<RedRow> = Vec::new();
    for b in &batches {
        let service = str_col(b, "service")?;
        let operation = match group {
            RedGroup::Operation => Some(str_col(b, "operation")?),
            RedGroup::Service => None,
        };
        let n = i64_col(b, "n")?;
        let errors = i64_col(b, "errors")?;
        let p50 = i64_col(b, "p50")?;
        let p90 = i64_col(b, "p90")?;
        let p99 = i64_col(b, "p99")?;
        let sat = i64_col(b, "sat")?;
        let tol = i64_col(b, "tol")?;
        let fru = i64_col(b, "fru")?;
        for i in 0..b.num_rows() {
            if service.is_null(i) {
                continue; // service.name is promoted+required; a null group is not a real service
            }
            rows.push(RedRow {
                service: service.value(i).to_string(),
                operation: operation.and_then(|o| {
                    if o.is_null(i) {
                        None
                    } else {
                        Some(o.value(i).to_string())
                    }
                }),
                count: nonneg_u64(n, i),
                error_count: nonneg_u64(errors, i),
                p50: opt_i64(p50, i),
                p90: opt_i64(p90, i),
                p99: opt_i64(p99, i),
                satisfied: nonneg_u64(sat, i),
                tolerating: nonneg_u64(tol, i),
                frustrated: nonneg_u64(fru, i),
            });
        }
    }

    if rows.len() > MAX_RED_GROUPS {
        eprintln!(
            "photon-query: red_metrics returning only the top {MAX_RED_GROUPS} of {} groups by \
             count (v1 cap)",
            rows.len()
        );
        rows.truncate(MAX_RED_GROUPS);
    }
    Ok(rows)
}

fn str_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a StringArray, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| PhotonError::Query(format!("red column `{name}` missing or not Utf8")))
}

fn i64_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| PhotonError::Query(format!("red column `{name}` missing or not Int64")))
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

    fn span(service: &str, name: &str, dur: Option<i64>, status: Option<i32>) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        SpanRecord {
            trace_id: "t1".into(),
            span_id: format!("{service}-{name}-{}", dur.unwrap_or(0)),
            name: Some(name.into()),
            start_time_nanos: 1,
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
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
        }
    }

    #[tokio::test]
    async fn groups_by_operation_with_counts_errors_and_percentiles() {
        // checkout/charge: 3 spans, 1 error, durations 100/200/300ms.
        // web/home: 1 span, 0 error.
        let records = vec![
            span("checkout", "charge", Some(100_000_000), Some(1)),
            span("checkout", "charge", Some(200_000_000), Some(2)), // error
            span("checkout", "charge", Some(300_000_000), Some(1)),
            span("web", "home", Some(10_000_000), Some(1)),
        ];
        let rows = red_over(
            df_of(&records).await,
            span_base_predicate(&req()),
            RedGroup::Operation,
            &std::collections::HashMap::new(),
            500,
        )
        .await
        .unwrap();

        let charge = rows
            .iter()
            .find(|r| r.service == "checkout" && r.operation.as_deref() == Some("charge"))
            .expect("charge row present");
        assert_eq!(charge.count, 3);
        assert_eq!(charge.error_count, 1);
        assert!(charge.p50 > 0 && charge.p50 <= charge.p90 && charge.p90 <= charge.p99);

        let home = rows
            .iter()
            .find(|r| r.service == "web" && r.operation.as_deref() == Some("home"))
            .expect("home row present");
        assert_eq!(home.count, 1);
        assert_eq!(home.error_count, 0);
    }

    #[tokio::test]
    async fn service_group_rolls_operations_up_and_nulls_operation() {
        let records = vec![
            span("checkout", "charge", Some(100_000_000), Some(2)),
            span("checkout", "lookup", Some(50_000_000), Some(1)),
            span("web", "home", Some(10_000_000), Some(1)),
        ];
        let rows = red_over(
            df_of(&records).await,
            span_base_predicate(&req()),
            RedGroup::Service,
            &std::collections::HashMap::new(),
            500,
        )
        .await
        .unwrap();

        let checkout = rows.iter().find(|r| r.service == "checkout").unwrap();
        assert_eq!(checkout.operation, None);
        assert_eq!(checkout.count, 2);
        assert_eq!(checkout.error_count, 1);
        assert_eq!(rows.len(), 2); // checkout + web, no per-operation split
    }

    #[tokio::test]
    async fn all_null_durations_yield_zero_percentiles() {
        let records = vec![
            span("svc", "op", None, Some(1)),
            span("svc", "op", None, Some(2)),
        ];
        let rows = red_over(
            df_of(&records).await,
            span_base_predicate(&req()),
            RedGroup::Operation,
            &std::collections::HashMap::new(),
            500,
        )
        .await
        .unwrap();
        let row = &rows[0];
        assert_eq!(row.count, 2);
        assert_eq!(row.error_count, 1);
        assert_eq!((row.p50, row.p90, row.p99), (0, 0, 0));
    }

    #[tokio::test]
    async fn latency_bands_use_per_service_threshold() {
        use std::collections::HashMap;
        // checkout T=200ms: 150ms satisfied, 300ms tolerating (200..800), 900ms frustrated.
        // web uses the default T=500ms: 100ms satisfied.
        let records = vec![
            span("checkout", "op", Some(150_000_000), Some(1)),
            span("checkout", "op", Some(300_000_000), Some(1)),
            span("checkout", "op", Some(900_000_000), Some(1)),
            span("web", "home", Some(100_000_000), Some(1)),
        ];
        let mut thresholds = HashMap::new();
        thresholds.insert("checkout".to_string(), 200u32);

        let rows = red_over(
            df_of(&records).await,
            span_base_predicate(&req()),
            RedGroup::Service,
            &thresholds,
            500,
        )
        .await
        .unwrap();

        let checkout = rows.iter().find(|r| r.service == "checkout").unwrap();
        assert_eq!(
            (checkout.satisfied, checkout.tolerating, checkout.frustrated),
            (1, 1, 1)
        );

        let web = rows.iter().find(|r| r.service == "web").unwrap();
        assert_eq!((web.satisfied, web.tolerating, web.frustrated), (1, 0, 0));
    }
}

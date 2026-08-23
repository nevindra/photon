//! `red_timeseries`: per-service bucketed RED (Rate, Errors, Duration) + Apdex band counts over
//! time — powers the Services (APM) detail-page charts. Mirrors `crate::span_histogram`'s
//! time-bucketing (bucket width, index clamping, zero-filling empty buckets over
//! `span_schema::START_TIME`) and reuses `crate::red`'s entry-set cascade and per-candidate
//! aggregate block verbatim, but groups only by bucket index for a single caller-scoped service,
//! using a single literal Apdex threshold (no per-service CASE — the caller passes one
//! `threshold_ms`).
//!
//! Like `crate::red`, every number here is taken over the service's ENTRY spans (ADR-0003), with
//! the raw span total kept beside them as `RedBucket::span_count`. The one difference: the cascade
//! is resolved ONCE for the whole series rather than per bucket — see `red_over_time`.
use arrow::array::{Array, Int64Array};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::{cast, lit, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::red::{
    candidate_aggs, entry_mask, http_status_expr, root_mask, RedBasis, HTTP_STATUS_CODE,
    HTTP_STATUS_CODE_LEGACY,
};
use crate::span_engine::{span_base_predicate, AttrKeys};
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// One time bucket of RED metrics + Apdex band counts for a single service's spans. `ts` is the
/// bucket-start timestamp (epoch nanos). Apdex itself is NOT computed here — only the raw
/// `satisfied`/`tolerating`/`frustrated` counts; the API layer derives Apdex from them.
#[derive(Debug, Clone, PartialEq)]
pub struct RedBucket {
    pub ts: i64,
    /// Requests served in the bucket: spans in the series' ENTRY set (see `basis`).
    pub count: u64,
    /// EVERY span in the bucket, all kinds — the separate second number (ADR-0003).
    pub span_count: u64,
    /// Which cascade rule produced `count`. The same value on every bucket of one series.
    pub basis: RedBasis,
    pub error_count: u64,
    pub p50: i64,
    pub p90: i64,
    pub p99: i64,
    pub satisfied: u64,
    pub tolerating: u64,
    pub frustrated: u64,
    /// Entry spans in the bucket whose HTTP status attribute has a leading digit of `4`/`5`. Sits
    /// BESIDE `error_count`, not instead of it — see `RedRow::http_4xx`'s doc comment in
    /// `crate::red` for the full "why beside" reasoning; this is the same counter, bucketed.
    pub http_4xx: u64,
    pub http_5xx: u64,
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
        let promoted = self.promoted_attributes();
        let status_promoted = promoted.iter().any(|p| p == HTTP_STATUS_CODE);
        let status_legacy_promoted = promoted.iter().any(|p| p == HTTP_STATUS_CODE_LEGACY);
        match self.span_survivors(&req).await? {
            None => Ok(empty_buckets(start, end, buckets)),
            Some((df, attr_keys)) => {
                red_over_time(
                    df,
                    span_base_predicate(&req),
                    start,
                    end,
                    buckets,
                    threshold_ms,
                    status_promoted,
                    status_legacy_promoted,
                    &attr_keys,
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
            span_count: 0,
            // No spans, so no cascade rule fired and there is nothing to badge. `Entry` is the
            // "unremarkable" basis; `AllSpans` would tell the UI to flag a service that is simply
            // quiet.
            basis: RedBasis::Entry,
            error_count: 0,
            p50: 0,
            p90: 0,
            p99: 0,
            satisfied: 0,
            tolerating: 0,
            frustrated: 0,
            http_4xx: 0,
            http_5xx: 0,
        })
        .collect()
}

/// Filter → per-bucket grouped aggregate (count, error count, p50/p90/p99, Apdex band counts) →
/// decode into a zero-filled `Vec<RedBucket>` indexed by bucket. Split out from `red_timeseries`
/// so unit tests can drive it directly over a `MemTable` DataFrame.
#[allow(clippy::too_many_arguments)]
async fn red_over_time(
    df: DataFrame,
    predicate: Expr,
    start: i64,
    end: i64,
    buckets: usize,
    threshold_ms: u32,
    status_promoted: bool,
    status_legacy_promoted: bool,
    attr_keys: &AttrKeys,
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

    // Single literal Apdex threshold — no per-service CASE (the caller already scoped `req` to
    // one service and passes that service's threshold).
    const MS: i64 = 1_000_000;
    let t_ns = lit(threshold_ms as i64 * MS);
    let dur = col_ref(span_schema::DURATION);
    let http_status = http_status_expr(status_promoted, status_legacy_promoted, attr_keys);

    // ADR-0003's entry-set cascade, borrowed whole from `crate::red` so the table and the chart
    // can never disagree on what a request is: all three candidate sets are aggregated side by
    // side in this one pass, and the winner is chosen in Rust below.
    let mut aggs = candidate_aggs(None, &dur, &t_ns, &http_status, "")?;
    aggs.extend(candidate_aggs(
        Some(&entry_mask()),
        &dur,
        &t_ns,
        &http_status,
        "_e",
    )?);
    aggs.extend(candidate_aggs(
        Some(&root_mask()),
        &dur,
        &t_ns,
        &http_status,
        "_r",
    )?);

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("red_timeseries filter: {e}")))?
        .aggregate(vec![bucket.alias("bucket")], aggs)
        .map_err(|e| PhotonError::Query(format!("red_timeseries aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("red_timeseries collect: {e}")))?;

    let mut cands: Vec<BucketCandidates> = vec![BucketCandidates::default(); buckets];
    for b in &batches {
        let bucket_col = i64_col(b, "bucket")?;
        let all = CandidateCols::read(b, "")?;
        let entry = CandidateCols::read(b, "_e")?;
        let root = CandidateCols::read(b, "_r")?;
        for i in 0..b.num_rows() {
            if bucket_col.is_null(i) {
                continue; // start_time_nanos is a required column; a null group can't occur
            }
            let idx = bucket_col.value(i).clamp(0, buckets as i64 - 1) as usize;
            let c = &mut cands[idx];
            c.all.absorb(&all, i);
            c.entry.absorb(&entry, i);
            c.root.absorb(&root, i);
        }
    }

    // The cascade is resolved ONCE for the whole series, not per bucket. This engine is already
    // scoped to one service, and a per-bucket decision would let a quiet bucket that happens to
    // hold only INTERNAL spans fall back to rule 3 and spike the chart against neighbours measured
    // a different way. Deciding once means a bucket where no request entered honestly reads 0
    // requests (its spans are still there, in `span_count`).
    let sum_of = |pick: fn(&BucketCandidates) -> &Candidate| -> u64 {
        cands.iter().map(|c| pick(c).n).sum()
    };
    let basis = if sum_of(|c| &c.entry) > 0 {
        RedBasis::Entry
    } else if sum_of(|c| &c.root) > 0 {
        RedBasis::TraceRoot
    } else if sum_of(|c| &c.all) > 0 {
        RedBasis::AllSpans
    } else {
        RedBasis::Entry // nothing in the window; same reasoning as `empty_buckets`
    };

    let mut out = empty_buckets(start, end, buckets);
    for (rb, c) in out.iter_mut().zip(cands.iter()) {
        let won = match basis {
            RedBasis::Entry => &c.entry,
            RedBasis::TraceRoot => &c.root,
            RedBasis::AllSpans => &c.all,
        };
        rb.span_count = c.all.n;
        rb.basis = basis;
        rb.count = won.n;
        rb.error_count = won.errors;
        rb.p50 = won.p50;
        rb.p90 = won.p90;
        rb.p99 = won.p99;
        rb.satisfied = won.sat;
        rb.tolerating = won.tol;
        rb.frustrated = won.fru;
        rb.http_4xx = won.h4;
        rb.http_5xx = won.h5;
    }
    Ok(out)
}

/// One bucket's three candidate entry sets, held until the series-wide cascade picks a winner.
#[derive(Default, Clone)]
struct BucketCandidates {
    all: Candidate,
    entry: Candidate,
    root: Candidate,
}

/// One candidate entry set's decoded numbers for one bucket.
#[derive(Default, Clone)]
struct Candidate {
    n: u64,
    errors: u64,
    p50: i64,
    p90: i64,
    p99: i64,
    sat: u64,
    tol: u64,
    fru: u64,
    h4: u64,
    h5: u64,
}

impl Candidate {
    /// Fold row `i` of `cols` in. Counts accumulate (a bucket can appear in several batches);
    /// percentiles are t-digest estimates that cannot be summed, so the latest wins — the same
    /// approximation this engine has always made.
    fn absorb(&mut self, cols: &CandidateCols<'_>, i: usize) {
        self.n += nonneg_u64(cols.n, i);
        self.errors += nonneg_u64(cols.errors, i);
        self.p50 = opt_i64(cols.p50, i);
        self.p90 = opt_i64(cols.p90, i);
        self.p99 = opt_i64(cols.p99, i);
        self.sat += nonneg_u64(cols.sat, i);
        self.tol += nonneg_u64(cols.tol, i);
        self.fru += nonneg_u64(cols.fru, i);
        self.h4 += nonneg_u64(cols.h4, i);
        self.h5 += nonneg_u64(cols.h5, i);
    }
}

/// One candidate entry set's ten aggregate columns in a `RecordBatch`, found by the `suffix`
/// `candidate_aggs` aliased them with (mirrors `crate::red`'s decode-side `Candidate`).
struct CandidateCols<'a> {
    n: &'a Int64Array,
    errors: &'a Int64Array,
    p50: &'a Int64Array,
    p90: &'a Int64Array,
    p99: &'a Int64Array,
    sat: &'a Int64Array,
    tol: &'a Int64Array,
    fru: &'a Int64Array,
    h4: &'a Int64Array,
    h5: &'a Int64Array,
}

impl<'a> CandidateCols<'a> {
    fn read(b: &'a RecordBatch, suffix: &str) -> Result<CandidateCols<'a>, PhotonError> {
        Ok(CandidateCols {
            n: i64_col(b, &format!("n{suffix}"))?,
            errors: i64_col(b, &format!("errors{suffix}"))?,
            p50: i64_col(b, &format!("p50{suffix}"))?,
            p90: i64_col(b, &format!("p90{suffix}"))?,
            p99: i64_col(b, &format!("p99{suffix}"))?,
            sat: i64_col(b, &format!("sat{suffix}"))?,
            tol: i64_col(b, &format!("tol{suffix}"))?,
            fru: i64_col(b, &format!("fru{suffix}"))?,
            h4: i64_col(b, &format!("h4{suffix}"))?,
            h5: i64_col(b, &format!("h5{suffix}"))?,
        })
    }
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
            required_attributes: Vec::new(),
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
            false,
            false,
            &AttrKeys::Unknown,
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
            false,
            false,
            &AttrKeys::Unknown,
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
            false,
            false,
            &AttrKeys::Unknown,
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
            false,
            false,
            &AttrKeys::Unknown,
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
            required_attributes: Vec::new(),
        };
        let out = engine
            .red_timeseries(req, 0, 100, 10_000_000, 500)
            .await
            .unwrap();
        assert_eq!(out.len(), 3000, "buckets must be clamped to MAX_BUCKETS");
    }

    // ---- ADR-0003: entry spans define a service's request rate ----------------------------
    //
    // The tests above still pass unchanged: `span_at()` leaves `kind` and `parent_span_id` unset,
    // so every one of their spans is a trace root and cascade rule 2 reproduces the old numbers.

    /// OTLP `SpanKind` discriminants, spelled out for the tests below.
    const INTERNAL: Option<i32> = Some(1);
    const SERVER: Option<i32> = Some(2);
    const CLIENT: Option<i32> = Some(3);
    const CONSUMER: Option<i32> = Some(5);

    /// `span_at` plus the two columns the entry-set cascade reads.
    fn span_at_kind(
        service: &str,
        name: &str,
        start_ns: i64,
        dur: Option<i64>,
        status: Option<i32>,
        kind: Option<i32>,
        parent: Option<&str>,
    ) -> SpanRecord {
        SpanRecord {
            kind,
            parent_span_id: parent.map(str::to_string),
            ..span_at(service, name, start_ns, dur, status)
        }
    }

    /// `span_at_kind` plus one HTTP status-code attribute, for the `http_4xx`/`http_5xx` tests
    /// below (mirrors `crate::red::tests::span_http`).
    #[allow(clippy::too_many_arguments)]
    fn span_at_http(
        service: &str,
        name: &str,
        start_ns: i64,
        dur: Option<i64>,
        status: Option<i32>,
        kind: Option<i32>,
        parent: Option<&str>,
        key: &str,
        value: &str,
    ) -> SpanRecord {
        let mut s = span_at_kind(service, name, start_ns, dur, status, kind, parent);
        s.attributes.insert(key.to_string(), value.to_string());
        s
    }

    /// One bucket over `[0, 100]` for `svc`, the shape every cascade test below wants.
    async fn one_bucket(records: &[SpanRecord]) -> RedBucket {
        red_over_time(
            df_of(records).await,
            span_base_predicate(&req_for("service.name:svc", 0, 100)),
            0,
            100,
            1,
            500,
            false,
            false,
            &AttrKeys::Unknown,
        )
        .await
        .unwrap()
        .remove(0)
    }

    #[tokio::test]
    async fn entry_spans_only_count_toward_the_bucket_rate() {
        let mut records = vec![span_at_kind(
            "svc",
            "GET /orders",
            10,
            Some(50_000_000),
            Some(1),
            SERVER,
            None,
        )];
        for i in 1..=9 {
            records.push(span_at_kind(
                "svc",
                "stage",
                10,
                Some(i * 1_000_000),
                Some(1),
                INTERNAL,
                Some("a1"),
            ));
        }

        let b = one_bucket(&records).await;
        assert_eq!(b.count, 1);
        assert_eq!(b.span_count, 10);
        assert_eq!(b.basis, RedBasis::Entry);
    }

    #[tokio::test]
    async fn client_span_errors_do_not_error_the_bucket() {
        let records = vec![
            span_at_kind(
                "svc",
                "GET /orders",
                10,
                Some(50_000_000),
                Some(1),
                SERVER,
                None,
            ),
            span_at_kind(
                "svc",
                "SELECT orders",
                10,
                Some(9_000_000),
                Some(2),
                CLIENT,
                Some("a1"),
            ),
        ];

        let b = one_bucket(&records).await;
        assert_eq!(b.count, 1);
        assert_eq!(b.error_count, 0);
        assert_eq!(b.span_count, 2);
    }

    #[tokio::test]
    async fn consumer_spans_are_entry_points_in_a_bucket() {
        let records = vec![
            span_at_kind(
                "svc",
                "process job",
                10,
                Some(50_000_000),
                Some(1),
                CONSUMER,
                None,
            ),
            span_at_kind(
                "svc",
                "decode",
                10,
                Some(1_000_000),
                Some(1),
                INTERNAL,
                Some("w1"),
            ),
        ];

        let b = one_bucket(&records).await;
        assert_eq!(b.count, 1);
        assert_eq!(b.span_count, 2);
        assert_eq!(b.basis, RedBasis::Entry);
    }

    #[tokio::test]
    async fn worker_without_entry_spans_falls_back_to_trace_roots() {
        let records = vec![
            span_at_kind("svc", "tick", 10, Some(50_000_000), Some(1), INTERNAL, None),
            span_at_kind(
                "svc",
                "step",
                10,
                Some(10_000_000),
                Some(1),
                INTERNAL,
                Some("c1"),
            ),
            span_at_kind(
                "svc",
                "step",
                10,
                Some(20_000_000),
                Some(1),
                INTERNAL,
                Some("c1"),
            ),
        ];

        let b = one_bucket(&records).await;
        assert_eq!(b.count, 1);
        assert_eq!(b.span_count, 3);
        assert_eq!(b.basis, RedBasis::TraceRoot);
    }

    #[tokio::test]
    async fn service_with_neither_falls_back_to_all_spans() {
        let records = vec![
            span_at_kind(
                "svc",
                "step",
                10,
                Some(10_000_000),
                Some(1),
                INTERNAL,
                Some("p1"),
            ),
            span_at_kind(
                "svc",
                "step",
                10,
                Some(20_000_000),
                Some(2),
                INTERNAL,
                Some("p1"),
            ),
            span_at_kind(
                "svc",
                "step",
                10,
                Some(30_000_000),
                Some(1),
                INTERNAL,
                Some("p1"),
            ),
        ];

        let b = one_bucket(&records).await;
        assert_eq!(b.count, 3);
        assert_eq!(b.span_count, 3);
        assert_eq!(b.error_count, 1);
        assert_eq!(b.basis, RedBasis::AllSpans);
    }

    #[tokio::test]
    async fn apdex_bands_are_computed_over_the_entry_set() {
        // One satisfied request + one still open (no band) + five very slow INTERNAL spans that
        // must not reach the bands.
        let mut records = vec![
            span_at_kind(
                "svc",
                "GET /orders",
                10,
                Some(100_000_000),
                Some(1),
                SERVER,
                None,
            ),
            span_at_kind("svc", "GET /slow", 10, None, Some(1), SERVER, None),
        ];
        for i in 1..=5 {
            records.push(span_at_kind(
                "svc",
                "stage",
                10,
                Some(9_000_000_000 + i),
                Some(1),
                INTERNAL,
                Some("a1"),
            ));
        }

        let b = one_bucket(&records).await;
        assert_eq!(b.count, 2);
        assert_eq!(b.span_count, 7);
        assert_eq!((b.satisfied, b.tolerating, b.frustrated), (1, 0, 0));
        assert!(b.satisfied + b.tolerating + b.frustrated <= b.count);
    }

    #[tokio::test]
    async fn basis_is_decided_once_for_the_series_not_per_bucket() {
        // Bucket 0 serves a request; bucket 1 holds only INTERNAL parented spans. A per-bucket
        // cascade would fall back to all-spans in bucket 1 and spike the chart against a
        // neighbour measured a different way — so the series decides once.
        let records = vec![
            span_at_kind(
                "svc",
                "GET /orders",
                10,
                Some(50_000_000),
                Some(1),
                SERVER,
                None,
            ),
            span_at_kind(
                "svc",
                "stage",
                10,
                Some(1_000_000),
                Some(1),
                INTERNAL,
                Some("a1"),
            ),
            span_at_kind(
                "svc",
                "step",
                150,
                Some(2_000_000),
                Some(1),
                INTERNAL,
                Some("b1"),
            ),
            span_at_kind(
                "svc",
                "step",
                160,
                Some(3_000_000),
                Some(1),
                INTERNAL,
                Some("b1"),
            ),
        ];
        let out = red_over_time(
            df_of(&records).await,
            span_base_predicate(&req_for("service.name:svc", 0, 200)),
            0,
            200,
            2,
            500,
            false,
            false,
            &AttrKeys::Unknown,
        )
        .await
        .unwrap();

        assert_eq!((out[0].count, out[0].span_count), (1, 2));
        assert_eq!(
            (out[1].count, out[1].span_count),
            (0, 2),
            "no request entered the service in bucket 1"
        );
        assert!(out.iter().all(|b| b.basis == RedBasis::Entry));
    }

    #[tokio::test]
    async fn an_empty_series_reports_no_spans_and_the_entry_basis() {
        let out = red_over_time(
            df_of(&[]).await,
            span_base_predicate(&req_for("service.name:svc", 0, 100)),
            0,
            100,
            2,
            500,
            false,
            false,
            &AttrKeys::Unknown,
        )
        .await
        .unwrap();
        assert!(out.iter().all(|b| b.span_count == 0));
        assert!(out.iter().all(|b| b.basis == RedBasis::Entry));
    }

    // ---- error taxonomy: http_4xx / http_5xx sit beside error_count, not instead of it -----

    #[tokio::test]
    async fn counts_4xx_and_5xx_from_the_http_attribute() {
        let records = vec![
            span_at_http(
                "svc",
                "GET /a",
                10,
                Some(10_000_000),
                Some(1),
                SERVER,
                None,
                HTTP_STATUS_CODE,
                "404",
            ),
            span_at_http(
                "svc",
                "GET /b",
                10,
                Some(10_000_000),
                Some(1),
                SERVER,
                None,
                HTTP_STATUS_CODE,
                "500",
            ),
            span_at_http(
                "svc",
                "GET /c",
                10,
                Some(10_000_000),
                Some(1),
                SERVER,
                None,
                HTTP_STATUS_CODE,
                "200",
            ),
        ];
        let b = one_bucket(&records).await;
        assert_eq!(b.http_4xx, 1);
        assert_eq!(b.http_5xx, 1);
    }

    #[tokio::test]
    async fn falls_back_to_the_legacy_http_status_code_key() {
        let records = vec![span_at_http(
            "svc",
            "GET /a",
            10,
            Some(10_000_000),
            Some(1),
            SERVER,
            None,
            HTTP_STATUS_CODE_LEGACY,
            "503",
        )];
        let b = one_bucket(&records).await;
        assert_eq!(b.http_5xx, 1);
        assert_eq!(b.http_4xx, 0);
    }

    #[tokio::test]
    async fn prefers_the_current_key_when_both_are_present() {
        let mut s = span_at_kind("svc", "GET /a", 10, Some(10_000_000), Some(1), SERVER, None);
        s.attributes.insert(HTTP_STATUS_CODE.into(), "404".into());
        s.attributes
            .insert(HTTP_STATUS_CODE_LEGACY.into(), "500".into());
        let b = one_bucket(&[s]).await;
        assert_eq!(b.http_4xx, 1, "the current key wins over the legacy one");
        assert_eq!(b.http_5xx, 0);
    }

    #[tokio::test]
    async fn non_http_spans_contribute_to_neither() {
        let records = vec![span_at_kind(
            "svc",
            "GET /a",
            10,
            Some(10_000_000),
            Some(1),
            SERVER,
            None,
        )];
        let b = one_bucket(&records).await;
        assert_eq!(b.http_4xx, 0);
        assert_eq!(b.http_5xx, 0);
    }

    #[tokio::test]
    async fn http_counters_follow_the_entry_set() {
        // The SERVER entry span succeeded (200); a downstream CLIENT call failed with a 500 that
        // the code retried and the caller never saw — `http_5xx` must not count the CLIENT span.
        let records = vec![
            span_at_http(
                "svc",
                "GET /orders",
                10,
                Some(50_000_000),
                Some(1),
                SERVER,
                None,
                HTTP_STATUS_CODE,
                "200",
            ),
            span_at_http(
                "svc",
                "POST /charge",
                10,
                Some(9_000_000),
                Some(1),
                CLIENT,
                Some("a1"),
                HTTP_STATUS_CODE,
                "500",
            ),
        ];
        let b = one_bucket(&records).await;
        assert_eq!(b.basis, RedBasis::Entry);
        assert_eq!(
            b.http_5xx, 0,
            "the CLIENT span's 500 is not in the entry set"
        );
        assert_eq!(b.http_4xx, 0);
    }

    #[tokio::test]
    async fn a_4xx_span_is_not_an_error_count() {
        // `status_code` stays OK (1) but the HTTP status is a 404 — `error_count` must stay 0
        // while `http_4xx` is the only counter that moves.
        let records = vec![span_at_http(
            "svc",
            "GET /missing",
            10,
            Some(10_000_000),
            Some(1),
            SERVER,
            None,
            HTTP_STATUS_CODE,
            "404",
        )];
        let b = one_bucket(&records).await;
        assert_eq!(b.error_count, 0);
        assert_eq!(b.http_4xx, 1);
    }
}

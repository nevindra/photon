//! `red_metrics`: RED (Rate, Errors, Duration) per (service.name[, operation]) over the matched
//! span set. Mirrors `crate::span_latency` (percentiles via `approx_percentile_cont`) and
//! `crate::span_facet` (grouped aggregate over `span_survivors_df` + `span_base_predicate`), but
//! groups by service — and, for `RedGroup::Operation`, operation `name` — and additionally counts
//! errored spans (`status_code == 2`, OTEL ERROR).
//!
//! **A service's Requests are its ENTRY spans, not every span it emits** (ADR-0003). `count`,
//! `error_count`, the duration percentiles and the Apdex bands are all taken over the entry set
//! chosen by [`entry_mask`]/[`root_mask`]'s cascade; the total span count survives beside them as
//! `RedRow::span_count`, and `RedRow::basis` says which cascade rule produced the number. The
//! cascade and the per-candidate aggregate block are defined here once and reused by
//! `crate::red_timeseries`, so the two engines can never drift on what a request is.
//!
//! v1 is breadth-first / correctness-over-speed, like `search_traces`: the aggregate is recomputed
//! per query over the pruned candidate set (no precomputed rollups — the flagged optimization
//! target). Group cardinality (service × operation) is naturally bounded, but a `MAX_RED_GROUPS`
//! cap (ordered by count desc, logged — never silent) guards a pathological high-cardinality name.
use std::collections::HashMap;

use arrow::array::{Array, Int64Array, StringArray};
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::functions::string::expr_fn::starts_with;
use datafusion::functions_aggregate::expr_fn::{approx_percentile_cont, sum};
use datafusion::prelude::{col, lit, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::{attr_lookup, span_base_predicate, AttrKeys};
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

/// Which rule of the entry-set cascade produced a row's `count` (ADR-0003). It travels with the
/// number so the UI can be honest about what it is showing rather than quietly changing meaning —
/// `AllSpans` earns a visible `span-based` badge on the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedBasis {
    /// `kind IN (SERVER, CONSUMER)` — the spans where work entered the service. The normal case.
    Entry,
    /// The group had no entry spans, so trace roots (no `parent_span_id`) stood in — a worker, or
    /// an SDK that never sets `kind`.
    TraceRoot,
    /// Neither: every span in the group counted as a request.
    AllSpans,
}

/// One RED row: identity + raw counts + duration percentiles (nanoseconds). Rate and error-rate
/// are DERIVED in the API handler from `count`/`error_count` and the query window — the engine
/// stays window-agnostic beyond the pruning it already applied.
#[derive(Debug, Clone, PartialEq)]
pub struct RedRow {
    /// `service.name`.
    pub service: String,
    /// The `tenant` attribute stamped by federation ingest; `None` for local (untenanted) spans.
    /// Only populated for `RedGroup::Service` (the tables that label rows by tenant).
    pub tenant: Option<String>,
    /// `deployment.environment`. Like `tenant`, only populated for `RedGroup::Service`: the
    /// Services index splits a service into one row per `(Site, environment)`, because "broken in
    /// prod" and "broken in staging" are different facts about the same piece of software.
    pub environment: Option<String>,
    /// Operation `name`; `Some` only for `RedGroup::Operation`, `None` for `RedGroup::Service`.
    pub operation: Option<String>,
    /// Requests served: spans in the group's ENTRY set (see `basis`) — the rate numerator.
    pub count: u64,
    /// EVERY span matched in the group, all kinds. The second, separate number: `count` answers
    /// "how many requests did you serve", this one "how much work did you record" (ADR-0003).
    pub span_count: u64,
    /// Which cascade rule produced `count` (and the percentiles and bands below).
    pub basis: RedBasis,
    /// Entry spans with `status_code == 2` (OTEL ERROR) in the group. A failed CLIENT span — a
    /// database call the code retried successfully — is not an error the caller saw.
    pub error_count: u64,
    /// p50/p90/p99 of `duration_nanos` over the group's entry set (t-digest approx). 0 when it has
    /// no non-null duration (e.g. every span is still open).
    pub p50: i64,
    pub p90: i64,
    pub p99: i64,
    /// Entry spans whose duration ≤ the service's Apdex T (satisfied). Null-duration spans count
    /// in none, so the three bands sum to at most `count`.
    pub satisfied: u64,
    /// Entry spans with T < duration ≤ 4T (tolerating).
    pub tolerating: u64,
    /// Entry spans with duration > 4T (frustrated).
    pub frustrated: u64,
    /// Entry spans whose HTTP status attribute (`http.response.status_code`, falling back to the
    /// legacy `http.status_code`) has a leading digit of `4`. Sits BESIDE `error_count`, not
    /// instead of it: `error_count` keeps meaning exactly `status_code == 2` (OTEL ERROR) so
    /// "error rate" continues to mean one thing. An SDK that never sets span status hides real
    /// HTTP 4xx/5xx from `error_count` entirely; these two counters give the Overview a second,
    /// HTTP-native read over the SAME entry set (see `basis`) without redefining the first one. A
    /// span with neither key set contributes to neither counter.
    pub http_4xx: u64,
    /// Entry spans whose HTTP status attribute has a leading digit of `5`. See `http_4xx`.
    pub http_5xx: u64,
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
        let promoted = self.promoted_attributes();
        let tenant_promoted = promoted.iter().any(|p| p == "tenant");
        let env_promoted = promoted.iter().any(|p| p == ENVIRONMENT);
        let status_promoted = promoted.iter().any(|p| p == HTTP_STATUS_CODE);
        let status_legacy_promoted = promoted.iter().any(|p| p == HTTP_STATUS_CODE_LEGACY);
        match self.span_survivors(&req).await? {
            None => Ok(Vec::new()),
            Some((df, attr_keys)) => {
                red_over(
                    df,
                    span_base_predicate(&req),
                    group,
                    thresholds,
                    default_ms,
                    tenant_promoted,
                    env_promoted,
                    status_promoted,
                    status_legacy_promoted,
                    &attr_keys,
                )
                .await
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

/// OTLP `SpanKind` discriminants for the two kinds that mean "work entered this service".
const KIND_SERVER: i32 = 2;
const KIND_CONSUMER: i32 = 5;

/// Cascade rule 1: the spans where work entered the service. A NULL `kind` (an SDK that never set
/// it) yields NULL here, so the CASE arms below fall through to 0 rather than counting the span.
pub(crate) fn entry_mask() -> Expr {
    col_ref(span_schema::KIND)
        .eq(lit(KIND_SERVER))
        .or(col_ref(span_schema::KIND).eq(lit(KIND_CONSUMER)))
}

/// Cascade rule 2: trace roots. `photon-ingest` maps OTLP's all-zero parent to `None`, but an
/// empty string is checked too so a differently-mapped corpus still reads as a root.
pub(crate) fn root_mask() -> Expr {
    col_ref(span_schema::PARENT_SPAN_ID)
        .is_null()
        .or(col_ref(span_schema::PARENT_SPAN_ID).eq(lit("")))
}

/// `deployment.environment` — the third global context dimension (`lib/core/context.ts`), promoted
/// by default.
pub(crate) const ENVIRONMENT: &str = "deployment.environment";

/// Current OTel semantic-convention key for an HTTP span's status code.
pub(crate) const HTTP_STATUS_CODE: &str = "http.response.status_code";
/// The key the convention replaced. Read only when the current key is absent.
pub(crate) const HTTP_STATUS_CODE_LEGACY: &str = "http.status_code";

/// The span's HTTP status code as a DataFusion expression: `HTTP_STATUS_CODE` if present, else
/// `HTTP_STATUS_CODE_LEGACY`, else NULL. NULL means neither key was set, and
/// `candidate_aggs`'s `starts_with` flags read NULL as "not in the flagged set" — a span with
/// neither key contributes to neither `http_4xx` nor `http_5xx`.
///
/// Each key is looked up by its own promoted/map location — the same choice `red_over` makes
/// below for `tenant`: `span_record` routes a promoted key OUT of the `attributes` map, so a map
/// lookup on a promoted key reads NULL.
pub(crate) fn http_status_expr(
    current_promoted: bool,
    legacy_promoted: bool,
    keys: &AttrKeys,
) -> Expr {
    let current = attr_lookup(HTTP_STATUS_CODE, current_promoted, keys);
    let legacy = attr_lookup(HTTP_STATUS_CODE_LEGACY, legacy_promoted, keys);
    when(current.clone().is_not_null(), current)
        .otherwise(legacy)
        .expect("http status coalesce case")
}

/// Names of the four per-row values every RED aggregate derives from. They are materialised ONCE by
/// [`with_red_derived`] in a projection ahead of the aggregate, and the aggregate expressions then
/// reference the resulting columns instead of re-stating the expressions that built them.
///
/// This is not tidiness, it is the read path's single biggest cost. Each candidate block below
/// mentions `http_status` twice, `t_ns` three times and its mask seven times, and `red_over` builds
/// three candidate blocks — so an inlined `http_status` appears **six** times in one plan, and each
/// occurrence is two `get_field` probes into the `attributes` map. That map is 63% of a span
/// Parquet file's bytes and 96% of its *decoded* bytes (133 MB raw against 5.5 MB for every other
/// column put together), so paying for it once per row instead of six times is worth roughly 6x on
/// every Backend page. Measured on 86.6M spans: `/api/red` over 24h went 2133 ms → 318 ms, over 7d
/// 13371 ms → 2050 ms.
pub(crate) const DERIVED_HTTP: &str = "__red_http_status";
pub(crate) const DERIVED_T: &str = "__red_apdex_t";
pub(crate) const DERIVED_ENTRY: &str = "__red_is_entry";
pub(crate) const DERIVED_ROOT: &str = "__red_is_root";

/// The four derived columns as expressions, ready to hand to `candidate_aggs` / the mask arguments.
/// Always used together with [`with_red_derived`], which is what puts them on the frame.
pub(crate) struct RedDerived {
    pub http_status: Expr,
    pub t_ns: Expr,
    pub entry: Expr,
    pub root: Expr,
}

impl RedDerived {
    pub(crate) fn cols() -> Self {
        Self {
            http_status: col(DERIVED_HTTP),
            t_ns: col(DERIVED_T),
            entry: col(DERIVED_ENTRY),
            root: col(DERIVED_ROOT),
        }
    }
}

/// Materialise the four derived columns on `df` so the aggregate that follows evaluates each of
/// them exactly once per row. See [`DERIVED_HTTP`] for why this matters so much.
pub(crate) fn with_red_derived(
    df: DataFrame,
    thresholds: &HashMap<String, u32>,
    default_ms: u32,
    status_promoted: bool,
    status_legacy_promoted: bool,
    attr_keys: &AttrKeys,
) -> Result<DataFrame, PhotonError> {
    fn map_err(what: &'static str) -> impl Fn(datafusion::error::DataFusionError) -> PhotonError {
        move |e| PhotonError::Query(format!("red derived {what}: {e}"))
    }
    df.with_column(
        DERIVED_HTTP,
        http_status_expr(status_promoted, status_legacy_promoted, attr_keys),
    )
    .map_err(map_err("http status"))?
    .with_column(DERIVED_T, threshold_ns_expr(thresholds, default_ms))
    .map_err(map_err("apdex threshold"))?
    .with_column(DERIVED_ENTRY, entry_mask())
    .map_err(map_err("entry mask"))?
    .with_column(DERIVED_ROOT, root_mask())
    .map_err(map_err("root mask"))
}

/// The seven COUNTING per-group aggregates for ONE candidate entry set — count, error count, the
/// three Apdex bands, and the two HTTP status-class counts (4xx/5xx) — aliased with `suffix` so
/// several candidates can sit side by side in the same `.aggregate()` call (`""` = all spans,
/// `"_e"` = entry, `"_r"` = trace roots).
///
/// Split out from [`candidate_aggs`] because the rollup pass (`crate::red_rollup`) wants these
/// seven and NOT the three `approx_percentile_cont` t-digests below: it emits a real explicit-
/// bucket latency histogram instead, which the metrics engine can re-quantile over any window.
/// Three t-digests per candidate is the expensive part of this aggregate, so skipping them pays
/// for the latency-bucket flags the rollup adds in their place.
pub(crate) fn candidate_count_aggs(
    mask: Option<&Expr>,
    dur: &Expr,
    t_ns: &Expr,
    http_status: &Expr,
    suffix: &str,
) -> Result<Vec<Expr>, PhotonError> {
    let four_t = t_ns.clone() * lit(4_i64);
    // A 0/1 flag per span: `cond` AND-ed with the candidate's mask, summed per group below.
    let flag = |cond: Expr, what: &str| -> Result<Expr, PhotonError> {
        let in_set = match mask {
            Some(m) => m.clone().and(cond),
            None => cond,
        };
        when(in_set, lit(1_i64))
            .otherwise(lit(0_i64))
            .map_err(|e| PhotonError::Query(format!("red {what} case: {e}")))
    };

    let n = flag(lit(true), "count")?;
    let errors = flag(
        col_ref(span_schema::STATUS_CODE).eq(lit(2_i32)),
        "error-flag",
    )?;
    let satisfied = flag(dur.clone().lt_eq(t_ns.clone()), "satisfied")?;
    let tolerating = flag(
        dur.clone()
            .gt(t_ns.clone())
            .and(dur.clone().lt_eq(four_t.clone())),
        "tolerating",
    )?;
    let frustrated = flag(dur.clone().gt(four_t), "frustrated")?;
    // Leading-digit classification avoids a numeric parse in the expression tree. The stored
    // attribute value is a string; `starts_with` is NULL when `http_status` is NULL (neither key
    // set), and `flag`'s CASE falls through to 0 for a NULL condition — so a non-HTTP span
    // contributes to neither counter.
    let http_4xx = flag(starts_with(http_status.clone(), lit("4")), "http-4xx")?;
    let http_5xx = flag(starts_with(http_status.clone(), lit("5")), "http-5xx")?;

    Ok(vec![
        sum(n).alias(format!("n{suffix}")),
        sum(errors).alias(format!("errors{suffix}")),
        sum(satisfied).alias(format!("sat{suffix}")),
        sum(tolerating).alias(format!("tol{suffix}")),
        sum(frustrated).alias(format!("fru{suffix}")),
        sum(http_4xx).alias(format!("h4{suffix}")),
        sum(http_5xx).alias(format!("h5{suffix}")),
    ])
}

/// A duration expression NULLed outside `mask` — `approx_percentile_cont` filters nulls out before
/// merging into its t-digest, and `sum` ignores them, so a masked-out row contributes nothing.
pub(crate) fn masked_duration(mask: Option<&Expr>, dur: &Expr) -> Result<Expr, PhotonError> {
    match mask {
        // CASE with no ELSE → NULL outside the mask.
        Some(m) => when(m.clone(), dur.clone())
            .end()
            .map_err(|e| PhotonError::Query(format!("red duration mask case: {e}"))),
        None => Ok(dur.clone()),
    }
}

/// [`candidate_count_aggs`] plus the three duration percentiles — the full ten-aggregate set the
/// interactive RED read paths (`red_over`, `red_over_time`) use. `mask = None` means "every row",
/// which is both the third cascade rule and the `span_count` source (`n`).
pub(crate) fn candidate_aggs(
    mask: Option<&Expr>,
    dur: &Expr,
    t_ns: &Expr,
    http_status: &Expr,
    suffix: &str,
) -> Result<Vec<Expr>, PhotonError> {
    let mut aggs = candidate_count_aggs(mask, dur, t_ns, http_status, suffix)?;
    let masked_dur = masked_duration(mask, dur)?;
    aggs.extend([
        approx_percentile_cont(masked_dur.clone(), lit(0.5_f64), None)
            .alias(format!("p50{suffix}")),
        approx_percentile_cont(masked_dur.clone(), lit(0.9_f64), None)
            .alias(format!("p90{suffix}")),
        approx_percentile_cont(masked_dur, lit(0.99_f64), None).alias(format!("p99{suffix}")),
    ]);
    Ok(aggs)
}

/// Filter → grouped aggregate (count, error count, p50/p90/p99) → order by count desc → cap →
/// decode. Split out from `red_metrics` so the unit tests can drive it over a `MemTable` DataFrame.
#[allow(clippy::too_many_arguments)]
async fn red_over(
    df: DataFrame,
    predicate: Expr,
    group: RedGroup,
    thresholds: &HashMap<String, u32>,
    default_ms: u32,
    tenant_promoted: bool,
    env_promoted: bool,
    status_promoted: bool,
    status_legacy_promoted: bool,
    attr_keys: &AttrKeys,
) -> Result<Vec<RedRow>, PhotonError> {
    // The four per-row values below are materialised as columns before the aggregate (see
    // `DERIVED_HTTP`) — inlining them here instead puts six copies of the `attributes` map probe
    // into one plan and costs ~6x on every Backend page.
    let d = RedDerived::cols();
    let (t_ns, http_status) = (d.t_ns.clone(), d.http_status.clone());
    let dur = col_ref(span_schema::DURATION);

    // ADR-0003. A service's Requests are its ENTRY spans, not every span it emits: a service that
    // wraps each stage of its own work in an INTERNAL span is not busier than one doing the same
    // work with a single span, and a CLIENT span that failed (a database call the code retried
    // successfully) is not an error the caller saw. So the counts, percentiles and Apdex bands
    // below are taken over the entry set; the raw span total survives beside them as `span_count`.
    //
    // The entry set is a cascade, first non-empty wins, decided PER GROUP — so a worker that only
    // emits INTERNAL spans, or an SDK that never sets `kind`, never silently vanishes from the
    // table exactly when it breaks:
    //   1. kind IN (SERVER, CONSUMER)   2. trace roots   3. every span
    //
    // "Per group" is why all three candidates are computed side by side in ONE aggregate pass and
    // the winner is picked in Rust when decoding: the choice turns on a group-level fact (does
    // this group have any entry spans at all?) that no per-row expression can know. Three separate
    // queries would put three DataFusion passes on the read path of every Backend page.
    let mut aggs = candidate_aggs(None, &dur, &t_ns, &http_status, "")?;
    aggs.extend(candidate_aggs(
        Some(&d.entry),
        &dur,
        &t_ns,
        &http_status,
        "_e",
    )?);
    aggs.extend(candidate_aggs(
        Some(&d.root),
        &dur,
        &t_ns,
        &http_status,
        "_r",
    )?);

    // The count each group will actually report, in SQL, so the cap below can order by it: the
    // busiest groups must survive, and "busiest" now means requests served, not spans recorded.
    let effective_n = when(col("n_e").gt(lit(0_i64)), col("n_e"))
        .when(col("n_r").gt(lit(0_i64)), col("n_r"))
        .otherwise(col("n"))
        .map_err(|e| PhotonError::Query(format!("red effective-count case: {e}")))?;

    let group_exprs: Vec<Expr> = match group {
        RedGroup::Operation => vec![
            col_ref("service.name").alias("service"),
            col_ref(span_schema::NAME).alias("operation"),
        ],
        RedGroup::Service => {
            // `span_record` routes promoted keys OUT of the attributes map into their own column,
            // so a map lookup on a promoted `tenant` reads NULL — pick the lookup by schema.
            let tenant_expr = attr_lookup("tenant", tenant_promoted, attr_keys);
            let env_expr = attr_lookup(ENVIRONMENT, env_promoted, attr_keys);
            vec![
                col_ref("service.name").alias("service"),
                tenant_expr.alias("tenant"),
                env_expr.alias("environment"),
            ]
        }
    };

    let filtered = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("red filter: {e}")))?;
    let batches = with_red_derived(
        filtered,
        thresholds,
        default_ms,
        status_promoted,
        status_legacy_promoted,
        attr_keys,
    )?
    .aggregate(group_exprs, aggs)
    .map_err(|e| PhotonError::Query(format!("red aggregate: {e}")))?
    .with_column("eff_n", effective_n)
    .map_err(|e| PhotonError::Query(format!("red effective-count column: {e}")))?
    .sort(vec![
        col("eff_n").sort(false, false), // count desc — keep the busiest groups under the cap
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
        let tenant = match group {
            RedGroup::Service => Some(str_col(b, "tenant")?),
            RedGroup::Operation => None,
        };
        let environment = match group {
            RedGroup::Service => Some(str_col(b, "environment")?),
            RedGroup::Operation => None,
        };
        // The three candidates, side by side in the one pass; the cascade picks per row below.
        let all = Candidate::read(b, "")?;
        let entry = Candidate::read(b, "_e")?;
        let root = Candidate::read(b, "_r")?;
        for i in 0..b.num_rows() {
            if service.is_null(i) {
                continue; // service.name is promoted+required; a null group is not a real service
            }
            let span_count = all.count(i);
            let (c, basis) = if entry.count(i) > 0 {
                (&entry, RedBasis::Entry)
            } else if root.count(i) > 0 {
                (&root, RedBasis::TraceRoot)
            } else {
                (&all, RedBasis::AllSpans)
            };
            rows.push(RedRow {
                service: service.value(i).to_string(),
                tenant: tenant.and_then(|t| {
                    if t.is_null(i) {
                        None
                    } else {
                        Some(t.value(i).to_string())
                    }
                }),
                environment: environment.and_then(|e| {
                    if e.is_null(i) {
                        None
                    } else {
                        Some(e.value(i).to_string())
                    }
                }),
                operation: operation.and_then(|o| {
                    if o.is_null(i) {
                        None
                    } else {
                        Some(o.value(i).to_string())
                    }
                }),
                count: c.count(i),
                span_count,
                basis,
                error_count: nonneg_u64(c.errors, i),
                p50: opt_i64(c.p50, i),
                p90: opt_i64(c.p90, i),
                p99: opt_i64(c.p99, i),
                satisfied: nonneg_u64(c.sat, i),
                tolerating: nonneg_u64(c.tol, i),
                frustrated: nonneg_u64(c.fru, i),
                http_4xx: nonneg_u64(c.h4, i),
                http_5xx: nonneg_u64(c.h5, i),
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

/// One candidate entry set's ten aggregate columns inside a `RecordBatch`, borrowed by the
/// `suffix` [`candidate_aggs`] aliased them with. Purely a decode-side grouping — it holds column
/// references, not values, so picking a cascade winner per row is a pointer choice.
struct Candidate<'a> {
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

impl<'a> Candidate<'a> {
    fn read(b: &'a RecordBatch, suffix: &str) -> Result<Candidate<'a>, PhotonError> {
        Ok(Candidate {
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

    /// Rows in this candidate set for group `i` — the cascade's "non-empty" test.
    fn count(&self, i: usize) -> u64 {
        nonneg_u64(self.n, i)
    }
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
            required_attributes: Vec::new(),
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
            false,
            false,
            false,
            false,
            &AttrKeys::Unknown,
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
            false,
            false,
            false,
            false,
            &AttrKeys::Unknown,
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
    async fn service_group_splits_rows_per_site_and_environment() {
        // ADR-0004: the Services index is one row per (service, Site, environment). "Broken in
        // prod" and "broken in staging" are different facts about the same piece of software, and
        // merging them would hide the one that matters.
        let mk = |tenant: Option<&str>, env: Option<&str>| {
            let mut sp = span("api", "op", Some(10_000_000), Some(1));
            if let Some(t) = tenant {
                sp.attributes.insert("tenant".into(), t.into());
            }
            if let Some(e) = env {
                sp.attributes.insert(ENVIRONMENT.into(), e.into());
            }
            sp
        };
        let records = vec![
            mk(Some("jkt"), Some("prod")),
            mk(Some("jkt"), Some("prod")),
            mk(Some("jkt"), Some("staging")),
            mk(Some("sgp"), Some("prod")),
            mk(None, None),
        ];
        let rows = red_over(
            df_of(&records).await,
            span_base_predicate(&req()),
            RedGroup::Service,
            &std::collections::HashMap::new(),
            500,
            false,
            false,
            false,
            false,
            &AttrKeys::Unknown,
        )
        .await
        .unwrap();

        assert_eq!(
            rows.len(),
            4,
            "3 (Site, env) pairs plus the untagged local one"
        );
        let find = |t: Option<&str>, e: Option<&str>| {
            rows.iter()
                .find(|r| r.tenant.as_deref() == t && r.environment.as_deref() == e)
                .unwrap_or_else(|| panic!("missing row for {t:?}/{e:?}"))
        };
        assert_eq!(find(Some("jkt"), Some("prod")).count, 2);
        assert_eq!(find(Some("jkt"), Some("staging")).count, 1);
        assert_eq!(find(Some("sgp"), Some("prod")).count, 1);
        assert_eq!(find(None, None).count, 1);
    }

    #[tokio::test]
    async fn operation_group_carries_no_site_or_environment() {
        // Those two label the index's rows; an operation row is already inside one service, and
        // repeating them there would invite summing across levels.
        let mut sp = span("api", "op", Some(10_000_000), Some(1));
        sp.attributes.insert("tenant".into(), "jkt".into());
        sp.attributes.insert(ENVIRONMENT.into(), "prod".into());
        let rows = red_over(
            df_of(&[sp]).await,
            span_base_predicate(&req()),
            RedGroup::Operation,
            &std::collections::HashMap::new(),
            500,
            false,
            false,
            false,
            false,
            &AttrKeys::Unknown,
        )
        .await
        .unwrap();
        assert_eq!(rows[0].tenant, None);
        assert_eq!(rows[0].environment, None);
    }

    #[tokio::test]
    async fn service_group_splits_and_labels_rows_by_tenant_attr() {
        let mut a = span("api", "op", Some(10_000_000), Some(1));
        a.attributes.insert("tenant".into(), "divtik".into());
        let local = span("api", "op", Some(20_000_000), Some(1));
        let rows = red_over(
            df_of(&[a, local]).await,
            span_base_predicate(&req()),
            RedGroup::Service,
            &std::collections::HashMap::new(),
            500,
            false,
            false,
            false,
            false,
            &AttrKeys::Unknown,
        )
        .await
        .unwrap();

        assert_eq!(rows.len(), 2); // same service name, split by tenant
        let tenanted = rows.iter().find(|r| r.tenant.is_some()).unwrap();
        assert_eq!(tenanted.tenant.as_deref(), Some("divtik"));
        assert!(rows.iter().any(|r| r.tenant.is_none()));
    }

    #[tokio::test]
    async fn service_group_labels_tenant_when_promoted() {
        // Same data as the map-path test above, but `tenant` is a promoted column — the
        // configuration `federation_e2e.rs` runs central with.
        let schema = SpanSchema::new(&["service.name".into(), "tenant".into()]);
        let mut a = span("api", "op", Some(10_000_000), Some(1));
        a.attributes.insert("tenant".into(), "divtik".into());
        let local = span("api", "op", Some(20_000_000), Some(1));
        let mut b = SpanBatchBuilder::new(&schema);
        b.append(&a);
        b.append(&local);
        let ctx = SessionContext::new();
        ctx.register_table(
            "spans",
            Arc::new(
                MemTable::try_new(schema.arrow.clone(), vec![vec![b.finish().unwrap()]]).unwrap(),
            ),
        )
        .unwrap();
        let rows = red_over(
            ctx.table("spans").await.unwrap(),
            span_base_predicate(&req()),
            RedGroup::Service,
            &std::collections::HashMap::new(),
            500,
            true,
            false,
            false,
            false,
            &AttrKeys::Unknown,
        )
        .await
        .unwrap();

        assert_eq!(rows.len(), 2);
        let tenanted = rows.iter().find(|r| r.tenant.is_some()).unwrap();
        assert_eq!(tenanted.tenant.as_deref(), Some("divtik"));
        assert!(rows.iter().any(|r| r.tenant.is_none()));
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
            false,
            false,
            false,
            false,
            &AttrKeys::Unknown,
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
            false,
            false,
            false,
            false,
            &AttrKeys::Unknown,
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

    // ---- ADR-0003: entry spans define a service's request rate ----------------------------
    //
    // Note the tests above still pass unchanged: `span()` leaves `kind` and `parent_span_id`
    // unset, so every one of their spans is a trace root and cascade rule 2 reproduces the old
    // all-spans numbers exactly.

    /// OTLP `SpanKind` discriminants, spelled out for the tests below.
    const INTERNAL: Option<i32> = Some(1);
    const SERVER: Option<i32> = Some(2);
    const CLIENT: Option<i32> = Some(3);
    const CONSUMER: Option<i32> = Some(5);

    /// `span` plus the two columns the entry-set cascade reads: the OTLP `kind` discriminant and
    /// `parent_span_id`.
    fn span_kind(
        service: &str,
        name: &str,
        dur: Option<i64>,
        status: Option<i32>,
        kind: Option<i32>,
        parent: Option<&str>,
    ) -> SpanRecord {
        SpanRecord {
            kind,
            parent_span_id: parent.map(str::to_string),
            ..span(service, name, dur, status)
        }
    }

    /// `span_kind` plus one HTTP status-code attribute (`key` is one of `HTTP_STATUS_CODE` /
    /// `HTTP_STATUS_CODE_LEGACY`), for the `http_4xx`/`http_5xx` tests below.
    #[allow(clippy::too_many_arguments)]
    fn span_http(
        service: &str,
        name: &str,
        dur: Option<i64>,
        status: Option<i32>,
        kind: Option<i32>,
        parent: Option<&str>,
        key: &str,
        value: &str,
    ) -> SpanRecord {
        let mut s = span_kind(service, name, dur, status, kind, parent);
        s.attributes.insert(key.to_string(), value.to_string());
        s
    }

    /// `red_over` grouped per service with default thresholds — the shape every cascade test wants.
    async fn service_rows(records: &[SpanRecord]) -> Vec<RedRow> {
        red_over(
            df_of(records).await,
            span_base_predicate(&req()),
            RedGroup::Service,
            &std::collections::HashMap::new(),
            500,
            false,
            false,
            false,
            false,
            &AttrKeys::Unknown,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn entry_spans_only_count_toward_rate() {
        // One request served (SERVER), wrapped in nine INTERNAL spans of the service's own work.
        let mut records = vec![span_kind(
            "api",
            "GET /orders",
            Some(50_000_000),
            Some(1),
            SERVER,
            None,
        )];
        for i in 1..=9 {
            records.push(span_kind(
                "api",
                "stage",
                Some(i * 1_000_000),
                Some(1),
                INTERNAL,
                Some("a1"),
            ));
        }

        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.count, 1, "one request entered the service");
        assert_eq!(api.span_count, 10, "every span still counted, separately");
        assert_eq!(api.basis, RedBasis::Entry);
    }

    #[tokio::test]
    async fn client_span_errors_do_not_error_the_service() {
        // The database call failed and the code retried; the request the user saw succeeded.
        let records = vec![
            span_kind(
                "api",
                "GET /orders",
                Some(50_000_000),
                Some(1),
                SERVER,
                None,
            ),
            span_kind(
                "api",
                "SELECT orders",
                Some(9_000_000),
                Some(2),
                CLIENT,
                Some("a1"),
            ),
        ];

        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.error_count, 0);
        assert_eq!(api.count, 1);
        assert_eq!(api.span_count, 2);
    }

    #[tokio::test]
    async fn consumer_spans_are_entry_points() {
        // A queue worker: work enters on CONSUMER, never on SERVER.
        let records = vec![
            span_kind(
                "worker",
                "process job",
                Some(50_000_000),
                Some(1),
                CONSUMER,
                None,
            ),
            span_kind(
                "worker",
                "decode",
                Some(1_000_000),
                Some(1),
                INTERNAL,
                Some("w1"),
            ),
        ];

        let rows = service_rows(&records).await;
        let worker = rows.iter().find(|r| r.service == "worker").unwrap();
        assert_eq!(worker.count, 1);
        assert_eq!(worker.span_count, 2);
        assert_eq!(worker.basis, RedBasis::Entry);
    }

    #[tokio::test]
    async fn worker_without_entry_spans_falls_back_to_trace_roots() {
        let records = vec![
            span_kind("cron", "tick", Some(50_000_000), Some(1), INTERNAL, None), // the root
            span_kind(
                "cron",
                "step",
                Some(10_000_000),
                Some(1),
                INTERNAL,
                Some("c1"),
            ),
            span_kind(
                "cron",
                "step",
                Some(20_000_000),
                Some(1),
                INTERNAL,
                Some("c1"),
            ),
        ];

        let rows = service_rows(&records).await;
        let cron = rows.iter().find(|r| r.service == "cron").unwrap();
        assert_eq!(cron.count, 1);
        assert_eq!(cron.span_count, 3);
        assert_eq!(cron.basis, RedBasis::TraceRoot);
    }

    #[tokio::test]
    async fn service_with_neither_falls_back_to_all_spans() {
        // Every span INTERNAL and parented — an SDK that never sets `kind`, seen mid-trace. The
        // service must not vanish from the table; it reports every span and says so via `basis`.
        let records = vec![
            span_kind(
                "lib",
                "step",
                Some(10_000_000),
                Some(1),
                INTERNAL,
                Some("p1"),
            ),
            span_kind(
                "lib",
                "step",
                Some(20_000_000),
                Some(2),
                INTERNAL,
                Some("p1"),
            ),
            span_kind(
                "lib",
                "step",
                Some(30_000_000),
                Some(1),
                INTERNAL,
                Some("p1"),
            ),
        ];

        let rows = service_rows(&records).await;
        let lib = rows.iter().find(|r| r.service == "lib").unwrap();
        assert_eq!(lib.count, 3);
        assert_eq!(lib.span_count, 3);
        assert_eq!(lib.error_count, 1);
        assert_eq!(lib.basis, RedBasis::AllSpans);
    }

    #[tokio::test]
    async fn apdex_bands_are_computed_over_the_entry_set() {
        // Two requests entered (one still open, so it lands in no band) and five very slow
        // INTERNAL spans that must not reach the bands at all.
        let mut records = vec![
            span_kind(
                "api",
                "GET /orders",
                Some(100_000_000),
                Some(1),
                SERVER,
                None,
            ), // satisfied
            span_kind("api", "GET /slow", None, Some(1), SERVER, None), // open: no band
        ];
        for i in 1..=5 {
            records.push(span_kind(
                "api",
                "stage",
                Some(9_000_000_000 + i),
                Some(1),
                INTERNAL,
                Some("a1"),
            ));
        }

        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.count, 2);
        assert_eq!(api.span_count, 7);
        assert_eq!((api.satisfied, api.tolerating, api.frustrated), (1, 0, 0));
        let banded = api.satisfied + api.tolerating + api.frustrated;
        assert!(banded <= api.count, "bands never exceed the entry count");
        assert!(banded < api.span_count, "and are not taken over every span");
    }

    #[tokio::test]
    async fn basis_is_decided_per_group_not_globally() {
        let records = vec![
            span_kind(
                "api",
                "GET /orders",
                Some(50_000_000),
                Some(1),
                SERVER,
                None,
            ),
            span_kind(
                "api",
                "stage",
                Some(1_000_000),
                Some(1),
                INTERNAL,
                Some("a1"),
            ),
            span_kind(
                "lib",
                "step",
                Some(10_000_000),
                Some(1),
                INTERNAL,
                Some("p1"),
            ),
            span_kind(
                "lib",
                "step",
                Some(20_000_000),
                Some(1),
                INTERNAL,
                Some("p1"),
            ),
        ];

        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        let lib = rows.iter().find(|r| r.service == "lib").unwrap();
        assert_eq!((api.basis, api.count), (RedBasis::Entry, 1));
        assert_eq!((lib.basis, lib.count), (RedBasis::AllSpans, 2));
    }

    // ---- error taxonomy: http_4xx / http_5xx sit beside error_count, not instead of it -----

    #[tokio::test]
    async fn counts_4xx_and_5xx_from_the_http_attribute() {
        let records = vec![
            span_http(
                "api",
                "GET /a",
                Some(10_000_000),
                Some(1),
                SERVER,
                None,
                HTTP_STATUS_CODE,
                "404",
            ),
            span_http(
                "api",
                "GET /b",
                Some(10_000_000),
                Some(1),
                SERVER,
                None,
                HTTP_STATUS_CODE,
                "500",
            ),
            span_http(
                "api",
                "GET /c",
                Some(10_000_000),
                Some(1),
                SERVER,
                None,
                HTTP_STATUS_CODE,
                "200",
            ),
        ];
        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.http_4xx, 1);
        assert_eq!(api.http_5xx, 1);
    }

    #[tokio::test]
    async fn falls_back_to_the_legacy_http_status_code_key() {
        let records = vec![span_http(
            "api",
            "GET /a",
            Some(10_000_000),
            Some(1),
            SERVER,
            None,
            HTTP_STATUS_CODE_LEGACY,
            "503",
        )];
        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.http_5xx, 1);
        assert_eq!(api.http_4xx, 0);
    }

    #[tokio::test]
    async fn prefers_the_current_key_when_both_are_present() {
        let mut s = span_kind("api", "GET /a", Some(10_000_000), Some(1), SERVER, None);
        s.attributes.insert(HTTP_STATUS_CODE.into(), "404".into());
        s.attributes
            .insert(HTTP_STATUS_CODE_LEGACY.into(), "500".into());
        let rows = service_rows(&[s]).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.http_4xx, 1, "the current key wins over the legacy one");
        assert_eq!(api.http_5xx, 0);
    }

    #[tokio::test]
    async fn non_http_spans_contribute_to_neither() {
        let records = vec![span_kind(
            "api",
            "GET /a",
            Some(10_000_000),
            Some(1),
            SERVER,
            None,
        )];
        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.http_4xx, 0);
        assert_eq!(api.http_5xx, 0);
    }

    #[tokio::test]
    async fn http_counters_follow_the_entry_set() {
        // The SERVER entry span succeeded (200); a downstream CLIENT call failed with a 500 that
        // the code retried and the caller never saw. `http_5xx` must not count the CLIENT span —
        // same reasoning as `client_span_errors_do_not_error_the_service` above, applied to the
        // HTTP counters.
        let records = vec![
            span_http(
                "api",
                "GET /orders",
                Some(50_000_000),
                Some(1),
                SERVER,
                None,
                HTTP_STATUS_CODE,
                "200",
            ),
            span_http(
                "api",
                "POST /charge",
                Some(9_000_000),
                Some(1),
                CLIENT,
                Some("a1"),
                HTTP_STATUS_CODE,
                "500",
            ),
        ];
        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.basis, RedBasis::Entry);
        assert_eq!(
            api.http_5xx, 0,
            "the CLIENT span's 500 is not in the entry set"
        );
        assert_eq!(api.http_4xx, 0);
    }

    #[tokio::test]
    async fn a_4xx_span_is_not_an_error_count() {
        // `status_code` stays OK (1) — the SDK never marked the span an OTEL error — but the HTTP
        // status is a 404. `error_count` must stay untouched; `http_4xx` is the only counter that
        // moves. Asserts the two concepts (`status_code == 2` vs. an HTTP status class) stay
        // separate, per RedRow::http_4xx's doc comment.
        let records = vec![span_http(
            "api",
            "GET /missing",
            Some(10_000_000),
            Some(1),
            SERVER,
            None,
            HTTP_STATUS_CODE,
            "404",
        )];
        let rows = service_rows(&records).await;
        let api = rows.iter().find(|r| r.service == "api").unwrap();
        assert_eq!(api.error_count, 0);
        assert_eq!(api.http_4xx, 1);
    }
}

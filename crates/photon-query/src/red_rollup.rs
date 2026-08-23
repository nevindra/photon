//! `red_rollup`: the query behind the `photon.service.*` rollup task
//! (`docs/adr/0002-red-rollups-are-published-as-metrics.md`).
//!
//! This is the third RED shape, and it exists because neither of the other two fits a background
//! pass. [`crate::red`]'s `red_metrics` groups by service but not by time; [`crate::red_timeseries`]
//! buckets by time but, in its own words, "only buckets by time, it does not group by service".
//! The rollup needs BOTH at once, plus `deployment.environment`, so it is its own query rather
//! than N calls to either — one pass over a 10-minute window instead of one call per service per
//! bucket.
//!
//! Three things make it different from the interactive read paths:
//!
//! 1. **No t-digests.** It emits an explicit-bucket latency histogram
//!    ([`LATENCY_BOUNDS_NS`]) that `metric_dist::histogram_series` can re-quantile over ANY window.
//!    Percentiles pre-computed per bucket cannot be re-aggregated — averaging p99s is not a p99 —
//!    so storing the distribution is the only way a wide-window read off the rollup is honest.
//! 2. **Every tenant that is not rolling itself up.** Spans carrying a `tenant` attribute arrived
//!    from a Site. When that Site ships its own rollup (`push_service_rollup`), rolling the
//!    mirrored spans up again on Central would double-count the federated service — so Central
//!    skips those tenants, and the caller says which ones by passing `covered_tenants`.
//!
//!    Central used to skip ALL mirrored spans, on the assumption that every Site ships its rollup.
//!    `push_service_rollup` is opt-in, so that assumption is wrong by default, and the result was
//!    worse than slow: Central's watermark still advanced over those windows, so
//!    `services_rollup::rollup_eligible` reported the window as covered, the Backend pages read a
//!    rollup that had deliberately been given nothing, and the fleet board rendered EMPTY. A
//!    Central aggregating Sites that do not push now rolls their spans up itself.
//! 3. **Bounded operation cardinality.** See [`RollupPass::folded_operations`].

use std::collections::{HashMap, HashSet};

use arrow::array::{Array, Int64Array, StringArray};
use arrow::datatypes::DataType;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::expr_fn::sum;
use datafusion::prelude::{cast, lit, when, Expr};

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::red::{
    candidate_count_aggs, entry_mask, http_status_expr, masked_duration, root_mask, RedBasis,
    HTTP_STATUS_CODE, HTTP_STATUS_CODE_LEGACY,
};
use crate::span_engine::{attr_lookup, span_base_predicate, AttrKeys};
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// The metric-name suffix the latency histogram is published under — `photon.service.latency`
/// and `photon.operation.latency`. It was `…duration` while [`LATENCY_BOUNDS_NS`] topped out at
/// 10s; the rename is what lets the longer ladder below exist without a query interpolating
/// across two different meanings of the same name. See the ladder's own note.
pub const LATENCY_METRIC: &str = "latency";

/// Explicit histogram bucket boundaries for `photon.service.latency`, in NANOSECONDS, ascending.
/// A ladder over the range request latency actually occupies: sub-millisecond internal calls at
/// one end, a half-hour batch job at the other, densest through 50ms–1s where the p90 of most
/// services sits.
///
/// The tail above 10s is not padding. Everything past the top bound lands in the `+Inf` overflow
/// bucket, and the quantile read interpolates that bucket at the top bound — so a ladder ending
/// at 10s reports **exactly 10s** for a service whose real p50 is two minutes, silently, with no
/// marker that the number is a floor rather than a measurement. Whatever the top bound is, it is
/// the largest latency this histogram can ever report.
///
/// These are the `explicit_bounds` of the emitted OTLP histogram, so they are a **wire contract**:
/// changing them changes the meaning of every already-stored point under [`LATENCY_METRIC`], and
/// a query spanning the change would interpolate across two different ladders. Renumbering this
/// array means renaming [`LATENCY_METRIC`] in the same change — that is how the 10s ladder was
/// retired, and it is the only safe way to retire this one.
pub const LATENCY_BOUNDS_NS: [i64; 19] = [
    1_000_000,         // 1ms
    2_000_000,         // 2ms
    5_000_000,         // 5ms
    10_000_000,        // 10ms
    20_000_000,        // 20ms
    50_000_000,        // 50ms
    100_000_000,       // 100ms
    200_000_000,       // 200ms
    500_000_000,       // 500ms
    1_000_000_000,     // 1s
    2_000_000_000,     // 2s
    5_000_000_000,     // 5s
    10_000_000_000,    // 10s
    30_000_000_000,    // 30s
    60_000_000_000,    // 1m
    120_000_000_000,   // 2m
    300_000_000_000,   // 5m
    600_000_000_000,   // 10m
    1_800_000_000_000, // 30m
];

/// The operation name every folded-away operation is summed into. Not a real operation: it is the
/// honest remainder, and the UI must render it as such rather than as an endpoint someone can
/// click into.
pub const OTHER_OPERATION: &str = "__other__";

/// Discovery candidates keyed by `(service, environment)`: `(count, operation)` pairs, sorted in
/// Rust because the candidate set is already bounded (see `top_operations_over`).
type DiscoveryByService = HashMap<(String, Option<String>, Option<String>), Vec<(u64, String)>>;

/// Hard cap on the (service, environment, operation) triples operation discovery will consider.
/// An operation name carrying an id (`GET /v1/charges/ch_1a2b3c`) makes distinct operations scale
/// with distinct REQUESTS, so this is the difference between a bounded background pass and one
/// that takes the metrics store down. Discovery aggregates ONE column per triple, so 200k of them
/// is a few megabytes — the expensive 21-aggregate pass runs only over the survivors.
const MAX_DISCOVERY_TRIPLES: usize = 200_000;

/// Cap on rows returned by either level of the pass, after folding. Folding already bounds the
/// operation level at `(top_operations + 1) × services × environments × buckets`; this is the
/// backstop for a service count nobody predicted.
const MAX_ROLLUP_ROWS: usize = 200_000;

/// One rolled-up time bucket for one series. `operation` is `None` at the service level and
/// `Some` at the operation level (possibly [`OTHER_OPERATION`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RollupRow {
    /// Bucket START, epoch nanos, aligned to the pass's bucket grid.
    pub ts: i64,
    pub service: String,
    /// The Site this service belongs to, from the span's `tenant` attribute; `None` for a
    /// service running on this node rather than mirrored in from a federated Site.
    pub tenant: Option<String>,
    /// `deployment.environment`; `None` when the service never set it.
    pub environment: Option<String>,
    /// `None` at the service level.
    pub operation: Option<String>,
    /// Requests served: spans in the series' ENTRY set (see `basis`).
    pub count: u64,
    /// EVERY span in the bucket, all kinds (ADR-0003's honest second number).
    pub span_count: u64,
    /// Which cascade rule produced `count`. Decided ONCE per series, not per bucket.
    pub basis: RedBasis,
    pub error_count: u64,
    pub http_4xx: u64,
    pub http_5xx: u64,
    pub satisfied: u64,
    pub tolerating: u64,
    pub frustrated: u64,
    /// OTLP-style NON-cumulative bucket counts over [`LATENCY_BOUNDS_NS`], length
    /// `LATENCY_BOUNDS_NS.len() + 1` — the last element is the `+Inf` overflow bucket. Spans with
    /// a NULL duration land in none of them, so this sums to at most `count`.
    pub latency_buckets: Vec<u64>,
    /// Sum of the entry set's durations, nanos — the histogram's `sum` field.
    pub duration_sum: i64,
    /// Entry-set spans that HAVE a duration — the histogram's `count` field, and what
    /// `latency_buckets` sums to. Below `count` whenever the bucket held an open span.
    pub duration_count: u64,
}

/// Both levels of one rollup pass, plus what it had to give up to stay bounded.
#[derive(Debug, Clone, Default)]
pub struct RollupPass {
    /// One row per (bucket, service, environment).
    pub services: Vec<RollupRow>,
    /// One row per (bucket, service, environment, operation), including [`OTHER_OPERATION`].
    pub operations: Vec<RollupRow>,
    /// Distinct (service, environment, operation) triples that did NOT make the per-service top-N
    /// and were summed into [`OTHER_OPERATION`]. Zero means the operation level is complete. The
    /// caller logs this — a rollup that quietly drops half a service's endpoints is worse than one
    /// that says so.
    pub folded_operations: usize,
    /// Discovery hit [`MAX_DISCOVERY_TRIPLES`]. [`OTHER_OPERATION`] is still EXACT when this is
    /// true (it is aggregated, not computed by subtraction), but the top-N choice was made over a
    /// truncated candidate set, so a busy operation may have been passed over.
    pub discovery_truncated: bool,
}

impl SpanQueryEngine {
    /// Roll up `[start, end)` into `bucket_ns`-wide buckets, at both the service and the operation
    /// level, over LOCAL spans only.
    ///
    /// `start` and `bucket_ns` must be chosen by the caller so bucket boundaries land on whole
    /// minutes — the rollup re-emits its lag window every pass and the reader de-duplicates by
    /// `(series, timestamp)`, which only works if successive passes agree on the grid.
    ///
    /// `top_operations` is the per-`(service, environment)` operation cap; the rest fold into
    /// [`OTHER_OPERATION`]. `0` skips the operation level entirely.
    #[allow(clippy::too_many_arguments)]
    pub async fn red_rollup(
        &self,
        req: SpanQueryRequest,
        start: i64,
        end: i64,
        bucket_ns: i64,
        thresholds: &HashMap<String, u32>,
        default_ms: u32,
        top_operations: usize,
        covered_tenants: &HashSet<String>,
    ) -> Result<RollupPass, PhotonError> {
        let promoted = self.promoted_attributes();
        // Survivors first: the attribute expressions below are built against what the surviving
        // files actually declare, so a key none of them carries never drags the `attributes` map
        // into the plan (see `AttrKeys`).
        let Some((df, attr_keys)) = self.span_survivors(&req).await? else {
            return Ok(RollupPass::default());
        };
        let cols = RollupCols {
            environment: promoted_or_map(promoted, ENVIRONMENT, &attr_keys),
            tenant: promoted_or_map(promoted, "tenant", &attr_keys),
            http_status: http_status_expr(
                promoted.iter().any(|p| p == HTTP_STATUS_CODE),
                promoted.iter().any(|p| p == HTTP_STATUS_CODE_LEGACY),
                &attr_keys,
            ),
        };
        // Local spans, plus every mirrored tenant that is not already publishing its own rollup
        // into this window (see the module header). `covered_tenants` is what the caller observed
        // in the metrics, not a config flag — a Site's `push_service_rollup` is the Site's
        // setting, and Central cannot read it.
        let mut skip: Vec<Expr> = covered_tenants.iter().map(|t| lit(t.as_str())).collect();
        skip.sort_by_key(|e| format!("{e}")); // deterministic plan across passes
        let mine = if skip.is_empty() {
            cols.tenant
                .clone()
                .is_null()
                .or(cols.tenant.clone().is_not_null())
        } else {
            cols.tenant
                .clone()
                .is_null()
                .or(cols.tenant.clone().in_list(skip, true))
        };
        let predicate = span_base_predicate(&req).and(mine);

        let services = rollup_over(
            df.clone(),
            predicate.clone(),
            &cols,
            start,
            end,
            bucket_ns,
            thresholds,
            default_ms,
            None,
            None,
        )
        .await?;

        if top_operations == 0 {
            return Ok(RollupPass {
                services,
                ..Default::default()
            });
        }

        let (keep, folded_operations, discovery_truncated) = discover_top_operations(
            df.clone(),
            predicate.clone(),
            &cols,
            thresholds,
            default_ms,
            top_operations,
        )
        .await?;

        // The operation level inherits its service's basis rather than resolving its own. Two
        // things break otherwise. A CLIENT-only operation (`SELECT users`) has no entry spans, so
        // its own cascade would fall through to AllSpans and report every outbound call as a
        // request the service served — the exact conflation ADR-0003 exists to end. And operations
        // resolved on mixed bases no longer sum to their service, which is what makes an operation
        // table (and `__other__`) trustworthy.
        let basis_by_service = basis_map(&services);

        let operations = rollup_over(
            df,
            predicate,
            &cols,
            start,
            end,
            bucket_ns,
            thresholds,
            default_ms,
            Some(&keep),
            Some(&basis_by_service),
        )
        .await?;

        Ok(RollupPass {
            services,
            operations,
            folded_operations,
            discovery_truncated,
        })
    }
}

/// `deployment.environment`, the OTel resource attribute the Backend world's environment dimension
/// is built on (`lib/core/context.ts`); promoted by default since Task 3.
const ENVIRONMENT: &str = "deployment.environment";

/// A key's value expression, picked by where the schema actually put it: `span_record` routes a
/// promoted key OUT of the `attributes` map, so a map lookup on a promoted key reads NULL. Same
/// choice `red_over` makes for `tenant`.
fn promoted_or_map(promoted: &[String], key: &str, attr_keys: &AttrKeys) -> Expr {
    attr_lookup(key, promoted.iter().any(|p| p == key), attr_keys)
}

/// The three schema-dependent expressions every query in the pass needs, resolved once.
struct RollupCols {
    environment: Expr,
    tenant: Expr,
    http_status: Expr,
}

/// The 21 aggregates for ONE candidate entry set: [`candidate_count_aggs`]'s seven counters, the
/// duration sum, and one cumulative `<= bound` flag per [`LATENCY_BOUNDS_NS`] entry.
///
/// The latency flags are CUMULATIVE here (`count(dur <= bound)`, i.e. Prometheus `le` semantics)
/// because that is what a single summed flag can express; [`decode_latency`] differences them into
/// the non-cumulative counts OTLP's `bucket_counts` wants.
fn rollup_aggs(
    mask: Option<&Expr>,
    dur: &Expr,
    t_ns: &Expr,
    http_status: &Expr,
    suffix: &str,
) -> Result<Vec<Expr>, PhotonError> {
    let mut aggs = candidate_count_aggs(mask, dur, t_ns, http_status, suffix)?;
    let masked_dur = masked_duration(mask, dur)?;
    aggs.push(sum(masked_dur.clone()).alias(format!("dsum{suffix}")));
    // The histogram's own count. An open span (NULL duration) is a request, but it is not a
    // MEASURED one — counting it would push it into `+Inf` and report a timeout that never
    // happened.
    let measured = when(masked_dur.clone().is_not_null(), lit(1_i64))
        .otherwise(lit(0_i64))
        .map_err(|e| PhotonError::Query(format!("rollup measured case: {e}")))?;
    aggs.push(sum(measured).alias(format!("dcnt{suffix}")));
    for (i, bound) in LATENCY_BOUNDS_NS.iter().enumerate() {
        let le = when(masked_dur.clone().lt_eq(lit(*bound)), lit(1_i64))
            .otherwise(lit(0_i64))
            .map_err(|e| PhotonError::Query(format!("rollup le-{i} case: {e}")))?;
        aggs.push(sum(le).alias(format!("le{i}{suffix}")));
    }
    Ok(aggs)
}

/// Per-row Apdex threshold `T` in nanoseconds: a CASE keyed on `service.name` falling back to
/// `default_ms`. Mirrors `red::threshold_ns_expr` — restated rather than shared because the rollup
/// is allowed to diverge from the interactive path's cap conventions without dragging it along.
fn threshold_ns_expr(thresholds: &HashMap<String, u32>, default_ms: u32) -> Expr {
    const MS: i64 = 1_000_000;
    let mut names: Vec<&String> = thresholds.keys().collect();
    names.sort(); // deterministic plan across passes
    let mut case = when(lit(false), lit(0_i64));
    for name in names {
        let ms = thresholds[name] as i64;
        case = case.when(col_ref("service.name").eq(lit(name.as_str())), lit(ms * MS));
    }
    case.otherwise(lit(default_ms as i64 * MS))
        .expect("rollup apdex threshold case")
}

/// The bucket INDEX expression: `(start_time - start) / bucket_ns`, with a row landing exactly on
/// `end` clamped down into the last bucket (mirrors `red_over_time`).
fn bucket_index_expr(start: i64, buckets: i64, bucket_ns: i64) -> Result<Expr, PhotonError> {
    let start_time = cast(col_ref(span_schema::START_TIME), DataType::Int64);
    let raw = (start_time - lit(start)) / lit(bucket_ns);
    when(raw.clone().gt_eq(lit(buckets)), lit(buckets - 1))
        .otherwise(raw)
        .map_err(|e| PhotonError::Query(format!("rollup bucket case: {e}")))
}

/// The composite identity of one operation series, as the single string the fold test compares.
/// `\u{1}` cannot appear in a tenant, a service name, an environment or a span name, so this is
/// injective — which matters, because a collision would fold one service's hot endpoint into
/// another's tail.
fn operation_key(
    service: &str,
    tenant: Option<&str>,
    environment: Option<&str>,
    operation: &str,
) -> String {
    format!(
        "{service}\u{1}{}\u{1}{}\u{1}{operation}",
        tenant.unwrap_or(""),
        environment.unwrap_or("")
    )
}

/// The same key, as a DataFusion expression over the grouped columns.
fn operation_key_expr(tenant: &Expr, environment: &Expr) -> Expr {
    use datafusion::functions::expr_fn::concat;
    fn or_empty(e: &Expr) -> Expr {
        when(e.clone().is_null(), lit(""))
            .otherwise(e.clone())
            .expect("rollup null-to-empty case")
    }
    concat(vec![
        col_ref("service.name"),
        lit("\u{1}"),
        or_empty(tenant),
        lit("\u{1}"),
        or_empty(environment),
        lit("\u{1}"),
        col_ref(span_schema::NAME),
    ])
}

/// Pass 1 of the operation level: which (service, environment, operation) triples earn a series of
/// their own. One cheap aggregate column per triple, then `row_number()` partitioned by
/// `(service, environment)` keeps the busiest `top_n` of each — top-N PER SERVICE, so a single
/// high-cardinality service cannot starve every other service out of the operation level.
///
/// Returns the surviving triple keys, how many triples were folded, and whether discovery itself
/// was truncated.
async fn discover_top_operations(
    df: DataFrame,
    predicate: Expr,
    cols: &RollupCols,
    thresholds: &HashMap<String, u32>,
    default_ms: u32,
    top_n: usize,
) -> Result<(HashSet<String>, usize, bool), PhotonError> {
    let t_ns = threshold_ns_expr(thresholds, default_ms);
    let dur = col_ref(span_schema::DURATION);

    // Ordering is by the count the series will actually report, so "busiest" means requests
    // served, not spans recorded — the same effective-count cascade `red_over` orders by.
    let n_all = candidate_count_aggs(None, &dur, &t_ns, &cols.http_status, "")?;
    let n_entry = candidate_count_aggs(Some(&entry_mask()), &dur, &t_ns, &cols.http_status, "_e")?;
    let n_root = candidate_count_aggs(Some(&root_mask()), &dur, &t_ns, &cols.http_status, "_r")?;
    // Only the `n*` aggregate of each set is needed; the rest would be computed and thrown away.
    let aggs = vec![
        n_all.into_iter().next().expect("n"),
        n_entry.into_iter().next().expect("n_e"),
        n_root.into_iter().next().expect("n_r"),
    ];

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("rollup discovery filter: {e}")))?
        .aggregate(
            vec![
                col_ref("service.name").alias("service"),
                cols.tenant.clone().alias("tenant"),
                cols.environment.clone().alias("environment"),
                col_ref(span_schema::NAME).alias("operation"),
            ],
            aggs,
        )
        .map_err(|e| PhotonError::Query(format!("rollup discovery aggregate: {e}")))?
        .limit(0, Some(MAX_DISCOVERY_TRIPLES + 1))
        .map_err(|e| PhotonError::Query(format!("rollup discovery limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("rollup discovery collect: {e}")))?;

    // `row_number()` is expressible in DataFusion, but the candidate set is already bounded at
    // MAX_DISCOVERY_TRIPLES and sorting it in Rust keeps the plan a single aggregate — no window
    // operator, no second sort of the whole span scan.
    let mut per_service: DiscoveryByService = HashMap::new();
    let mut total = 0usize;
    for b in &batches {
        let service = str_col(b, "service")?;
        let tenant = str_col(b, "tenant")?;
        let environment = str_col(b, "environment")?;
        let operation = str_col(b, "operation")?;
        let n = i64_col(b, "n")?;
        let n_e = i64_col(b, "n_e")?;
        let n_r = i64_col(b, "n_r")?;
        for i in 0..b.num_rows() {
            if service.is_null(i) || operation.is_null(i) {
                continue;
            }
            let eff = match (nonneg_u64(n_e, i), nonneg_u64(n_r, i)) {
                (e, _) if e > 0 => e,
                (_, r) if r > 0 => r,
                _ => nonneg_u64(n, i),
            };
            let env = (!environment.is_null(i)).then(|| environment.value(i).to_string());
            let ten = (!tenant.is_null(i)).then(|| tenant.value(i).to_string());
            per_service
                .entry((service.value(i).to_string(), ten, env))
                .or_default()
                .push((eff, operation.value(i).to_string()));
            total += 1;
        }
    }
    let discovery_truncated = total > MAX_DISCOVERY_TRIPLES;

    let mut keep = HashSet::new();
    let mut folded = 0usize;
    for ((service, tenant, environment), mut ops) in per_service {
        // count desc, then name asc so a tie picks the same operation on every pass — a series
        // that flickers in and out of `__other__` between passes is worse than a stable choice.
        ops.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (rank, (_, operation)) in ops.into_iter().enumerate() {
            if rank < top_n {
                keep.insert(operation_key(
                    &service,
                    tenant.as_deref(),
                    environment.as_deref(),
                    &operation,
                ));
            } else {
                folded += 1;
            }
        }
    }
    Ok((keep, folded, discovery_truncated))
}

/// The bucketed aggregate, at whichever level `keep` selects: `None` groups by
/// `(bucket, service, environment)`, `Some(keep)` adds a fourth key that is the operation name for
/// a surviving triple and [`OTHER_OPERATION`] for every other.
///
/// Folding in the GROUP BY rather than after the fact is what bounds this pass: the aggregate can
/// never build more than `(top_operations + 1)` operation groups per `(service, environment,
/// bucket)`, no matter how many distinct operation names the corpus contains. It also keeps
/// `__other__` self-consistent — it is aggregated over its own rows, with its own cascade, rather
/// than derived by subtracting the top-N from the service total (which would be wrong the moment
/// an operation resolved to a different basis than its service).
#[allow(clippy::too_many_arguments)]
async fn rollup_over(
    df: DataFrame,
    predicate: Expr,
    cols: &RollupCols,
    start: i64,
    end: i64,
    bucket_ns: i64,
    thresholds: &HashMap<String, u32>,
    default_ms: u32,
    keep: Option<&HashSet<String>>,
    basis_by_service: Option<&HashMap<ServiceKey, RedBasis>>,
) -> Result<Vec<RollupRow>, PhotonError> {
    let bucket_ns = bucket_ns.max(1);
    let buckets = (((end - start).max(1) + bucket_ns - 1) / bucket_ns).max(1);
    let t_ns = threshold_ns_expr(thresholds, default_ms);
    let dur = col_ref(span_schema::DURATION);

    let mut aggs = rollup_aggs(None, &dur, &t_ns, &cols.http_status, "")?;
    aggs.extend(rollup_aggs(
        Some(&entry_mask()),
        &dur,
        &t_ns,
        &cols.http_status,
        "_e",
    )?);
    aggs.extend(rollup_aggs(
        Some(&root_mask()),
        &dur,
        &t_ns,
        &cols.http_status,
        "_r",
    )?);

    let mut group_exprs = vec![
        bucket_index_expr(start, buckets, bucket_ns)?.alias("bucket"),
        col_ref("service.name").alias("service"),
        cols.tenant.clone().alias("tenant"),
        cols.environment.clone().alias("environment"),
    ];
    if let Some(keep) = keep {
        let mut keys: Vec<Expr> = keep.iter().map(|k| lit(k.as_str())).collect();
        keys.sort_by_key(|e| format!("{e}")); // deterministic plan across passes
        let survives = if keys.is_empty() {
            lit(false)
        } else {
            operation_key_expr(&cols.tenant, &cols.environment).in_list(keys, false)
        };
        let folded_name = when(survives, col_ref(span_schema::NAME))
            .otherwise(lit(OTHER_OPERATION))
            .map_err(|e| PhotonError::Query(format!("rollup fold case: {e}")))?;
        group_exprs.push(folded_name.alias("operation"));
    }

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("rollup filter: {e}")))?
        .aggregate(group_exprs, aggs)
        .map_err(|e| PhotonError::Query(format!("rollup aggregate: {e}")))?
        .limit(0, Some(MAX_ROLLUP_ROWS + 1))
        .map_err(|e| PhotonError::Query(format!("rollup limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("rollup collect: {e}")))?;

    decode(&batches, start, bucket_ns, keep.is_some(), basis_by_service)
}

/// A `(service, tenant, environment)` series identity — the key an operation inherits its basis
/// by.
pub(crate) type ServiceKey = (String, Option<String>, Option<String>);

/// The basis each `(service, environment)` resolved to, from the service-level rows.
fn basis_map(services: &[RollupRow]) -> HashMap<ServiceKey, RedBasis> {
    services
        .iter()
        .map(|r| {
            (
                (r.service.clone(), r.tenant.clone(), r.environment.clone()),
                r.basis,
            )
        })
        .collect()
}

/// Decode the aggregate into `RollupRow`s, resolving the entry-set cascade ONCE PER SERIES.
///
/// Per-bucket resolution would let a service report Entry in a busy bucket and AllSpans in a quiet
/// one, which would make the emitted rate change meaning mid-chart. `red_over_time` takes the same
/// position for the same reason.
fn decode(
    batches: &[arrow::record_batch::RecordBatch],
    start: i64,
    bucket_ns: i64,
    with_operation: bool,
    basis_by_service: Option<&HashMap<ServiceKey, RedBasis>>,
) -> Result<Vec<RollupRow>, PhotonError> {
    type SeriesKey = (String, Option<String>, Option<String>, Option<String>);
    // Pass 1: collect every bucket's three candidates, keyed by series.
    let mut series: HashMap<SeriesKey, Vec<(i64, Cands)>> = HashMap::new();
    for b in batches {
        let bucket = i64_col(b, "bucket")?;
        let service = str_col(b, "service")?;
        let tenant = str_col(b, "tenant")?;
        let environment = str_col(b, "environment")?;
        let operation = with_operation
            .then(|| str_col(b, "operation"))
            .transpose()?;
        let all = CandCols::read(b, "")?;
        let entry = CandCols::read(b, "_e")?;
        let root = CandCols::read(b, "_r")?;
        for i in 0..b.num_rows() {
            // `service.name` is promoted and required; a null group is not a real service.
            if service.is_null(i) || bucket.is_null(i) {
                continue;
            }
            let key = (
                service.value(i).to_string(),
                (!tenant.is_null(i)).then(|| tenant.value(i).to_string()),
                (!environment.is_null(i)).then(|| environment.value(i).to_string()),
                operation.map(|o| {
                    if o.is_null(i) {
                        OTHER_OPERATION.to_string()
                    } else {
                        o.value(i).to_string()
                    }
                }),
            );
            series.entry(key).or_default().push((
                bucket.value(i),
                Cands {
                    all: all.read_row(i),
                    entry: entry.read_row(i),
                    root: root.read_row(i),
                },
            ));
        }
    }

    // Pass 2: pick the cascade winner per series, then emit its buckets.
    let mut rows = Vec::new();
    for ((service, tenant, environment, operation), buckets) in series {
        let inherited = basis_by_service.and_then(|m| {
            m.get(&(service.clone(), tenant.clone(), environment.clone()))
                .copied()
        });
        let basis = inherited.unwrap_or_else(|| {
            let entry_total: u64 = buckets.iter().map(|(_, c)| c.entry.n).sum();
            let root_total: u64 = buckets.iter().map(|(_, c)| c.root.n).sum();
            if entry_total > 0 {
                RedBasis::Entry
            } else if root_total > 0 {
                RedBasis::TraceRoot
            } else {
                RedBasis::AllSpans
            }
        });
        for (bucket, c) in buckets {
            let won = match basis {
                RedBasis::Entry => &c.entry,
                RedBasis::TraceRoot => &c.root,
                RedBasis::AllSpans => &c.all,
            };
            rows.push(RollupRow {
                ts: start + bucket * bucket_ns,
                service: service.clone(),
                tenant: tenant.clone(),
                environment: environment.clone(),
                operation: operation.clone(),
                count: won.n,
                span_count: c.all.n,
                basis,
                error_count: won.errors,
                http_4xx: won.h4,
                http_5xx: won.h5,
                satisfied: won.sat,
                tolerating: won.tol,
                frustrated: won.fru,
                latency_buckets: decode_latency(&won.le, won.dcnt),
                duration_sum: won.dsum,
                duration_count: won.dcnt,
            });
        }
    }
    // Deterministic output so a caller diffing two passes sees data changes, not ordering churn.
    rows.sort_by(|a, b| {
        a.service
            .cmp(&b.service)
            .then_with(|| a.environment.cmp(&b.environment))
            .then_with(|| a.operation.cmp(&b.operation))
            .then_with(|| a.ts.cmp(&b.ts))
    });
    Ok(rows)
}

/// Cumulative `le` counts → OTLP's non-cumulative `bucket_counts`, with the `+Inf` bucket appended.
///
/// `measured` is the count of spans that HAVE a duration, not the request count: a span still open
/// when the pass ran is unmeasured, not slow, and putting it in `+Inf` would report a ten-second
/// timeout that never happened.
///
/// The `le` counts are monotonic by construction, but `saturating_sub` guards the decode anyway:
/// a corrupt or partially-decoded batch must not produce a histogram whose buckets sum past its
/// count, which would make `interpolate_quantile` return nonsense rather than fail.
fn decode_latency(le: &[u64; LATENCY_BOUNDS_NS.len()], measured: u64) -> Vec<u64> {
    let mut out = Vec::with_capacity(LATENCY_BOUNDS_NS.len() + 1);
    let mut prev = 0u64;
    for cum in le {
        out.push(cum.saturating_sub(prev));
        prev = *cum;
    }
    out.push(measured.saturating_sub(prev));
    out
}

/// One candidate entry set's decoded values for a single bucket.
#[derive(Default, Clone)]
struct Cand {
    n: u64,
    errors: u64,
    sat: u64,
    tol: u64,
    fru: u64,
    h4: u64,
    h5: u64,
    dsum: i64,
    dcnt: u64,
    le: [u64; LATENCY_BOUNDS_NS.len()],
}

struct Cands {
    all: Cand,
    entry: Cand,
    root: Cand,
}

/// Borrowed column references for one candidate set, so decoding a row is a pointer walk.
struct CandCols<'a> {
    n: &'a Int64Array,
    errors: &'a Int64Array,
    sat: &'a Int64Array,
    tol: &'a Int64Array,
    fru: &'a Int64Array,
    h4: &'a Int64Array,
    h5: &'a Int64Array,
    dsum: &'a Int64Array,
    dcnt: &'a Int64Array,
    le: Vec<&'a Int64Array>,
}

impl<'a> CandCols<'a> {
    fn read(
        b: &'a arrow::record_batch::RecordBatch,
        suffix: &str,
    ) -> Result<CandCols<'a>, PhotonError> {
        let mut le = Vec::with_capacity(LATENCY_BOUNDS_NS.len());
        for i in 0..LATENCY_BOUNDS_NS.len() {
            le.push(i64_col(b, &format!("le{i}{suffix}"))?);
        }
        Ok(CandCols {
            n: i64_col(b, &format!("n{suffix}"))?,
            errors: i64_col(b, &format!("errors{suffix}"))?,
            sat: i64_col(b, &format!("sat{suffix}"))?,
            tol: i64_col(b, &format!("tol{suffix}"))?,
            fru: i64_col(b, &format!("fru{suffix}"))?,
            h4: i64_col(b, &format!("h4{suffix}"))?,
            h5: i64_col(b, &format!("h5{suffix}"))?,
            dsum: i64_col(b, &format!("dsum{suffix}"))?,
            dcnt: i64_col(b, &format!("dcnt{suffix}"))?,
            le,
        })
    }

    fn read_row(&self, i: usize) -> Cand {
        let mut out = Cand {
            n: nonneg_u64(self.n, i),
            errors: nonneg_u64(self.errors, i),
            sat: nonneg_u64(self.sat, i),
            tol: nonneg_u64(self.tol, i),
            fru: nonneg_u64(self.fru, i),
            h4: nonneg_u64(self.h4, i),
            h5: nonneg_u64(self.h5, i),
            dsum: if self.dsum.is_null(i) {
                0
            } else {
                self.dsum.value(i)
            },
            dcnt: nonneg_u64(self.dcnt, i),
            le: [0; LATENCY_BOUNDS_NS.len()],
        };
        for (slot, col) in out.le.iter_mut().zip(&self.le) {
            *slot = nonneg_u64(col, i);
        }
        out
    }
}

fn str_col<'a>(
    b: &'a arrow::record_batch::RecordBatch,
    name: &str,
) -> Result<&'a StringArray, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| PhotonError::Query(format!("rollup column `{name}` missing or not Utf8")))
}

fn i64_col<'a>(
    b: &'a arrow::record_batch::RecordBatch,
    name: &str,
) -> Result<&'a Int64Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| PhotonError::Query(format!("rollup column `{name}` missing or not Int64")))
}

fn nonneg_u64(col: &Int64Array, i: usize) -> u64 {
    if col.is_null(i) {
        0
    } else {
        col.value(i).max(0) as u64
    }
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

    const KIND_SERVER: i32 = 2;
    const KIND_CLIENT: i32 = 3;
    const MS: i64 = 1_000_000;

    /// `service.name` and `deployment.environment` promoted (the shipped default since Task 3);
    /// `tenant` deliberately left in the attributes map, which is where federation ingest puts it.
    fn schema() -> SpanSchema {
        SpanSchema::new(&["service.name".into(), "deployment.environment".into()])
    }

    fn cols() -> RollupCols {
        let promoted = vec!["service.name".to_string(), ENVIRONMENT.to_string()];
        RollupCols {
            environment: promoted_or_map(&promoted, ENVIRONMENT, &AttrKeys::Unknown),
            tenant: promoted_or_map(&promoted, "tenant", &AttrKeys::Unknown),
            http_status: http_status_expr(false, false, &AttrKeys::Unknown),
        }
    }

    struct Span {
        service: &'static str,
        env: Option<&'static str>,
        name: &'static str,
        start: i64,
        dur: Option<i64>,
        kind: Option<i32>,
        status: Option<i32>,
        tenant: Option<&'static str>,
    }

    impl Default for Span {
        fn default() -> Self {
            Span {
                service: "svc",
                env: Some("prod"),
                name: "GET /x",
                start: 0,
                dur: Some(10 * MS),
                kind: Some(KIND_SERVER),
                status: Some(1),
                tenant: None,
            }
        }
    }

    impl Span {
        fn record(&self, i: usize) -> SpanRecord {
            let mut attributes = BTreeMap::new();
            attributes.insert("service.name".into(), self.service.to_string());
            if let Some(e) = self.env {
                attributes.insert(ENVIRONMENT.into(), e.to_string());
            }
            if let Some(t) = self.tenant {
                attributes.insert("tenant".into(), t.to_string());
            }
            SpanRecord {
                trace_id: "t1".into(),
                span_id: format!("s{i}"),
                // Non-root by default so the cascade's rule 2 never fires by accident; the
                // trace-root tests set this to None explicitly.
                parent_span_id: Some("parent".into()),
                name: Some(self.name.into()),
                kind: self.kind,
                start_time_nanos: self.start,
                duration_nanos: self.dur,
                status_code: self.status,
                attributes,
                ..Default::default()
            }
        }
    }

    async fn df_of(spans: &[Span]) -> DataFrame {
        let schema = schema();
        let mut b = SpanBatchBuilder::new(&schema);
        for (i, s) in spans.iter().enumerate() {
            b.append(&s.record(i));
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

    /// `rollup_over` with no thresholds and a 500ms default Apdex T.
    async fn roll(
        spans: &[Span],
        start: i64,
        end: i64,
        bucket_ns: i64,
        keep: Option<&HashSet<String>>,
    ) -> Vec<RollupRow> {
        rollup_over(
            df_of(spans).await,
            cols().tenant.clone().is_null(),
            &cols(),
            start,
            end,
            bucket_ns,
            &HashMap::new(),
            500,
            keep,
            None,
        )
        .await
        .unwrap()
    }

    fn find<'a>(
        rows: &'a [RollupRow],
        service: &str,
        env: Option<&str>,
        ts: i64,
    ) -> Option<&'a RollupRow> {
        rows.iter()
            .find(|r| r.service == service && r.environment.as_deref() == env && r.ts == ts)
    }

    #[tokio::test]
    async fn emits_one_series_per_service_and_environment() {
        let rows = roll(
            &[
                Span {
                    service: "api",
                    env: Some("prod"),
                    ..Default::default()
                },
                Span {
                    service: "api",
                    env: Some("prod"),
                    ..Default::default()
                },
                Span {
                    service: "api",
                    env: Some("staging"),
                    ..Default::default()
                },
                Span {
                    service: "worker",
                    env: Some("prod"),
                    ..Default::default()
                },
                // No `deployment.environment` at all — its own series, not folded into prod.
                Span {
                    service: "api",
                    env: None,
                    ..Default::default()
                },
            ],
            0,
            60_000_000_000,
            60_000_000_000,
            None,
        )
        .await;

        assert_eq!(
            rows.len(),
            4,
            "one row per (service, environment): {rows:#?}"
        );
        assert_eq!(find(&rows, "api", Some("prod"), 0).unwrap().count, 2);
        assert_eq!(find(&rows, "api", Some("staging"), 0).unwrap().count, 1);
        assert_eq!(find(&rows, "worker", Some("prod"), 0).unwrap().count, 1);
        assert_eq!(find(&rows, "api", None, 0).unwrap().count, 1);
    }

    #[tokio::test]
    async fn tenant_spans_are_excluded_so_central_never_double_counts() {
        let rows = roll(
            &[
                Span {
                    service: "api",
                    ..Default::default()
                },
                Span {
                    service: "api",
                    tenant: Some("us-east"),
                    ..Default::default()
                },
                Span {
                    service: "api",
                    tenant: Some("eu-west"),
                    ..Default::default()
                },
            ],
            0,
            60_000_000_000,
            60_000_000_000,
            None,
        )
        .await;

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].count, 1,
            "only the local span counts — the two mirrored ones are rolled up by their own Site"
        );
    }

    #[tokio::test]
    async fn buckets_are_placed_on_the_grid_the_caller_chose() {
        let minute = 60_000_000_000_i64;
        let rows = roll(
            &[
                Span {
                    start: 0,
                    ..Default::default()
                },
                Span {
                    start: minute + 5,
                    ..Default::default()
                },
                Span {
                    start: minute + 9,
                    ..Default::default()
                },
                // Exactly on `end` — clamps down into the last bucket rather than overflowing.
                Span {
                    start: 3 * minute,
                    ..Default::default()
                },
            ],
            0,
            3 * minute,
            minute,
            None,
        )
        .await;

        assert_eq!(rows.len(), 3, "3 buckets, only the non-empty ones emitted");
        assert_eq!(find(&rows, "svc", Some("prod"), 0).unwrap().count, 1);
        assert_eq!(find(&rows, "svc", Some("prod"), minute).unwrap().count, 2);
        assert_eq!(
            find(&rows, "svc", Some("prod"), 2 * minute).unwrap().count,
            1
        );
    }

    #[tokio::test]
    async fn latency_buckets_are_non_cumulative_and_carry_the_sum() {
        let rows = roll(
            &[
                // bounds are [1ms, 2, 5, 10, 20, 50, 100, 200, 500, 1s, 2s, 5s, 10s,
                //             30s, 1m, 2m, 5m, 10m, 30m]
                Span {
                    dur: Some(500_000),
                    ..Default::default()
                }, // <= 1ms  -> idx 0
                Span {
                    dur: Some(3 * MS),
                    ..Default::default()
                }, // <= 5ms  -> idx 2
                Span {
                    dur: Some(3 * MS),
                    ..Default::default()
                }, // <= 5ms  -> idx 2
                Span {
                    dur: Some(120_000 * MS),
                    ..Default::default()
                }, // <= 2m   -> idx 15
                Span {
                    dur: Some(3_600_000 * MS),
                    ..Default::default()
                }, // > 30m   -> +Inf
            ],
            0,
            60_000_000_000,
            60_000_000_000,
            None,
        )
        .await;

        let r = &rows[0];
        assert_eq!(r.latency_buckets.len(), LATENCY_BOUNDS_NS.len() + 1);
        assert_eq!(r.latency_buckets[0], 1);
        assert_eq!(r.latency_buckets[1], 0, "nothing between 1ms and 2ms");
        assert_eq!(r.latency_buckets[2], 2);
        assert_eq!(
            r.latency_buckets[15], 1,
            "a two-minute span is measured, not overflowed — the ladder reaches past 10s, and a \
             service whose real p50 is minutes used to read as exactly 10s because it did not"
        );
        assert_eq!(*r.latency_buckets.last().unwrap(), 1, "+Inf overflow");
        assert_eq!(
            r.latency_buckets.iter().sum::<u64>(),
            r.duration_count,
            "every measured span lands in exactly one bucket"
        );
        assert_eq!(r.duration_count, r.count, "nothing was left open here");
        assert_eq!(
            r.duration_sum as i64,
            500_000 + 3 * MS + 3 * MS + 120_000 * MS + 3_600_000 * MS
        );
    }

    #[tokio::test]
    async fn a_null_duration_span_is_counted_but_lands_in_no_latency_bucket() {
        let rows = roll(
            &[
                Span {
                    dur: Some(3 * MS),
                    ..Default::default()
                },
                Span {
                    dur: None,
                    ..Default::default()
                },
            ],
            0,
            60_000_000_000,
            60_000_000_000,
            None,
        )
        .await;

        let r = &rows[0];
        assert_eq!(r.count, 2, "an open span is still a request");
        assert_eq!(r.duration_count, 1);
        assert_eq!(
            r.latency_buckets.iter().sum::<u64>(),
            1,
            "it is unmeasured, not slow — it must not land in +Inf"
        );
        assert_eq!(*r.latency_buckets.last().unwrap(), 0);
    }

    #[tokio::test]
    async fn basis_is_decided_once_per_series_not_per_bucket() {
        let minute = 60_000_000_000_i64;
        let rows = roll(
            &[
                // Bucket 0 has a SERVER span, so the series' basis is Entry.
                Span {
                    start: 0,
                    kind: Some(KIND_SERVER),
                    ..Default::default()
                },
                // Bucket 1 has only CLIENT spans. Resolved per bucket it would fall to AllSpans
                // and report 2 requests; resolved per series it correctly reports 0.
                Span {
                    start: minute + 1,
                    kind: Some(KIND_CLIENT),
                    ..Default::default()
                },
                Span {
                    start: minute + 2,
                    kind: Some(KIND_CLIENT),
                    ..Default::default()
                },
            ],
            0,
            2 * minute,
            minute,
            None,
        )
        .await;

        assert!(rows.iter().all(|r| r.basis == RedBasis::Entry));
        assert_eq!(find(&rows, "svc", Some("prod"), 0).unwrap().count, 1);
        let second = find(&rows, "svc", Some("prod"), minute).unwrap();
        assert_eq!(second.count, 0, "no requests entered in this bucket");
        assert_eq!(second.span_count, 2, "but two spans were recorded");
    }

    #[tokio::test]
    async fn falls_back_to_all_spans_when_the_service_has_no_entry_or_root_spans() {
        let rows = roll(
            &[
                Span {
                    kind: Some(KIND_CLIENT),
                    ..Default::default()
                },
                Span {
                    kind: None,
                    ..Default::default()
                },
            ],
            0,
            60_000_000_000,
            60_000_000_000,
            None,
        )
        .await;

        assert_eq!(rows[0].basis, RedBasis::AllSpans);
        assert_eq!(rows[0].count, 2);
    }

    #[tokio::test]
    async fn discovery_keeps_the_busiest_operations_per_service_not_globally() {
        // `loud` is busy enough that a global top-2 would take both of its operations and leave
        // `quiet` with none. Top-N is per (service, environment), so each keeps its own busiest.
        let mut spans = Vec::new();
        for _ in 0..10 {
            spans.push(Span {
                service: "loud",
                name: "GET /a",
                ..Default::default()
            });
        }
        for _ in 0..8 {
            spans.push(Span {
                service: "loud",
                name: "GET /b",
                ..Default::default()
            });
        }
        spans.push(Span {
            service: "loud",
            name: "GET /c",
            ..Default::default()
        });
        spans.push(Span {
            service: "quiet",
            name: "GET /z",
            ..Default::default()
        });

        let (keep, folded, truncated) = discover_top_operations(
            df_of(&spans).await,
            cols().tenant.clone().is_null(),
            &cols(),
            &HashMap::new(),
            500,
            2,
        )
        .await
        .unwrap();

        assert!(!truncated);
        assert!(keep.contains(&operation_key("loud", None, Some("prod"), "GET /a")));
        assert!(keep.contains(&operation_key("loud", None, Some("prod"), "GET /b")));
        assert!(
            keep.contains(&operation_key("quiet", None, Some("prod"), "GET /z")),
            "a quiet service still gets its own top-N slot"
        );
        assert!(!keep.contains(&operation_key("loud", None, Some("prod"), "GET /c")));
        assert_eq!(folded, 1, "only `loud`'s third operation folds");
    }

    #[tokio::test]
    async fn the_operation_tail_folds_into_other_and_still_adds_up() {
        let mut spans = vec![
            Span {
                name: "GET /a",
                ..Default::default()
            },
            Span {
                name: "GET /a",
                ..Default::default()
            },
        ];
        // Nine id-carrying one-hit operations — the shape that would otherwise explode the store.
        for name in [
            "GET /o1", "GET /o2", "GET /o3", "GET /o4", "GET /o5", "GET /o6", "GET /o7", "GET /o8",
            "GET /o9",
        ] {
            spans.push(Span {
                name,
                status: Some(2),
                ..Default::default()
            });
        }

        let mut keep = HashSet::new();
        keep.insert(operation_key("svc", None, Some("prod"), "GET /a"));
        let ops = roll(&spans, 0, 60_000_000_000, 60_000_000_000, Some(&keep)).await;

        assert_eq!(ops.len(), 2, "one kept operation plus `__other__`");
        let top = ops
            .iter()
            .find(|r| r.operation.as_deref() == Some("GET /a"))
            .unwrap();
        let other = ops
            .iter()
            .find(|r| r.operation.as_deref() == Some(OTHER_OPERATION))
            .unwrap();
        assert_eq!(top.count, 2);
        assert_eq!(other.count, 9, "the whole tail, not a truncated sample");
        assert_eq!(
            other.error_count, 9,
            "`__other__` aggregates its own errors too"
        );

        // The property that makes the operation table trustworthy: the levels agree.
        let services = roll(&spans, 0, 60_000_000_000, 60_000_000_000, None).await;
        assert_eq!(
            top.count + other.count,
            services[0].count,
            "operations must sum to the service total"
        );
    }

    #[tokio::test]
    async fn an_operation_inherits_its_service_basis_instead_of_resolving_its_own() {
        // An entry-based service that also emits CLIENT spans under their own operation name.
        // Left to its own cascade, `SELECT users` has no entry spans and no roots, so it would
        // fall through to AllSpans and report 2 requests — outbound calls counted as traffic
        // served. Inheriting `Entry` reports 0, and the operations still sum to the service.
        let spans = [
            Span {
                name: "GET /a",
                kind: Some(KIND_SERVER),
                ..Default::default()
            },
            Span {
                name: "GET /a",
                kind: Some(KIND_SERVER),
                ..Default::default()
            },
            Span {
                name: "SELECT users",
                kind: Some(KIND_CLIENT),
                ..Default::default()
            },
            Span {
                name: "SELECT users",
                kind: Some(KIND_CLIENT),
                ..Default::default()
            },
        ];
        let services = roll(&spans, 0, 60_000_000_000, 60_000_000_000, None).await;
        assert_eq!(services[0].basis, RedBasis::Entry);
        assert_eq!(services[0].count, 2);

        let mut keep = HashSet::new();
        keep.insert(operation_key("svc", None, Some("prod"), "GET /a"));
        keep.insert(operation_key("svc", None, Some("prod"), "SELECT users"));
        let basis = basis_map(&services);
        let ops = rollup_over(
            df_of(&spans).await,
            cols().tenant.clone().is_null(),
            &cols(),
            0,
            60_000_000_000,
            60_000_000_000,
            &HashMap::new(),
            500,
            Some(&keep),
            Some(&basis),
        )
        .await
        .unwrap();

        let client = ops
            .iter()
            .find(|r| r.operation.as_deref() == Some("SELECT users"))
            .unwrap();
        assert_eq!(client.basis, RedBasis::Entry);
        assert_eq!(client.count, 0, "an outbound call is not a request served");
        assert_eq!(client.span_count, 2, "the work it did is still reported");

        let total: u64 = ops.iter().map(|r| r.count).sum();
        assert_eq!(total, services[0].count, "operations sum to their service");
    }

    #[tokio::test]
    async fn an_empty_keep_set_folds_every_operation() {
        let ops = roll(
            &[
                Span {
                    name: "GET /a",
                    ..Default::default()
                },
                Span {
                    name: "GET /b",
                    ..Default::default()
                },
            ],
            0,
            60_000_000_000,
            60_000_000_000,
            Some(&HashSet::new()),
        )
        .await;

        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operation.as_deref(), Some(OTHER_OPERATION));
        assert_eq!(ops[0].count, 2);
    }

    #[test]
    fn decode_latency_differences_cumulative_counts() {
        let mut le = [0u64; LATENCY_BOUNDS_NS.len()];
        le[0] = 1;
        le[1] = 1;
        le[2] = 4;
        for slot in le.iter_mut().skip(3) {
            *slot = 4;
        }
        let out = decode_latency(&le, 6);
        assert_eq!(out[0], 1);
        assert_eq!(out[1], 0);
        assert_eq!(out[2], 3);
        assert_eq!(*out.last().unwrap(), 2, "+Inf takes the rest");
        assert_eq!(out.iter().sum::<u64>(), 6);
    }

    #[test]
    fn operation_keys_do_not_collide_across_services() {
        // Naive concatenation would make ("a", "b\u{1}c") and ("a\u{1}b", "c") the same key.
        assert_ne!(
            operation_key("a", None, Some("b"), "c"),
            operation_key("a\u{1}b", None, None, "c")
        );
        assert_ne!(
            operation_key("api", None, None, "GET /x"),
            operation_key("api", None, Some(""), "GET /x\u{1}")
        );
    }
}

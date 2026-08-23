//! Reading the Backend RED numbers back off the `photon.service.*` / `photon.operation.*` rollup
//! instead of re-scanning raw spans (ADR-0002; the writer is `photon-server/src/rollup/`).
//!
//! The rollup is a **shortcut, never the source of truth**. Every read path here has a raw-span
//! fallback, and [`rollup_split`] is deliberately conservative: falling back costs latency,
//! answering off the rollup when it cannot really answer costs correctness. When in doubt, say no.
//!
//! Three things make a request ineligible, and only the first is in the plan:
//!
//! 1. **The window is not fully published.** The rollup writes a bucket once, ten minutes after it
//!    closes. Reading a window whose tail has not been rolled up yet would show a service's traffic
//!    falling off a cliff at `now - lag`.
//! 2. **The window predates coverage.** A rollup turned on yesterday has nothing to say about last
//!    week. Without the lower bound the answer would be zeros, which reads exactly like an outage.
//! 3. **The filter asks something the rollup cannot answer.** `/api/red` takes a span-grammar
//!    query, and most of that grammar has no rollup equivalent: the rollup carries `service.name`,
//!    `deployment.environment`, `operation` and `tenant`, and nothing else. `status_code:2`, a body
//!    substring, a `duration>1s` comparison — none of those survive aggregation. Only a query built
//!    entirely from the four carried keys can be served.

use std::collections::{BTreeMap, HashMap};

use photon_core::metric_agg::Agg;
use photon_core::query::{parse, MetricFieldResolver, MetricResolvedQuery, Query, TermKind};
use photon_core::PhotonError;
use photon_query::{
    MetricSeriesRequest, MetricsQueryEngine, RedBasis, RedBucket, RedGroup, RedRow, RedSeriesKey,
    SeriesResult, SpanQueryRequest, SparkPoint,
};

use crate::AppState;

/// Where a set of RED rows actually came from. Reported on the wire so a wrong number is
/// attributable instead of mysterious.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedSource {
    Rollup,
    RawSpans,
}

impl RedSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RedSource::Rollup => "rollup",
            RedSource::RawSpans => "raw",
        }
    }
}

/// What the rollup has published, as a half-open nanosecond range `[first_bucket, published_through)`.
/// `None` means the rollup has never published anything (disabled, or a fresh install).
pub type RollupCoverage = Option<(i64, i64)>;

/// The seam `photon-server` implements over the rollup's SQLite watermark. `photon-api` cannot
/// depend on `photon-server`, and the watermark is the only thing it needs from it.
#[async_trait::async_trait]
pub trait RollupStatus: Send + Sync {
    /// `[first published bucket start, end of the newest published bucket)`, or `None` if nothing
    /// has been published.
    async fn coverage(&self) -> Result<RollupCoverage, PhotonError>;
}

/// The only attribute keys a rollup series carries. A query touching anything else cannot be
/// answered from it — see the module header.
const CARRIED_KEYS: [&str; 4] = [
    "service.name",
    "deployment.environment",
    "operation",
    "tenant",
];

/// The rollup's bucket width. Restated from `photon-core`'s `RollupConfig::BUCKET_SECS` as nanos:
/// a read finer than one bucket cannot come off the rollup, because the rollup does not have it.
pub(crate) const BUCKET_NS: i64 = 60 * 1_000_000_000;

/// Can the rollup answer this request?
///
/// `bucket_ns` is the requested resolution for a time-series read, or `None` for the un-bucketed
/// `/api/red` table (which only needs totals, so any resolution serves).
#[cfg(test)]
pub(crate) fn rollup_eligible(
    query: &str,
    start_ns: i64,
    end_ns: i64,
    bucket_ns: Option<i64>,
    coverage: RollupCoverage,
) -> bool {
    rollup_split(query, start_ns, end_ns, bucket_ns, coverage).is_some()
}

/// How to answer `[start_ns, end_ns]`: `Some(through)` to read `[start_ns, through]` off the
/// rollup and `(through, end_ns]` off raw spans, or `None` to read the whole thing raw.
///
/// The tail is what this function exists for. A rollup bucket is published once its minute has
/// closed AND `lag_secs` has passed, so `published_through` always trails `now` by ~11 minutes at
/// the default settings. The rule used to be `end_ns > published_through → ineligible`, which
/// reads as caution and behaves as a switch: every window the UI actually asks for ends at NOW, so
/// the rollup was never once used for a live page. It was built to keep the Backend pages off raw
/// spans and, in normal use, kept them on it — `/api/services/overview` took 3.4 s over 24 hours
/// and 24.8 s over 7 days answering from spans, against 24 ms off the rollup.
///
/// The caution was right about the failure it named, though: reading only the covered prefix and
/// calling it the answer would drop the last eleven minutes, which on a chart is a service's
/// traffic falling off a cliff. So the tail is not dropped, it is read from raw spans — eleven
/// minutes of spans, next to nothing — and merged in.
///
/// Two bounds keep that honest:
///
/// * The prefix must be the bulk of the window ([`MIN_ROLLUP_SHARE`]). Below that the tail carries
///   the answer, and a raw scan of a window that short is fast anyway.
/// * The window must start inside coverage, and the query must only touch keys the rollup carries.
pub(crate) fn rollup_split(
    query: &str,
    start_ns: i64,
    end_ns: i64,
    bucket_ns: Option<i64>,
    coverage: RollupCoverage,
) -> Option<i64> {
    let (first, published_through) = coverage?;
    if start_ns < first {
        return None;
    }
    let through = published_through.min(end_ns);
    if through <= start_ns {
        return None;
    }
    let window = (end_ns - start_ns).max(1) as f64;
    if (through - start_ns) as f64 / window < MIN_ROLLUP_SHARE {
        return None;
    }
    if bucket_ns.is_some_and(|b| b < BUCKET_NS) {
        return None;
    }
    match parse(query) {
        Ok(ast) if query_is_carried(&ast) => Some(through),
        // An unparseable query is the raw path's problem to report, with its byte offset. Silently
        // treating it as "matches everything" here would answer a broken query with real numbers.
        _ => None,
    }
}

/// How much of the window the rollup must cover before it is worth splitting the read in two.
///
/// At 90%, a 24-hour window may leave ~2.4 hours to raw spans and an hour window is answered
/// entirely from spans — which is the right trade: the settle lag is a fixed ~11 minutes, so the
/// share the rollup can cover only gets better as the window widens, and a raw scan of an hour is
/// already fast (166 ms measured). It also bounds how much of a percentile comes from the prefix
/// alone; see `merge_tail`.
const MIN_ROLLUP_SHARE: f64 = 0.9;

/// Every term is a plain (non-negated) equality or existence check on a key the rollup carries.
///
/// Negation is excluded even though a metric filter could express it: `-service.name:api` over the
/// rollup means "every OTHER service's rollup series", which is right, but `-operation:x` at the
/// service level means nothing at all, and the distinction is not worth the footgun. Comparisons
/// and free text are excluded because aggregation destroyed what they range over.
fn query_is_carried(q: &Query) -> bool {
    q.terms.iter().all(|t| {
        if t.negated {
            return false;
        }
        match &t.kind {
            TermKind::Match { field, .. } | TermKind::Exists { field } => {
                CARRIED_KEYS.contains(&field.as_str())
            }
            TermKind::Compare { .. } | TermKind::FreeText { .. } => false,
        }
    })
}

/// Compile an eligible span-grammar query into the equivalent metric filter. Only ever called
/// after [`rollup_split`] said yes, so a resolve failure here is a bug, not user input.
pub(crate) fn metric_filter(
    query: &str,
    promoted: &[String],
) -> Result<Option<MetricResolvedQuery>, PhotonError> {
    if query.trim().is_empty() {
        return Ok(None);
    }
    let ast = parse(query).map_err(|e| PhotonError::Query(format!("rollup filter parse: {e}")))?;
    MetricFieldResolver::new(promoted)
        .resolve(&ast)
        .map(Some)
        .map_err(|e| PhotonError::Query(format!("rollup filter resolve: {}", e.message)))
}

/// The metric family for a grouping level. They are separate names on purpose — see
/// `docs/subsystems/rollup.md`.
fn family(group: RedGroup) -> &'static str {
    match group {
        RedGroup::Service => "photon.service.",
        RedGroup::Operation => "photon.operation.",
    }
}

/// The labels each level is keyed by. `tenant` rides along at the service level so a Central
/// install reading its Sites' shipped rollups still labels rows by Site, exactly as the raw span
/// path does.
fn group_by(group: RedGroup) -> Vec<String> {
    match group {
        RedGroup::Service => vec![
            "service.name".into(),
            "tenant".into(),
            "deployment.environment".into(),
        ],
        RedGroup::Operation => vec!["service.name".into(), "operation".into()],
    }
}

/// Pick a source and return the rows. The single seam both `/api/red` and
/// `/api/services/:service/timeseries` go through, so the two can never disagree about when the
/// rollup is trustworthy.
///
/// `bucket_ns` is the requested resolution, or `None` for an un-bucketed total.
pub(crate) async fn red_rows(
    state: &AppState,
    query: &str,
    req: SpanQueryRequest,
    bucket_ns: Option<i64>,
    group: RedGroup,
    thresholds: &HashMap<String, u32>,
    default_ms: u32,
) -> Result<(Vec<RedRow>, RedSource), PhotonError> {
    let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);
    let coverage = match &state.rollup {
        Some(status) => status.coverage().await.unwrap_or(None),
        None => None,
    };
    if let Some(through) = rollup_split(query, start, end, bucket_ns, coverage) {
        // A rollup read that errors is not fatal: the raw path can still answer, and a Backend
        // page that works slowly beats one that 500s because a shortcut broke.
        match red_rows_from_rollup(&state.metrics_query, query, start, through, group).await {
            Ok(rows) => {
                if through >= end {
                    return Ok((rows, RedSource::Rollup));
                }
                // The unsettled tail, from spans. A failure here loses the tail, not the answer —
                // so report the prefix rather than throwing away a good 23-hour read over the last
                // eleven minutes.
                let mut tail_req = req.clone();
                tail_req.start_ts_nanos = through + 1;
                match state
                    .span_query
                    .red_metrics(tail_req, group, thresholds, default_ms)
                    .await
                {
                    Ok(tail) => return Ok((merge_tail(rows, tail), RedSource::Rollup)),
                    Err(e) => {
                        eprintln!("photon-api: rollup tail read failed ({e}) — prefix only");
                        return Ok((rows, RedSource::Rollup));
                    }
                }
            }
            Err(e) => eprintln!("photon-api: rollup read failed ({e}) — falling back to raw spans"),
        }
    }
    let rows = state
        .span_query
        .red_metrics(req, group, thresholds, default_ms)
        .await?;
    Ok((rows, RedSource::RawSpans))
}

/// Fold the raw-span tail into the rollup prefix, one series at a time.
///
/// Counters add, because they are counts of disjoint sets of spans — the prefix and the tail share
/// no span. Percentiles do NOT: p99 over the prefix and p99 over the tail cannot be combined into
/// p99 over both, and there is no third number that would let them be. The prefix's percentiles
/// stand, which is why [`MIN_ROLLUP_SHARE`] exists — they describe at least 90% of the window, and
/// the alternative (raw-scanning the whole window for an exact percentile) is the cost this whole
/// path removes. A series the prefix never saw keeps the tail's percentiles, since they are then
/// the only ones there are.
///
/// `basis` comes from the prefix for the same reason it is resolved once per series rather than
/// per bucket in `red_rollup`: a series that reported Entry over 23 hours and AllSpans over the
/// last eleven minutes is one series, not two.
fn merge_tail(prefix: Vec<RedRow>, tail: Vec<RedRow>) -> Vec<RedRow> {
    let key = |r: &RedRow| {
        (
            r.service.clone(),
            r.tenant.clone(),
            r.environment.clone(),
            r.operation.clone(),
        )
    };
    let mut by_key: BTreeMap<_, RedRow> = prefix.into_iter().map(|r| (key(&r), r)).collect();
    for t in tail {
        match by_key.get_mut(&key(&t)) {
            Some(p) => {
                p.count += t.count;
                p.span_count += t.span_count;
                p.error_count += t.error_count;
                p.satisfied += t.satisfied;
                p.tolerating += t.tolerating;
                p.frustrated += t.frustrated;
                p.http_4xx += t.http_4xx;
                p.http_5xx += t.http_5xx;
            }
            None => {
                by_key.insert(key(&t), t);
            }
        }
    }
    let mut rows: Vec<RedRow> = by_key.into_values().collect();
    // Busiest first, matching `red_over`'s ordering — the merge can reorder by count.
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.service.cmp(&b.service))
            .then_with(|| a.environment.cmp(&b.environment))
            .then_with(|| a.operation.cmp(&b.operation))
    });
    rows
}

/// One RED row per series, read off the rollup.
pub(crate) async fn red_rows_from_rollup(
    engine: &MetricsQueryEngine,
    query: &str,
    start_ns: i64,
    end_ns: i64,
    group: RedGroup,
) -> Result<Vec<RedRow>, PhotonError> {
    let filter = metric_filter(query, engine.promoted_attributes())?;
    let fam = family(group);
    let keys = group_by(group);

    // One query per metric, all concurrent: each is a cheap scan (metric_name leads the sort key),
    // and running them in sequence would put eleven round trips on a page load.
    let counters = [
        "requests",
        "spans",
        "errors",
        "http_4xx",
        "http_5xx",
        "apdex.satisfied",
        "apdex.tolerating",
        "apdex.frustrated",
    ];
    let counter_futs = counters.iter().map(|name| {
        read_one(
            engine,
            format!("{fam}{name}"),
            Agg::Sum,
            &filter,
            &keys,
            start_ns,
            end_ns,
        )
    });
    let pct_futs = [Agg::P50, Agg::P90, Agg::P99].into_iter().map(|agg| {
        read_one(
            engine,
            format!("{fam}{}", photon_query::LATENCY_METRIC),
            agg,
            &filter,
            &keys,
            start_ns,
            end_ns,
        )
    });
    let basis_fut = read_one(
        engine,
        "photon.service.basis".to_string(),
        Agg::Last,
        &filter,
        &keys,
        start_ns,
        end_ns,
    );

    let (counter_maps, pct_maps, basis_map) = futures::try_join!(
        futures::future::try_join_all(counter_futs),
        futures::future::try_join_all(pct_futs),
        basis_fut,
    )?;

    // Union of every series key seen, so a series that reported errors but (impossibly) no
    // requests still produces a row rather than vanishing.
    let mut keys_seen: Vec<SeriesKey> = counter_maps
        .iter()
        .chain(pct_maps.iter())
        .flat_map(|m| m.keys().cloned())
        .collect();
    keys_seen.sort();
    keys_seen.dedup();

    let get = |m: &BTreeMap<SeriesKey, f64>, k: &SeriesKey| -> u64 {
        m.get(k).copied().unwrap_or(0.0).max(0.0).round() as u64
    };
    let get_ns = |m: &BTreeMap<SeriesKey, f64>, k: &SeriesKey| -> i64 {
        m.get(k).copied().unwrap_or(0.0).max(0.0).round() as i64
    };

    let mut rows: Vec<RedRow> = keys_seen
        .iter()
        .map(|k| RedRow {
            service: k.service.clone(),
            tenant: match group {
                RedGroup::Service => Some(k.tenant.clone().unwrap_or_default()),
                RedGroup::Operation => None,
            },
            environment: k.environment.clone(),
            operation: k.operation.clone(),
            count: get(&counter_maps[0], k),
            span_count: get(&counter_maps[1], k),
            // The gauge is only published when the basis is NOT entry spans, so its absence is
            // the common case and means exactly `Entry`.
            basis: basis_from(basis_map.get(k).copied()),
            error_count: get(&counter_maps[2], k),
            http_4xx: get(&counter_maps[3], k),
            http_5xx: get(&counter_maps[4], k),
            satisfied: get(&counter_maps[5], k),
            tolerating: get(&counter_maps[6], k),
            frustrated: get(&counter_maps[7], k),
            p50: get_ns(&pct_maps[0], k),
            p90: get_ns(&pct_maps[1], k),
            p99: get_ns(&pct_maps[2], k),
        })
        .collect();

    // Busiest first, matching `red_over`'s ordering so the two sources present the same table.
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.service.cmp(&b.service))
            .then_with(|| a.environment.cmp(&b.environment))
            .then_with(|| a.operation.cmp(&b.operation))
    });
    Ok(rows)
}

/// Pick a source for the bucketed, single-service read behind
/// `GET /api/services/:service/timeseries`.
///
/// Same eligibility rule as [`red_rows`], plus the one that only applies here: the requested
/// resolution must be at least one rollup bucket wide. The rollup simply does not hold anything
/// finer, and interpolating 60s buckets down to 10s would invent a shape.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn red_buckets(
    state: &AppState,
    query: &str,
    req: SpanQueryRequest,
    buckets: usize,
    threshold_ms: u32,
) -> Result<(Vec<RedBucket>, RedSource), PhotonError> {
    let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);
    let width = bucket_width(start, end, buckets);
    let coverage = match &state.rollup {
        Some(status) => status.coverage().await.unwrap_or(None),
        None => None,
    };
    if let Some(through) = rollup_split(query, start, end, Some(width), coverage) {
        match red_buckets_from_rollup(&state.metrics_query, query, start, end, buckets).await {
            Ok(mut rows) => {
                // The rollup read fills the whole grid, zeroing buckets it has nothing for. Those
                // trailing zeros are the unsettled tail, and left alone they draw the service's
                // traffic falling off a cliff. Overwrite them from spans — see `rollup_split`.
                match tail_buckets(state, query, &req, start, end, through, width, threshold_ms)
                    .await
                {
                    Ok(Some((from, tail))) => splice_tail(&mut rows, from, tail),
                    Ok(None) => {}
                    Err(e) => eprintln!("photon-api: rollup tail read failed ({e}) — prefix only"),
                }
                return Ok((rows, RedSource::Rollup));
            }
            Err(e) => eprintln!("photon-api: rollup read failed ({e}) — falling back to raw spans"),
        }
    }
    let rows = state
        .span_query
        .red_timeseries(req, start, end, buckets, threshold_ms)
        .await?;
    Ok((rows, RedSource::RawSpans))
}

/// The unsettled tail of a bucketed read, as raw-span buckets on the SAME grid, plus the grid
/// index they start at. `None` when the rollup already covers the whole window.
///
/// `through` is snapped DOWN to a grid boundary first, so the tail's buckets line up with the
/// prefix's instead of straddling them — a half-bucket offset would draw a step in the chart at
/// the seam that no service actually served.
#[allow(clippy::too_many_arguments)]
async fn tail_buckets(
    state: &AppState,
    _query: &str,
    req: &SpanQueryRequest,
    start: i64,
    end: i64,
    through: i64,
    width: i64,
    threshold_ms: u32,
) -> Result<Option<(usize, Vec<RedBucket>)>, PhotonError> {
    if through >= end {
        return Ok(None);
    }
    let from_idx = ((through - start) / width).max(0);
    let from_ts = start + from_idx * width;
    let n = (((end - from_ts) + width - 1) / width).max(1) as usize;
    let mut tail_req = req.clone();
    tail_req.start_ts_nanos = from_ts;
    let rows = state
        .span_query
        .red_timeseries(tail_req, from_ts, end, n, threshold_ms)
        .await?;
    Ok(Some((from_idx as usize, rows)))
}

/// Overwrite `rows[from..]` with `tail`, keeping each bucket's own timestamp from the grid.
fn splice_tail(rows: &mut [RedBucket], from: usize, tail: Vec<RedBucket>) {
    for (i, t) in tail.into_iter().enumerate() {
        let Some(slot) = rows.get_mut(from + i) else {
            break;
        };
        let ts = slot.ts;
        *slot = t;
        slot.ts = ts;
    }
}

/// The width of one of `buckets` equal-width buckets spanning `[start, end]` — mirrors
/// `red_timeseries::bucket_width` so the two sources agree on the grid.
fn bucket_width(start: i64, end: i64, buckets: usize) -> i64 {
    ((end - start).max(1) / buckets.max(1) as i64).max(1)
}

/// A zero-filled series of `buckets` `RedBucket`s, read off the rollup.
///
/// Zero-filled, not sparse: the chart needs its x-axis to keep its shape, and a quiet minute is a
/// real fact about the service. `red_timeseries` makes the same promise.
async fn red_buckets_from_rollup(
    engine: &MetricsQueryEngine,
    query: &str,
    start_ns: i64,
    end_ns: i64,
    buckets: usize,
) -> Result<Vec<RedBucket>, PhotonError> {
    let filter = metric_filter(query, engine.promoted_attributes())?;
    let width = bucket_width(start_ns, end_ns, buckets);

    // No `group_by`: the caller already scoped `query` to one service, so this is a single series
    // — exactly the shape `red_timeseries` produces.
    let counters = [
        "requests",
        "spans",
        "errors",
        "http_4xx",
        "http_5xx",
        "apdex.satisfied",
        "apdex.tolerating",
        "apdex.frustrated",
    ];
    let counter_futs = counters.iter().map(|name| {
        read_bucketed(
            engine,
            format!("photon.service.{name}"),
            Agg::Sum,
            &filter,
            start_ns,
            end_ns,
            buckets,
        )
    });
    let pct_futs = [Agg::P50, Agg::P90, Agg::P99].into_iter().map(|agg| {
        read_bucketed(
            engine,
            format!("photon.service.{}", photon_query::LATENCY_METRIC),
            agg,
            &filter,
            start_ns,
            end_ns,
            buckets,
        )
    });
    let basis_fut = read_bucketed(
        engine,
        "photon.service.basis".to_string(),
        Agg::Last,
        &filter,
        start_ns,
        end_ns,
        buckets,
    );
    let (counters, pcts, basis) = futures::try_join!(
        futures::future::try_join_all(counter_futs),
        futures::future::try_join_all(pct_futs),
        basis_fut,
    )?;

    // One basis for the whole series, matching `red_timeseries`: a series that changed basis
    // mid-chart would change what its own rate means halfway across.
    let series_basis = basis_from(basis.iter().copied().find(|v| *v > 0.0));

    Ok((0..buckets)
        .map(|i| {
            let n = |c: &Vec<f64>| c.get(i).copied().unwrap_or(0.0).max(0.0).round() as u64;
            let ns = |c: &Vec<f64>| c.get(i).copied().unwrap_or(0.0).max(0.0).round() as i64;
            RedBucket {
                ts: start_ns + i as i64 * width,
                count: n(&counters[0]),
                span_count: n(&counters[1]),
                basis: series_basis,
                error_count: n(&counters[2]),
                http_4xx: n(&counters[3]),
                http_5xx: n(&counters[4]),
                satisfied: n(&counters[5]),
                tolerating: n(&counters[6]),
                frustrated: n(&counters[7]),
                p50: ns(&pcts[0]),
                p90: ns(&pcts[1]),
                p99: ns(&pcts[2]),
            }
        })
        .collect())
}

/// One metric as `buckets` values indexed by bucket, zero where the series had no point.
#[allow(clippy::too_many_arguments)]
async fn read_bucketed(
    engine: &MetricsQueryEngine,
    metric: String,
    agg: Agg,
    filter: &Option<MetricResolvedQuery>,
    start_ns: i64,
    end_ns: i64,
    buckets: usize,
) -> Result<Vec<f64>, PhotonError> {
    let res = engine
        .query_series(MetricSeriesRequest {
            metric,
            agg: Some(agg),
            group_by: Vec::new(),
            filter: filter.clone(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            buckets,
        })
        .await?;
    let width = bucket_width(start_ns, end_ns, buckets);
    let mut out = vec![0.0; buckets];
    // The caller scoped this to one service, but a stray extra series (a second `tenant`, say)
    // must not overwrite the first — sum instead, which is what a single series degrades to.
    for s in &res.series {
        for point in &s.points {
            let Some(v) = point.v else { continue };
            let idx = ((point.t - start_ns) / width).clamp(0, buckets as i64 - 1) as usize;
            out[idx] += v;
        }
    }
    Ok(out)
}

/// `photon.service.basis` is a presence signal, not a value: it is published (as `1`) only for a
/// service whose count is not entry spans. Which fallback it landed on rides in the `basis` label,
/// but the label is not in our `group_by`, so a present gauge without a readable label is reported
/// as the conservative `AllSpans` — the case the UI badges.
fn basis_from(gauge: Option<f64>) -> RedBasis {
    match gauge {
        Some(v) if v > 0.0 => RedBasis::AllSpans,
        _ => RedBasis::Entry,
    }
}

/// The identity of one rollup series, in the shape a `RedRow` needs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SeriesKey {
    service: String,
    tenant: Option<String>,
    environment: Option<String>,
    operation: Option<String>,
}

/// Per-series sparklines off the rollup — requests and errors per bucket, keyed exactly like the
/// raw-span pass's [`RedSeriesKey`] so the caller cannot tell the two apart.
///
/// The fleet board's rows already come off the rollup. Its sparklines did not, and that one raw
/// span scan was the whole cost of the page: with the rows served from the rollup in 24 ms,
/// `/api/services/overview` still took 1,849 ms over 24 hours because the sparkline pass was
/// re-reading 24 hours of spans underneath it.
///
/// Two metrics, not eleven: a sparkline is a shape, and the only shapes it draws are traffic and
/// errors.
pub(crate) async fn spark_from_rollup(
    engine: &MetricsQueryEngine,
    query: &str,
    start_ns: i64,
    end_ns: i64,
    buckets: usize,
    group: RedGroup,
) -> Result<HashMap<RedSeriesKey, Vec<SparkPoint>>, PhotonError> {
    let filter = metric_filter(query, engine.promoted_attributes())?;
    let fam = family(group);
    let keys = group_by(group);
    let (requests, errors) = futures::try_join!(
        read_series_bucketed(
            engine,
            format!("{fam}requests"),
            &filter,
            &keys,
            start_ns,
            end_ns,
            buckets
        ),
        read_series_bucketed(
            engine,
            format!("{fam}errors"),
            &filter,
            &keys,
            start_ns,
            end_ns,
            buckets
        ),
    )?;

    let width = bucket_width(start_ns, end_ns, buckets);
    let mut out: HashMap<RedSeriesKey, Vec<SparkPoint>> = HashMap::new();
    // Union of both metrics' keys, so a series that recorded errors but no requests still draws.
    let all_keys: BTreeMap<&SeriesKey, ()> = requests
        .keys()
        .chain(errors.keys())
        .map(|k| (k, ()))
        .collect();
    for (key, _) in all_keys {
        let mut points: Vec<SparkPoint> = (0..buckets)
            .map(|i| SparkPoint {
                ts: start_ns + i as i64 * width,
                count: 0,
                error_count: 0,
            })
            .collect();
        if let Some(v) = requests.get(key) {
            for (i, n) in v.iter().enumerate() {
                points[i].count = *n;
            }
        }
        if let Some(v) = errors.get(key) {
            for (i, n) in v.iter().enumerate() {
                points[i].error_count = *n;
            }
        }
        out.insert(
            RedSeriesKey {
                service: key.service.clone(),
                tenant: key.tenant.clone(),
                environment: key.environment.clone(),
                operation: key.operation.clone(),
            },
            points,
        );
    }
    Ok(out)
}

/// One counter metric, summed per series PER BUCKET — [`read_one`]'s shape with the window kept
/// rather than collapsed. Zero-filled to `buckets` so every series shares one x-axis.
#[allow(clippy::too_many_arguments)]
async fn read_series_bucketed(
    engine: &MetricsQueryEngine,
    metric: String,
    filter: &Option<MetricResolvedQuery>,
    keys: &[String],
    start_ns: i64,
    end_ns: i64,
    buckets: usize,
) -> Result<BTreeMap<SeriesKey, Vec<u64>>, PhotonError> {
    let res = engine
        .query_series(MetricSeriesRequest {
            metric,
            agg: Some(Agg::Sum),
            group_by: keys.to_vec(),
            filter: filter.clone(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            buckets,
        })
        .await?;
    let width = bucket_width(start_ns, end_ns, buckets);
    let mut out: BTreeMap<SeriesKey, Vec<u64>> = BTreeMap::new();
    for s in &res.series {
        let Some(key) = series_key(s) else { continue };
        let slot = out.entry(key).or_insert_with(|| vec![0; buckets]);
        for point in &s.points {
            let Some(v) = point.v else { continue };
            let idx = ((point.t - start_ns) / width).clamp(0, buckets as i64 - 1) as usize;
            // Counters are non-negative; a negative would be a corrupt point, not a real dip.
            slot[idx] += v.max(0.0).round() as u64;
        }
    }
    Ok(out)
}

/// One metric, aggregated to a single value per series over the whole window.
async fn read_one(
    engine: &MetricsQueryEngine,
    metric: String,
    agg: Agg,
    filter: &Option<MetricResolvedQuery>,
    keys: &[String],
    start_ns: i64,
    end_ns: i64,
) -> Result<BTreeMap<SeriesKey, f64>, PhotonError> {
    let res = engine
        .query_series(MetricSeriesRequest {
            metric,
            agg: Some(agg),
            group_by: keys.to_vec(),
            filter: filter.clone(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            // One bucket: this read wants the window total, not a shape.
            buckets: 1,
        })
        .await?;
    Ok(res.series.iter().filter_map(series_entry).collect())
}

fn series_entry(s: &SeriesResult) -> Option<(SeriesKey, f64)> {
    let value = s.points.iter().find_map(|p| p.v)?;
    Some((series_key(s)?, value))
}

/// The series identity carried by a rollup series' labels.
fn series_key(s: &SeriesResult) -> Option<SeriesKey> {
    Some(SeriesKey {
        service: s.labels.get("service.name")?.clone(),
        // An empty label is how a missing attribute comes back; treat it as absent so it does not
        // render as a Site (or an operation) literally named "".
        tenant: s.labels.get("tenant").filter(|v| !v.is_empty()).cloned(),
        environment: s
            .labels
            .get("deployment.environment")
            .filter(|v| !v.is_empty())
            .cloned(),
        operation: s.labels.get("operation").filter(|v| !v.is_empty()).cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600 * 1_000_000_000;
    /// Coverage: one hour wide, ending at 10h.
    const COVER: RollupCoverage = Some((9 * HOUR, 10 * HOUR));

    #[test]
    fn a_window_inside_coverage_is_eligible() {
        assert!(rollup_eligible("", 9 * HOUR, 10 * HOUR, None, COVER));
        assert!(rollup_eligible(
            "service.name:api",
            9 * HOUR,
            10 * HOUR,
            Some(BUCKET_NS),
            COVER
        ));
    }

    #[test]
    fn a_short_unpublished_tail_splits_rather_than_falling_back() {
        // The tail past the watermark used to disqualify the whole window, because reading it as
        // zeros looks like a traffic cliff rather than missing data. It is now read from raw
        // spans and merged on: `rollup_split` returns where the rollup stops and the tail starts.
        assert_eq!(
            rollup_split("", 9 * HOUR, 10 * HOUR + 1, None, COVER),
            Some(10 * HOUR),
            "the prefix comes off the rollup, the tail off the spans"
        );
    }

    #[test]
    fn a_tail_too_long_to_top_up_falls_back_entirely() {
        // Past `MIN_ROLLUP_SHARE` the tail is most of the answer, and scanning it costs what
        // scanning the whole window costs. Splitting there buys nothing and doubles the read.
        assert_eq!(
            rollup_split("", 9 * HOUR, 12 * HOUR, None, COVER),
            None,
            "a third covered is not worth two reads"
        );
    }

    #[test]
    fn a_window_predating_coverage_is_not_eligible() {
        assert!(
            !rollup_eligible("", 9 * HOUR - 1, 10 * HOUR, None, COVER),
            "zeros from before the rollup existed look exactly like an outage"
        );
    }

    #[test]
    fn no_coverage_at_all_is_never_eligible() {
        assert!(!rollup_eligible("", 9 * HOUR, 10 * HOUR, None, None));
    }

    #[test]
    fn a_resolution_finer_than_one_bucket_is_not_eligible() {
        assert!(!rollup_eligible(
            "",
            9 * HOUR,
            10 * HOUR,
            Some(BUCKET_NS - 1),
            COVER
        ));
        assert!(rollup_eligible(
            "",
            9 * HOUR,
            10 * HOUR,
            Some(BUCKET_NS * 5),
            COVER
        ));
    }

    #[test]
    fn only_queries_over_carried_keys_are_eligible() {
        for q in [
            "service.name:api",
            "service.name:api,web",
            "deployment.environment:prod",
            "operation:*",
            "tenant:us-east",
            "service.name:api deployment.environment:prod",
        ] {
            assert!(rollup_eligible(q, 9 * HOUR, 10 * HOUR, None, COVER), "{q}");
        }
    }

    #[test]
    fn a_query_the_rollup_cannot_answer_falls_back() {
        for q in [
            "status_code:2",         // aggregated away
            "http.route:/checkout",  // a span attribute the rollup does not carry
            "duration>1000000",      // comparison over a destroyed column
            "timeout",               // free-text body match
            "-service.name:api",     // negation, deliberately excluded
            "service.name:api oops", // one bad term poisons the whole query
        ] {
            assert!(!rollup_eligible(q, 9 * HOUR, 10 * HOUR, None, COVER), "{q}");
        }
    }

    #[test]
    fn an_unparseable_query_is_not_quietly_treated_as_match_everything() {
        assert!(!rollup_eligible("field:", 9 * HOUR, 10 * HOUR, None, COVER));
    }

    /// A rollup that claims a fixed coverage range, so the source decision can be driven without
    /// standing up a real pass.
    struct FakeStatus(RollupCoverage);

    #[async_trait::async_trait]
    impl RollupStatus for FakeStatus {
        async fn coverage(&self) -> Result<RollupCoverage, PhotonError> {
            Ok(self.0)
        }
    }

    fn span_req(start: i64, end: i64) -> SpanQueryRequest {
        SpanQueryRequest {
            start_ts_nanos: start,
            end_ts_nanos: end,
            query: None,
            sort: photon_query::SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
            required_attributes: Vec::new(),
        }
    }

    async fn source_for(coverage: RollupCoverage, bucket_ns: Option<i64>) -> RedSource {
        let state = crate::test_state_with_rollup(Some(std::sync::Arc::new(FakeStatus(coverage))));
        match bucket_ns {
            None => {
                red_rows(
                    &state,
                    "",
                    span_req(9 * HOUR, 10 * HOUR),
                    None,
                    RedGroup::Service,
                    &HashMap::new(),
                    500,
                )
                .await
                .unwrap()
                .1
            }
            Some(_) => {
                red_buckets(&state, "", span_req(9 * HOUR, 10 * HOUR), 60, 500)
                    .await
                    .unwrap()
                    .1
            }
        }
    }

    #[tokio::test]
    async fn a_wide_window_inside_coverage_uses_the_rollup() {
        assert_eq!(source_for(COVER, None).await, RedSource::Rollup);
    }

    #[tokio::test]
    async fn a_narrow_resolution_falls_back_to_raw_spans() {
        // 3600 buckets over one hour = 1s each, far finer than the rollup holds.
        let state = crate::test_state_with_rollup(Some(std::sync::Arc::new(FakeStatus(COVER))));
        let (_, source) = red_buckets(&state, "", span_req(9 * HOUR, 10 * HOUR), 3600, 500)
            .await
            .unwrap();
        assert_eq!(source, RedSource::RawSpans);
    }

    #[tokio::test]
    async fn a_bucket_resolution_at_or_above_one_minute_uses_the_rollup() {
        // 60 buckets over one hour = 60s each, exactly one rollup bucket.
        assert_eq!(source_for(COVER, Some(BUCKET_NS)).await, RedSource::Rollup);
    }

    #[tokio::test]
    async fn a_server_with_no_rollup_always_reads_raw_spans() {
        let state = crate::test_state_with_rollup(None);
        let (_, source) = red_rows(
            &state,
            "",
            span_req(9 * HOUR, 10 * HOUR),
            None,
            RedGroup::Service,
            &HashMap::new(),
            500,
        )
        .await
        .unwrap();
        assert_eq!(source, RedSource::RawSpans);
    }

    #[test]
    fn the_wire_spelling_is_stable() {
        assert_eq!(RedSource::Rollup.as_str(), "rollup");
        assert_eq!(RedSource::RawSpans.as_str(), "raw");
    }

    #[test]
    fn a_missing_basis_gauge_means_entry_spans() {
        assert_eq!(basis_from(None), RedBasis::Entry);
        assert_eq!(basis_from(Some(0.0)), RedBasis::Entry);
        assert_eq!(basis_from(Some(1.0)), RedBasis::AllSpans);
    }
}

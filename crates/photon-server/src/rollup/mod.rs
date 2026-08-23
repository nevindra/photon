//! The `photon.service.*` rollup: a background pass that reads spans and publishes per-service
//! (and per-operation) RED as ordinary metrics, so the Backend pages can read a wide window
//! without re-scanning raw spans. See
//! [`docs/adr/0002`](../../../../docs/adr/0002-red-rollups-are-published-as-metrics.md) for why
//! metrics rather than a table of its own, and `docs/subsystems/rollup.md` for the operator view.
//!
//! # Each bucket is written exactly once
//!
//! The plan for this task assumed the pass could re-roll its whole lag window every minute,
//! "because the counters are delta sums keyed by bucket start, so re-emitting replaces rather than
//! double-counts **if the reader de-duplicates by `(series, timestamp)`**".
//!
//! Photon's reader does not de-duplicate. `metric_query`'s bucket fold is `sum(value)` over every
//! point that lands in a bucket, with no `(series, timestamp)` key — so a bucket re-emitted on ten
//! successive passes would report ten times its real traffic. The conditional in the plan was
//! never satisfied.
//!
//! So the pass keeps a **watermark** instead: bucket `[T, T+60)` is published once, on the first
//! pass where `now >= T + 60 + lag_secs`, and never revisited. `lag_secs` therefore buys exactly
//! what it costs — spans that arrive within it are included, spans later than it never reach the
//! rollup. That is the rollup's one inaccuracy, and it is the same one the plan documented; only
//! the mechanism enforcing it changed.
//!
//! The watermark is persisted (SQLite, same control-plane DB as uptime/alerts/tenants) so a
//! restart does not republish what it already wrote. Delivery is **at-least-once**: a crash
//! between the WAL append and the watermark update republishes one bucket. That is a deliberate
//! choice over the alternative — updating the watermark first would drop a bucket that was
//! actually written, and a silent hole is harder to notice than a single doubled minute.

mod otlp;
mod points;
mod watermark;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use photon_api::settings::SettingsStore;
use photon_core::config::RollupConfig;
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_record::MetricBatchBuilder;
use photon_core::metric_schema::MetricSchema;
use photon_core::PhotonError;
use photon_query::{MetricsQueryEngine, SpanQueryEngine, SpanQueryRequest, SpanSort};
use photon_wal::{BroadcastingWal, DiskWal, Wal};

use watermark::{LadderState, Watermark};

const NS_PER_SEC: i64 = 1_000_000_000;

/// How many times a pass will wait and re-ask before giving up on deciding whether rollup points
/// already exist, and how long each wait is — together, a 15-minute grace. Giving up is not fatal:
/// the next tick asks again. See `PassOutcome::Deferred`.
const MAX_BACKFILL_DEFERRALS: usize = 30;
const BACKFILL_DEFERRAL_WAIT: Duration = Duration::from_secs(30);

/// Resolution of the one-off probe that finds where existing rollup points start. 1,000 buckets
/// over a 30-day horizon is ~43 minutes each — coarse, but it only sets a coverage floor, and a
/// floor an hour late costs one hour of rollup reads, not correctness.
const OLDEST_PROBE_BUCKETS: usize = 1000;

/// What one pass did, as opposed to how many rows it wrote — a pass that publishes nothing because
/// the minute was quiet and a pass that publishes nothing because there is nothing settled yet look
/// identical from a row count, and the catch-up loop has to tell them apart.
enum PassOutcome {
    /// Buckets were settled and published (possibly zero rows, if they were quiet). The watermark
    /// advanced.
    Published(usize),
    /// Nothing has settled since the last pass. The normal outcome of most ticks.
    NothingSettled,
    /// A first run that cannot yet tell whether rollup points already exist, so it declined to
    /// decide. Reading "no points" when there are some would backfill straight over them, and a
    /// bucket published twice reports twice its traffic. Right after startup the metrics read path
    /// answers a query for a metric holding 7.9M requests with zero series, and settles a few
    /// minutes later — so "I do not know" is a real state and it is not safe to round it to "no".
    Deferred,
}

/// Which of a bucket's points a publish writes.
///
/// `LatencyOnly` exists for the ladder repair and for nothing else. See
/// [`RollupTask::repair_latency`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Publish {
    All,
    LatencyOnly,
}

impl Publish {
    /// Does this publish have to skip tenants that already ship their own rollup?
    ///
    /// Only `All` does. The exclusion exists to protect COUNTERS — the metrics reader sums a
    /// bucket, so rolling a Site's mirrored spans up beside the Site's own pushed points would
    /// report every federated service serving twice its traffic. A `LatencyOnly` publish emits no
    /// counter, and the one point it does emit is a histogram, whose quantiles are scale-invariant:
    /// a bucket holding both central's copy and the Site's reports the same p50, p90 and p99 as one
    /// holding either alone.
    ///
    /// This is what makes the ladder repair work on a federation central at all. Central holds a
    /// mirrored tenant's spans but deliberately does not roll them up, so with the exclusion
    /// applied the repair would find nothing to publish and every Backend page for that tenant
    /// would show no percentiles until the Site upgraded, repaired and re-pushed. Central heals its
    /// own history instead.
    fn skips_covered_tenants(self) -> bool {
        matches!(self, Publish::All)
    }
}

/// Everything one pass needs. Cloned handles only — the task owns no storage of its own beyond the
/// watermark.
pub struct RollupTask {
    span_query: SpanQueryEngine,
    /// Read side, used for one thing only: finding out what a previous life already published,
    /// so a catch-up over existing data cannot double-count. See `backfill_floor`.
    metrics_query: MetricsQueryEngine,
    metrics_wal: Arc<BroadcastingWal<DiskWal>>,
    metric_schema: MetricSchema,
    counters: Arc<IngestCounters>,
    settings: Arc<dyn SettingsStore>,
    apdex_default_ms: u32,
    cfg: RollupConfig,
    watermark: Watermark,
    /// Offers each pass's points to federation, when this Site pushes its rollup to a central.
    /// `try_send` only (see [`photon_ingest::FederationTee`]): a full queue or an unreachable
    /// central must never delay or fail the local write, which has already happened by then.
    tee: Option<photon_ingest::FederationTee>,
}

impl RollupTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        span_query: SpanQueryEngine,
        metrics_query: MetricsQueryEngine,
        metrics_wal: Arc<BroadcastingWal<DiskWal>>,
        metric_schema: MetricSchema,
        counters: Arc<IngestCounters>,
        settings: Arc<dyn SettingsStore>,
        apdex_default_ms: u32,
        cfg: RollupConfig,
        db_path: &str,
        tee: Option<photon_ingest::FederationTee>,
    ) -> Result<RollupTask, PhotonError> {
        Ok(RollupTask {
            tee,
            span_query,
            metrics_query,
            metrics_wal,
            metric_schema,
            counters,
            settings,
            apdex_default_ms,
            cfg,
            watermark: Watermark::open(db_path)?,
        })
    }

    /// Run forever. A failing pass is logged and retried on the next tick — the watermark is only
    /// advanced by a pass that actually wrote, so a transient query failure delays buckets rather
    /// than losing them.
    pub async fn run(self) {
        let interval = self.cfg.interval();
        loop {
            // One tick before the first pass, deliberately. A first run decides whether to
            // backfill by asking the metrics what is already published, and at the instant the
            // process starts that question has no reliable answer — a probe seconds into startup
            // came back with zero series for a metric holding 7.9M requests, and settled minutes
            // later. It also keeps a minutes-long catch-up off the startup path, so the API is
            // serving before it begins.
            tokio::time::sleep(interval).await;
            // Before the forward pass, not after: a page loading during the catch-up should find
            // the histogram already republished rather than a covered range with no percentiles.
            self.repair_latency().await;
            self.catch_up(now_nanos()).await;
        }
    }

    /// Publish every settled bucket the watermark has not reached yet, slice by slice. This is
    /// the whole loop body: an ordinary tick has less than one slice to do and returns after one
    /// pass, and the same code drains a backlog when there is one. Two situations produce a
    /// backlog, and they are the same problem:
    ///
    /// * **A first run over existing data.** No watermark, spans already on disk. Without a
    ///   catch-up the rollup can only ever describe time from this moment on, so every Backend
    ///   page over a wider window falls back to scanning raw spans — see
    ///   `RollupConfig::backfill_days` for what that costs.
    /// * **A restart after downtime.** The watermark is where the process died, and one slice a
    ///   tick would take an hour of ticks to cover an hour of outage.
    ///
    /// Failures are logged and end the pass rather than retrying forever: the next tick resumes
    /// from the watermark either way, so the worst case is a slower catch-up, not a hole. Each
    /// slice writes the watermark, so this is resumable across restarts.
    async fn catch_up(&self, now_ns: i64) {
        let started = std::time::Instant::now();
        let mut slices = 0usize;
        let mut published = 0usize;
        let mut deferrals = 0usize;
        loop {
            match self.pass(now_ns).await {
                Ok(PassOutcome::Published(n)) => {
                    slices += 1;
                    published += n;
                    // Yield between slices: a catch-up is background work and must not starve
                    // ingest or a page load of the runtime for minutes on end.
                    tokio::task::yield_now().await;
                }
                Ok(PassOutcome::NothingSettled) => break,
                Ok(PassOutcome::Deferred) => {
                    deferrals += 1;
                    if deferrals > MAX_BACKFILL_DEFERRALS {
                        eprintln!(
                            "photon-server: rollup still cannot tell whether it has published \
                             before; leaving the backfill to a later tick"
                        );
                        return;
                    }
                    tokio::time::sleep(BACKFILL_DEFERRAL_WAIT).await;
                }
                Err(e) => {
                    eprintln!(
                        "photon-server: rollup catch-up stopped after {slices} slices ({e}) — \
                         the periodic pass resumes from the watermark"
                    );
                    return;
                }
            }
        }
        // An ordinary tick does exactly one slice; only a real backlog is worth a line.
        if slices > 1 {
            eprintln!(
                "photon-server: rollup caught up {slices} slices ({published} bucket-series rows) \
                 in {:.1}s",
                started.elapsed().as_secs_f64()
            );
        }
    }

    /// Which federated Sites are already publishing their own `photon.service.*` into this window,
    /// read from the metrics themselves.
    ///
    /// A Site ships its rollup only if ITS operator turned `push_service_rollup` on, and Central
    /// cannot see a Site's config — so "is this tenant covered" is answered by looking for its
    /// points, not by asking. A tenant that appears here is skipped by the pass, because rolling
    /// its mirrored spans up again would double every one of its services' counters (the metrics
    /// reader sums a bucket, it does not de-duplicate).
    ///
    /// On failure this returns EMPTY, which means "roll everything up". That is the wrong
    /// direction to be conservative in — an empty answer risks double-counting, a full one risks
    /// nothing — but the alternative is worse: treating a failed read as "everything is covered"
    /// would silently publish no rollup at all while the watermark advanced over the window,
    /// leaving the Backend pages reading a covered-but-empty rollup. The read is a single metadata
    /// query against the local metrics; if it is failing, the pass's own span query is failing too.
    async fn tenants_rolling_themselves_up(&self, start: i64, end: i64) -> HashSet<String> {
        match self
            .metrics_query
            .labels(
                points::REQUESTS_METRIC,
                Some(points::ATTR_TENANT),
                start,
                end,
            )
            .await
        {
            Ok(photon_query::LabelsResult::Values { values, capped }) => {
                if capped {
                    eprintln!(
                        "photon-server: rollup saw more federated tenants than it can list; \
                         some may be rolled up twice"
                    );
                }
                values.into_iter().collect()
            }
            Ok(_) => HashSet::new(),
            Err(e) => {
                eprintln!("photon-server: rollup could not list federated tenants ({e})");
                HashSet::new()
            }
        }
    }

    /// Where a first run starts publishing, and where its coverage actually begins.
    ///
    /// The two are the same on a genuinely fresh node — `backfill_days` back — and differ when a
    /// previous life already wrote rollup points into the horizon. Then publishing must resume
    /// AFTER those points, because the watermark normally guarantees a bucket is written exactly
    /// once and that guarantee is load-bearing: the metrics reader SUMS the points in a bucket
    /// rather than de-duplicating them, so a bucket published twice reports twice its traffic (see
    /// the module header). A backfill runs precisely when the watermark is absent, which is also
    /// the one case where the guarantee cannot come from the watermark — a control-plane DB
    /// rebuilt over a hot tier that still holds published points is a documented workflow in this
    /// repo (`photon.prod-replica.toml`), not a corner case.
    ///
    /// Coverage, though, has to describe what EXISTS, not where we resumed. Reporting only the
    /// resumed range would tell `services_rollup::rollup_eligible` that the days of rollup points
    /// sitting on disk are not covered, and every Backend page over them would fall back to
    /// scanning raw spans — the rollup would be there and unused.
    ///
    /// Returns `(resume_from, coverage_floor)`. The floor is found at bucket resolution and
    /// rounded up, so it under-claims by at most one probe bucket rather than claiming coverage
    /// that is not there.
    async fn backfill_floor(
        &self,
        end: i64,
        bucket_ns: i64,
    ) -> Result<Option<(i64, i64)>, PhotonError> {
        let horizon = floor_to(
            end - self.cfg.backfill_days as i64 * 86_400 * NS_PER_SEC,
            bucket_ns,
        );
        // The oldest existing point is the primary signal, not `metadata`'s newest: a cold read
        // path answers both with "nothing", and only this one lets the caller wait and re-ask
        // (`PassOutcome::Deferred`) rather than round "I do not know" down to "nothing published".
        let Some(oldest) = self.oldest_published(horizon, end, bucket_ns).await else {
            // Nothing readable. Either this really is a first run, or the metrics side is not
            // warm yet — `metadata` decides, and disagreement means wait.
            let seen = self
                .metrics_query
                .metadata(points::REQUESTS_METRIC, horizon, end)
                .await
                .unwrap_or(None);
            if seen.is_some() {
                return Ok(None);
            }
            return Ok(Some((horizon, horizon)));
        };

        let last = self
            .metrics_query
            .metadata(points::REQUESTS_METRIC, horizon, end)
            .await
            .unwrap_or(None)
            .map(|m| m.last_seen_nanos)
            .unwrap_or(oldest);
        // `last_seen` is inside the newest published bucket; resume at the next one.
        let resume = floor_to(last.max(oldest), bucket_ns) + bucket_ns;
        eprintln!(
            "photon-server: rollup backfill skipped — this window already holds {} points; \
             resuming after them, coverage from {oldest}",
            points::REQUESTS_METRIC
        );
        Ok(Some((resume, oldest)))
    }

    /// The start of the oldest bucket in `[horizon, end)` that already holds rollup points.
    ///
    /// `metadata` reports only the newest timestamp, so this walks the series instead: one
    /// bucketed read at [`OLDEST_PROBE_BUCKETS`] resolution, take the first bucket with a value.
    /// The answer is a bucket start, so it is at most one probe bucket later than the true oldest
    /// point — the safe direction, since claiming coverage that is not there would make a Backend
    /// page read an empty rollup and render an empty fleet.
    async fn oldest_published(&self, horizon: i64, end: i64, bucket_ns: i64) -> Option<i64> {
        let res = self
            .metrics_query
            .query_series(photon_query::MetricSeriesRequest {
                metric: points::REQUESTS_METRIC.to_string(),
                agg: Some(photon_core::metric_agg::Agg::Sum),
                group_by: Vec::new(),
                filter: None,
                start_ts_nanos: horizon,
                end_ts_nanos: end,
                buckets: OLDEST_PROBE_BUCKETS,
            })
            .await
            .map_err(|e| eprintln!("photon-server: rollup oldest-point probe failed ({e})"))
            .ok()?;
        let oldest = res
            .series
            .iter()
            .flat_map(|s| s.points.iter())
            .filter(|p| p.v.is_some_and(|v| v > 0.0))
            .map(|p| p.t)
            .min()?;
        Some(floor_to(oldest, bucket_ns))
    }

    /// One pass. Returns how many bucket-series rows were published (0 when nothing has settled
    /// since the last pass, which is the normal case for most ticks).
    ///
    /// `now_ns` is a parameter rather than read inside so tests can drive the clock.
    // The test seam: `run()` drives `pass` through `catch_up`, so nothing in the binary calls this.
    #[allow(dead_code)]
    pub async fn run_once(&self, now_ns: i64) -> Result<usize, PhotonError> {
        Ok(match self.pass(now_ns).await? {
            PassOutcome::Published(n) => n,
            PassOutcome::NothingSettled | PassOutcome::Deferred => 0,
        })
    }

    /// [`run_once`](Self::run_once) with the outcome kept, for the catch-up loop.
    async fn pass(&self, now_ns: i64) -> Result<PassOutcome, PhotonError> {
        let bucket_ns = RollupConfig::BUCKET_SECS as i64 * NS_PER_SEC;
        let lag_ns = self.cfg.lag_secs() as i64 * NS_PER_SEC;

        // The newest bucket whose window has fully closed AND whose lag has elapsed. Exclusive
        // upper bound: `end` is the START of the first bucket that is not yet settled.
        let end = floor_to(now_ns - lag_ns, bucket_ns);

        // Where to resume. With a watermark, the bucket after it. Without one this is a first run
        // — reach back `backfill_days` so the rollup can describe the spans already on disk, not
        // just the ones that arrive from now on (`RollupConfig::backfill_days`). `0` keeps the old
        // behaviour of starting at the lag window's edge.
        let (start, coverage_floor) = match self.watermark.read()? {
            Some(last) => (last + bucket_ns, last + bucket_ns),
            None if self.cfg.backfill_days > 0 => {
                match self.backfill_floor(end, bucket_ns).await? {
                    Some(pair) => pair,
                    None => return Ok(PassOutcome::Deferred),
                }
            }
            None => (end - bucket_ns, end - bucket_ns),
        };
        if start >= end {
            return Ok(PassOutcome::NothingSettled);
        }
        // One pass covers at most `CATCH_UP_SLICE_SECS`. A pass reads raw spans, so an uncapped
        // resume — after a long outage, or on the first run of a backfill — would put one
        // multi-day span scan on this task and hand the WAL a single enormous metrics batch.
        // `catch_up` loops over the slices; a routine tick has less than one slice to do anyway.
        let end = end.min(start + RollupConfig::CATCH_UP_SLICE_SECS * NS_PER_SEC);

        let rows = self
            .publish_range(start, end, bucket_ns, Publish::All)
            .await?;

        // Advance past the last bucket in the window, whether or not it had traffic: a quiet
        // minute is settled too, and leaving it unmarked would make every later pass re-query it.
        // The floor is `coverage_floor`, not `start`: on a resumed backfill they differ, and it is
        // the floor that tells the read path how far back the rollup can answer.
        self.watermark
            .write(coverage_floor.min(start), end - bucket_ns)?;
        Ok(PassOutcome::Published(rows))
    }

    /// Roll `[start, end)` up and write the resulting points. Returns how many bucket-series rows
    /// were produced. The caller owns the watermark — this only writes points, so it can be used
    /// both by a forward pass and by a repair over ground already covered.
    async fn publish_range(
        &self,
        start: i64,
        end: i64,
        bucket_ns: i64,
        what: Publish,
    ) -> Result<usize, PhotonError> {
        let thresholds = self
            .settings
            .all_apdex_thresholds()
            .await
            .unwrap_or_default();
        let covered = if what.skips_covered_tenants() {
            self.tenants_rolling_themselves_up(start, end).await
        } else {
            HashSet::new()
        };
        let pass = self
            .span_query
            .red_rollup(
                span_request(start, end),
                start,
                end,
                bucket_ns,
                &thresholds,
                self.apdex_default_ms,
                self.cfg.top_operations,
                &covered,
            )
            .await?;

        if pass.discovery_truncated {
            eprintln!(
                "photon-server: rollup operation discovery hit its cap — `__other__` is still \
                 exact, but the per-service top-{} was chosen from a truncated candidate set",
                self.cfg.top_operations
            );
        }
        if pass.folded_operations > 0 {
            eprintln!(
                "photon-server: rollup folded {} operations into `{}` (top_operations = {})",
                pass.folded_operations,
                photon_query::OTHER_OPERATION,
                self.cfg.top_operations
            );
        }

        let rows = pass.services.len() + pass.operations.len();
        let mut points = Vec::new();
        for row in pass.services.iter().chain(pass.operations.iter()) {
            match what {
                Publish::All => points.extend(points::points_for(row)),
                Publish::LatencyOnly => points.extend(points::latency_point(row)),
            }
        }

        if !points.is_empty() {
            let mut b = MetricBatchBuilder::with_capacity(&self.metric_schema, points.len());
            for p in &points {
                b.append(p);
            }
            let batch = b.finish()?;
            let n_rows = batch.num_rows() as u64;
            let bytes = batch.get_array_memory_size() as u64;
            self.metrics_wal.append(batch).await?;
            // Rollup points are metrics like any other, so they count under the metrics signal —
            // which means `/api/usage/series` shows what the rollup costs instead of hiding it.
            self.counters.metrics.add(n_rows, bytes);

            // Only AFTER the local write is durable. Central is a mirror of what this Site holds;
            // shipping a bucket we then failed to store would make it the other way round.
            // The repair tees too: a Site that pushes its rollup owns central's copy of its
            // histogram, so skipping it would leave central on the retired metric name forever.
            if let Some(tee) = &self.tee {
                tee.offer(
                    photon_ingest::TeeSignal::Metrics,
                    otlp::encode(&points).into(),
                );
            }
        }
        Ok(rows)
    }

    /// Republish the latency histogram over ground the rollup already covers, when the metric it
    /// is published under has changed since those buckets were written.
    ///
    /// `LATENCY_BOUNDS_NS` is a wire contract, so renumbering it requires renaming
    /// [`photon_query::LATENCY_METRIC`] in the same change — otherwise one series would hold
    /// points measured against two different ladders and the quantile read would interpolate
    /// across both. The rename does its job, and leaves a second problem behind: every bucket the
    /// rollup already published still carries its histogram under the retired name, so a Backend
    /// page over that range would show no percentiles at all. This closes that gap.
    ///
    /// Only the histogram is republished. The counters under those buckets are still correct and
    /// re-emitting them would double every one of them, because the metrics reader sums a bucket
    /// rather than de-duplicating it (see the module header). The histogram is exempt from that
    /// rule for a specific reason: a quantile is scale-invariant, so a bucket whose histogram
    /// landed twice reports exactly the same p50, p90 and p99 as one that landed once.
    ///
    /// Slices, resumes and yields like [`catch_up`](Self::catch_up), and for the same reason —
    /// this reads raw spans over what may be weeks.
    // `run()` drives this on every tick; it is `pub` only so `tests/rollup.rs` can drive it
    // without the tick, the same seam `run_once` is.
    pub async fn repair_latency(&self) {
        let bucket_ns = RollupConfig::BUCKET_SECS as i64 * NS_PER_SEC;
        let mut state = match self.ladder_state(bucket_ns) {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => {
                eprintln!("photon-server: rollup could not read its latency-ladder state ({e})");
                return;
            }
        };
        if !state.pending() {
            return;
        }

        let started = std::time::Instant::now();
        eprintln!(
            "photon-server: rollup latency metric is now `{}` — republishing the histogram for \
             already-covered buckets from {} (counters are untouched)",
            state.metric, state.repair_from
        );
        let mut slices = 0usize;
        while state.pending() {
            let to = state
                .repair_through
                .min(state.repair_from + RollupConfig::CATCH_UP_SLICE_SECS * NS_PER_SEC);
            if let Err(e) = self
                .publish_range(state.repair_from, to, bucket_ns, Publish::LatencyOnly)
                .await
            {
                eprintln!(
                    "photon-server: rollup latency repair stopped after {slices} slices ({e}) — \
                     the next tick resumes from {}",
                    state.repair_from
                );
                return;
            }
            if let Err(e) = self.watermark.advance_repair(to) {
                eprintln!("photon-server: rollup latency repair could not record progress ({e})");
                return;
            }
            state.repair_from = to;
            slices += 1;
            tokio::task::yield_now().await;
        }
        eprintln!(
            "photon-server: rollup latency repair done — {slices} slices in {:.1}s",
            started.elapsed().as_secs_f64()
        );
    }

    /// The repair range, given what the DB remembers and what is published now. `None` when there
    /// is nothing to decide yet.
    ///
    /// Three states, and the third is the one that matters:
    ///
    /// * **No coverage at all.** Nothing has ever been published, so there is nothing under a
    ///   retired name. Record the current metric and claim an empty repair range.
    /// * **Coverage, and the recorded name matches.** Steady state.
    /// * **Coverage, and the name does not match** — either because it changed, or because the
    ///   row predates this bookkeeping entirely (an install upgrading from before the ladder had a
    ///   name recorded). Repair the whole covered range.
    fn ladder_state(&self, bucket_ns: i64) -> Result<Option<LadderState>, PhotonError> {
        let recorded = self.watermark.ladder()?;
        if recorded
            .as_ref()
            .is_some_and(|s| s.metric == photon_query::LATENCY_METRIC)
        {
            return Ok(recorded);
        }
        // Everything the watermark says is covered was written under the retired name.
        let (from, through) = match self.watermark.range()? {
            // `through` is the START of the newest published bucket; the repair's upper bound is
            // exclusive, so it has to reach past it or that bucket keeps its old histogram.
            Some((first, last)) => (first, last + bucket_ns),
            None => (0, 0),
        };
        let state = LadderState {
            metric: photon_query::LATENCY_METRIC.to_string(),
            repair_from: from,
            repair_through: through,
        };
        self.watermark.set_ladder(&state)?;
        Ok(Some(state))
    }
}

/// How the rollup should reach a federation central, decided from config alone so the choice is
/// testable without standing up a tee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeePlan {
    /// No federation, or `push_service_rollup = false`. The rollup stays local.
    None,
    /// Full mode already built a tee for the OTLP mirror; the rollup shares it. It must NOT build
    /// a second one — two tees means two forwarders means every bucket delivered twice, and
    /// central sums the points in a bucket rather than de-duplicating them.
    Share,
    /// Summary mode has no tee at all. The rollup needs its own metrics-only one, plus a forwarder
    /// to drain it — this is the whole point of the task: a summary-mode Site otherwise tells
    /// central nothing per-service.
    Create,
}

/// `full_mode_tee_exists` is whether the full-mode OTLP tee was already built.
pub fn tee_plan(
    fed: Option<&photon_core::config::FederationConfig>,
    full_mode_tee_exists: bool,
) -> TeePlan {
    match fed {
        Some(f) if f.push_service_rollup => {
            if full_mode_tee_exists {
                TeePlan::Share
            } else {
                TeePlan::Create
            }
        }
        _ => TeePlan::None,
    }
}

/// The [`photon_api::services_rollup::RollupStatus`] seam, over the same watermark row the pass
/// writes. Its own SQLite handle rather than a shared one: the API reads this per request, the
/// pass writes it once a minute, and SQLite's WAL mode already makes that safe — sharing a mutex
/// across the two would put a background task's write in the path of a page load.
pub struct WatermarkStatus {
    watermark: Watermark,
    /// The published range is bucket STARTS; a reader asking "is my window covered" needs the end
    /// of the newest bucket, not its start.
    bucket_ns: i64,
}

impl WatermarkStatus {
    pub fn open(db_path: &str) -> Result<WatermarkStatus, PhotonError> {
        Ok(WatermarkStatus {
            watermark: Watermark::open(db_path)?,
            bucket_ns: RollupConfig::BUCKET_SECS as i64 * NS_PER_SEC,
        })
    }
}

#[async_trait::async_trait]
impl photon_api::services_rollup::RollupStatus for WatermarkStatus {
    async fn coverage(&self) -> Result<photon_api::services_rollup::RollupCoverage, PhotonError> {
        Ok(self
            .watermark
            .range()?
            .map(|(first, last)| (first, last + self.bucket_ns)))
    }
}

/// A request covering the whole pass window and nothing else. The rollup does not filter by
/// service or query — `red_rollup` groups over everything that survives pruning, and applies its
/// own local-spans-only predicate.
fn span_request(start: i64, end: i64) -> SpanQueryRequest {
    SpanQueryRequest {
        start_ts_nanos: start,
        // `red_rollup`'s window is half-open but `span_base_predicate` is inclusive, so pull the
        // upper bound back by one nanosecond. Without this a span landing exactly on `end` would
        // be scanned here AND again in the next pass, double-counting it.
        end_ts_nanos: end - 1,
        query: None,
        sort: SpanSort::Recent,
        limit: 0,
        offset: 0,
        projected_attributes: Vec::new(),
        required_attributes: Vec::new(),
    }
}

/// Round `ts` DOWN to a multiple of `step`, for negative `ts` too (Rust's `/` truncates toward
/// zero, which would round a pre-1970 timestamp the wrong way).
fn floor_to(ts: i64, step: i64) -> i64 {
    let step = step.max(1);
    ts.div_euclid(step) * step
}

fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60 * NS_PER_SEC;

    use photon_core::config::{FederationConfig, FederationMode};

    fn fed(mode: FederationMode, push: bool) -> FederationConfig {
        FederationConfig {
            endpoint: "http://central:8080".into(),
            token: "t".into(),
            mode,
            signals: None,
            interval_secs: 30,
            queue_batches: 16,
            push_service_rollup: push,
        }
    }

    #[test]
    fn summary_mode_pushes_service_rollup_when_enabled() {
        // Summary mode builds no OTLP tee, so the rollup needs one of its own — without it a
        // summary-mode Site tells central nothing at all about its services.
        assert_eq!(
            tee_plan(Some(&fed(FederationMode::Summary, true)), false),
            TeePlan::Create
        );
    }

    #[test]
    fn disabled_flag_pushes_only_the_federation_metrics() {
        assert_eq!(
            tee_plan(Some(&fed(FederationMode::Summary, false)), false),
            TeePlan::None
        );
        assert_eq!(
            tee_plan(Some(&fed(FederationMode::Full, false)), true),
            TeePlan::None
        );
    }

    #[test]
    fn full_mode_does_not_double_push() {
        assert_eq!(
            tee_plan(Some(&fed(FederationMode::Full, true)), true),
            TeePlan::Share,
            "a second tee means a second forwarder means every bucket counted twice"
        );
    }

    #[test]
    fn no_federation_at_all_keeps_the_rollup_local() {
        assert_eq!(tee_plan(None, false), TeePlan::None);
    }

    #[test]
    fn only_a_counter_publish_skips_a_tenant_that_rolls_itself_up() {
        assert!(Publish::All.skips_covered_tenants());
        assert!(
            !Publish::LatencyOnly.skips_covered_tenants(),
            "a federation central holds a mirrored tenant's spans and never rolls them up, so a \
             repair that honoured the exclusion would publish nothing and leave that tenant's \
             Backend pages with no percentiles at all"
        );
    }

    #[test]
    fn floor_to_rounds_down_on_both_sides_of_the_epoch() {
        assert_eq!(floor_to(0, MINUTE), 0);
        assert_eq!(floor_to(MINUTE + 1, MINUTE), MINUTE);
        assert_eq!(floor_to(MINUTE - 1, MINUTE), 0);
        assert_eq!(
            floor_to(-1, MINUTE),
            -MINUTE,
            "truncation would give 0 here"
        );
    }

    #[test]
    fn the_span_window_is_half_open_so_successive_passes_cannot_overlap() {
        let a = span_request(0, MINUTE);
        let b = span_request(MINUTE, 2 * MINUTE);
        assert_eq!(a.end_ts_nanos, MINUTE - 1);
        assert!(
            a.end_ts_nanos < b.start_ts_nanos,
            "a span at exactly {MINUTE} must belong to exactly one pass"
        );
    }
}

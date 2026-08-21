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

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use photon_api::settings::SettingsStore;
use photon_core::config::RollupConfig;
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_record::MetricBatchBuilder;
use photon_core::metric_schema::MetricSchema;
use photon_core::PhotonError;
use photon_query::{SpanQueryEngine, SpanQueryRequest, SpanSort};
use photon_wal::{BroadcastingWal, DiskWal, Wal};

use watermark::Watermark;

const NS_PER_SEC: i64 = 1_000_000_000;

/// Everything one pass needs. Cloned handles only — the task owns no storage of its own beyond the
/// watermark.
pub struct RollupTask {
    span_query: SpanQueryEngine,
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
            tokio::time::sleep(interval).await;
            match self.run_once(now_nanos()).await {
                Ok(0) => {}
                Ok(n) => eprintln!("photon-server: rollup published {n} buckets"),
                Err(e) => eprintln!("photon-server: rollup pass failed ({e}) — retrying next tick"),
            }
        }
    }

    /// One pass. Returns how many bucket-series rows were published (0 when nothing has settled
    /// since the last pass, which is the normal case for most ticks).
    ///
    /// `now_ns` is a parameter rather than read inside so tests can drive the clock.
    pub async fn run_once(&self, now_ns: i64) -> Result<usize, PhotonError> {
        let bucket_ns = RollupConfig::BUCKET_SECS as i64 * NS_PER_SEC;
        let lag_ns = self.cfg.lag_secs() as i64 * NS_PER_SEC;

        // The newest bucket whose window has fully closed AND whose lag has elapsed. Exclusive
        // upper bound: `end` is the START of the first bucket that is not yet settled.
        let end = floor_to(now_ns - lag_ns, bucket_ns);

        // Where to resume. A first run (or one after the watermark was lost) starts at the oldest
        // bucket still inside the lag window rather than backfilling all of history: backfilling
        // is a legitimate operation, but it is not something a routine tick should decide to do.
        let start = match self.watermark.read()? {
            Some(last) => last + bucket_ns,
            None => end - bucket_ns,
        };
        if start >= end {
            return Ok(0);
        }

        let thresholds = self
            .settings
            .all_apdex_thresholds()
            .await
            .unwrap_or_default();
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
            points.extend(points::points_for(row));
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
            if let Some(tee) = &self.tee {
                tee.offer(
                    photon_ingest::TeeSignal::Metrics,
                    otlp::encode(&points).into(),
                );
            }
        }

        // Advance past the last bucket in the window, whether or not it had traffic: a quiet
        // minute is settled too, and leaving it unmarked would make every later pass re-query it.
        self.watermark.write(start, end - bucket_ns)?;
        Ok(rows)
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

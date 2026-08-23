//! Service SLOs, error budgets and burn rate.
//!
//! An SLO turns a health page into a decision. "Error rate 0.4%" is a reading; "you have 12% of
//! this month's budget left and you are burning it 14× faster than you can afford" is an
//! instruction. That difference is the whole reason this exists.
//!
//! ## What it reads
//!
//! **The rollup, and only the rollup.** An SLO window is days or weeks wide; that is precisely the
//! shape the rollup was built for, and answering it from raw spans would put a multi-minute scan
//! behind a page load. So when the rollup does not cover the whole window, the budget is reported
//! **partial** — it says what it measured and from when — instead of silently falling back to a
//! scan or, worse, reporting a full-window number computed from part of a window.
//!
//! ## The two kinds
//!
//! - **Availability** — good events are requests that did not error. Straight from
//!   `photon.{service,operation}.requests` and `.errors`.
//! - **Latency** — good events are requests at or below a threshold. This comes off the rollup's
//!   explicit-bucket duration histogram, so the threshold is **snapped down to a bucket edge** and
//!   the edge actually measured is reported back (`measured_threshold_ms`). A histogram does not
//!   know anything finer than its own edges, and an SLO that quietly measured 500ms when it was
//!   configured for 300ms is a number nobody can act on.
//!
//! ## Burn rate is multi-window on purpose
//!
//! A single burn rate over one window either alerts on every transient blip (short window) or
//! notices a real outage an hour late (long window). The standard fix, and the one used here, is to
//! require BOTH a long and a short window to be burning: fast = 1h confirmed by 5m, slow = 6h
//! confirmed by 30m. `fast.firing` is true only when both of its windows exceed the burn threshold.
//!
//! Nothing here is a new alert kind. Because the rollup is metrics, a burn-rate alert is an
//! ordinary metrics rule in `photon-alerts` over `photon.service.errors` / `.requests` — see
//! `docs/subsystems/alerts.md`. This module adds no `ConditionSource` and no rule type.
use std::sync::Mutex;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use photon_core::metric_agg::Agg;
use photon_core::PhotonError;
use photon_query::MetricSeriesRequest;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::query_params::grammar_term;
use crate::AppState;

/// What an SLO measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SloKind {
    /// Good = requests that did not error.
    Availability,
    /// Good = requests at or below `threshold_ms`.
    Latency,
}

impl SloKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SloKind::Availability => "availability",
            SloKind::Latency => "latency",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "availability" => Some(SloKind::Availability),
            "latency" => Some(SloKind::Latency),
            _ => None,
        }
    }
}

/// One objective.
///
/// `environment` and `operation` are empty strings, never `None`, for the same reason the
/// deployments table does it: SQLite treats NULL as distinct from NULL, so a nullable column would
/// let two SLOs with the same name coexist under one service.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slo {
    pub id: String,
    pub service: String,
    pub environment: String,
    /// `""` = the whole service. A named operation holds one endpoint to a tighter line than the
    /// service around it, which is the point: a checkout call and an admin page are not the same
    /// promise.
    pub operation: String,
    pub name: String,
    pub kind: SloKind,
    /// The fraction of good events promised, `0 < target < 1` (0.999 = "three nines").
    pub target: f64,
    /// Latency only; `0` for an availability SLO.
    pub threshold_ms: u32,
    pub window_days: u32,
}

/// The fields a create/update accepts. `id` is server-minted, never client-supplied.
#[derive(Debug, Clone, Deserialize)]
pub struct SloInput {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub environment: String,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub target: f64,
    #[serde(default)]
    pub threshold_ms: u32,
    #[serde(default)]
    pub window_days: u32,
}

/// Persistence boundary, mirroring `settings.rs` / `deployments.rs`.
#[async_trait]
pub trait SloStore: Send + Sync {
    async fn list(&self, service: &str) -> Result<Vec<Slo>, PhotonError>;
    async fn get(&self, id: &str) -> Result<Option<Slo>, PhotonError>;
    async fn create(&self, service: &str, input: &SloInput) -> Result<Slo, PhotonError>;
    async fn update(&self, id: &str, input: &SloInput) -> Result<Option<Slo>, PhotonError>;
    async fn delete(&self, id: &str) -> Result<bool, PhotonError>;
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS service_slos (
    id           TEXT PRIMARY KEY,
    service      TEXT NOT NULL,
    environment  TEXT NOT NULL DEFAULT '',
    operation    TEXT NOT NULL DEFAULT '',
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,
    target       REAL NOT NULL,
    threshold_ms INTEGER NOT NULL DEFAULT 0,
    window_days  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS service_slos_ident
    ON service_slos (service, environment, operation, name);
"#;

const COLUMNS: &str = "id,service,environment,operation,name,kind,target,threshold_ms,window_days";

pub struct SqliteSloStore {
    conn: Mutex<Connection>,
}

impl SqliteSloStore {
    pub fn open(path: &str) -> Result<Self, PhotonError> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
        }
        let conn = Connection::open(path).map_err(err)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(err)?;
        Self::from_conn(conn)
    }

    pub fn open_in_memory() -> Result<Self, PhotonError> {
        Self::from_conn(Connection::open_in_memory().map_err(err)?)
    }

    fn from_conn(conn: Connection) -> Result<Self, PhotonError> {
        conn.execute_batch(SCHEMA).map_err(err)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

fn row_to_slo(r: &rusqlite::Row<'_>) -> rusqlite::Result<Slo> {
    let kind: String = r.get(5)?;
    Ok(Slo {
        id: r.get(0)?,
        service: r.get(1)?,
        environment: r.get(2)?,
        operation: r.get(3)?,
        name: r.get(4)?,
        // A row whose `kind` we no longer recognise reads as availability rather than failing the
        // whole list: one unreadable SLO must not take the page down with it.
        kind: SloKind::parse(&kind).unwrap_or(SloKind::Availability),
        target: r.get(6)?,
        threshold_ms: r.get::<_, i64>(7)? as u32,
        window_days: r.get::<_, i64>(8)? as u32,
    })
}

/// Validate an input, returning the normalized values or a message for a 400.
///
/// Every rule here exists because the alternative is a budget that cannot be computed: a target of
/// 1.0 has a zero-sized budget (every burn rate is infinite), a target of 0 promises nothing, and a
/// latency SLO with no threshold has no line to be on the right side of.
fn validate(input: &SloInput) -> Result<(SloKind, String), String> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err("name is required".into());
    }
    let Some(kind) = SloKind::parse(input.kind.trim()) else {
        return Err("kind must be `availability` or `latency`".into());
    };
    if !(input.target > 0.0 && input.target < 1.0) {
        return Err("target must be between 0 and 1 (exclusive) — 0.999 is three nines".into());
    }
    if input.window_days == 0 || input.window_days > 365 {
        return Err("window_days must be between 1 and 365".into());
    }
    if kind == SloKind::Latency && input.threshold_ms == 0 {
        return Err("a latency SLO needs a threshold_ms".into());
    }
    Ok((kind, name))
}

/// A stable id from the SLO's identity, so the same objective keeps its id across a restore.
/// FNV-1a 64, same construction as `deployments::deployment_id`.
fn slo_id(service: &str, environment: &str, operation: &str, name: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in [service, environment, operation, name] {
        for b in part.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        h ^= 0x1f;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{h:016x}")
}

#[async_trait]
impl SloStore for SqliteSloStore {
    async fn list(&self, service: &str) -> Result<Vec<Slo>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare(&format!(
                "SELECT {COLUMNS} FROM service_slos WHERE service=?1 ORDER BY name"
            ))
            .map_err(err)?;
        let rows = stmt.query_map(params![service], row_to_slo).map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
    }

    async fn get(&self, id: &str) -> Result<Option<Slo>, PhotonError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare(&format!("SELECT {COLUMNS} FROM service_slos WHERE id=?1"))
            .map_err(err)?;
        let mut rows = stmt.query_map(params![id], row_to_slo).map_err(err)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(err)?)),
            None => Ok(None),
        }
    }

    async fn create(&self, service: &str, input: &SloInput) -> Result<Slo, PhotonError> {
        let (kind, name) = validate(input).map_err(PhotonError::Query)?;
        let slo = Slo {
            id: slo_id(service, &input.environment, &input.operation, &name),
            service: service.to_string(),
            environment: input.environment.clone(),
            operation: input.operation.clone(),
            name,
            kind,
            target: input.target,
            threshold_ms: input.threshold_ms,
            window_days: input.window_days,
        };
        let c = self.conn.lock().unwrap();
        c.execute(
            &format!("INSERT INTO service_slos ({COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"),
            params![
                slo.id,
                slo.service,
                slo.environment,
                slo.operation,
                slo.name,
                slo.kind.as_str(),
                slo.target,
                slo.threshold_ms as i64,
                slo.window_days as i64,
            ],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                PhotonError::Query("an SLO with that name already exists here".into())
            }
            other => err(other),
        })?;
        Ok(slo)
    }

    /// Everything except identity is editable in place. The identity fields (service, environment,
    /// operation, name) are what the id hashes, so changing them is a delete plus a create — done
    /// by the caller, deliberately, rather than silently here under a stale id.
    async fn update(&self, id: &str, input: &SloInput) -> Result<Option<Slo>, PhotonError> {
        let (kind, _) = validate(input).map_err(PhotonError::Query)?;
        // The guard lives in its own block: `MutexGuard` is not `Send`, so it must be gone before
        // the `self.get(id).await` below, and a bare `drop` is not enough for the async lowering.
        let n = {
            let c = self.conn.lock().unwrap();
            c.execute(
                "UPDATE service_slos SET kind=?2, target=?3, threshold_ms=?4, window_days=?5 WHERE id=?1",
                params![
                    id,
                    kind.as_str(),
                    input.target,
                    input.threshold_ms as i64,
                    input.window_days as i64
                ],
            )
            .map_err(err)?
        };
        if n == 0 {
            return Ok(None);
        }
        self.get(id).await
    }

    async fn delete(&self, id: &str) -> Result<bool, PhotonError> {
        let c = self.conn.lock().unwrap();
        let n = c
            .execute("DELETE FROM service_slos WHERE id=?1", params![id])
            .map_err(err)?;
        Ok(n > 0)
    }
}

// --- Budget & burn rate ---------------------------------------------------------------------

/// One burn-rate window pair. `firing` requires BOTH — see the module header.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BurnWindow {
    /// The long window's label, e.g. `"1h"`.
    pub long: String,
    /// The short confirming window's label, e.g. `"5m"`.
    pub short: String,
    /// Burn rate over the long window: bad-event rate ÷ the rate the budget can afford.
    pub long_rate: f64,
    pub short_rate: f64,
    /// The rate at which this pair fires.
    pub threshold: f64,
    pub firing: bool,
}

/// What `GET /api/services/:service/slos/:id/budget` answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SloBudget {
    pub slo: Slo,
    /// The window actually measured, epoch ms. When `partial`, this is the covered part, not the
    /// SLO's nominal window.
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    /// `true` when the rollup does not hold the whole SLO window. The numbers are then over
    /// `window_start_ms..window_end_ms`, which is less than `window_days`.
    pub partial: bool,
    pub total_events: f64,
    pub bad_events: f64,
    /// Good ÷ total over the window, or `null` when nothing was measured.
    pub sli: Option<f64>,
    /// Events the budget allows over the window: `(1 - target) × total`.
    pub budget_events: f64,
    /// `budget_events - bad_events`. Negative means the objective is already missed.
    pub remaining_events: f64,
    /// Remaining ÷ budget, clamped to `[-1, 1]`… except it is not clamped below: "−240% of budget"
    /// is a real and useful thing to be told.
    pub remaining_fraction: Option<f64>,
    /// Seconds until the budget is gone at the fast window's current burn, or `null` when it is
    /// not burning (or is already spent).
    pub exhausts_in_secs: Option<f64>,
    pub fast: BurnWindow,
    pub slow: BurnWindow,
    /// For a latency SLO, the histogram edge the counts were actually taken at. Equal to
    /// `threshold_ms` only when the threshold happens to be a bucket edge.
    pub measured_threshold_ms: Option<f64>,
}

const MS: i64 = 1_000_000;
const SEC_NS: i64 = 1_000_000_000;

/// The metric family an SLO reads: the operation family when it is scoped to one, the service
/// family otherwise. Named for what it means rather than inlined, because picking the wrong one
/// silently doubles a service's traffic (see `docs/subsystems/rollup.md`).
fn family(slo: &Slo) -> &'static str {
    if slo.operation.is_empty() {
        "photon.service."
    } else {
        "photon.operation."
    }
}

/// The rollup filter for one SLO, as a log-grammar string the metrics resolver compiles.
///
/// An operation-scoped SLO adds `operation:<name>` and reads the operation family, so the other
/// operations of the same service contribute nothing — not even to the denominator.
pub(crate) fn slo_filter_query(slo: &Slo) -> String {
    let mut terms = vec![grammar_term("service.name", &slo.service)];
    if !slo.environment.is_empty() {
        terms.push(grammar_term("deployment.environment", &slo.environment));
    }
    if !slo.operation.is_empty() {
        terms.push(grammar_term("operation", &slo.operation));
    }
    terms.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

/// Burn rate: how many times faster than affordable the budget is being spent.
///
/// `1.0` means the budget lasts exactly the SLO window and no longer. `0` means nothing is burning.
/// `None` when the window observed no events — a service with no traffic is not burning budget, but
/// it is also not proving anything, and the two must not be conflated with "healthy".
fn burn_rate(bad: f64, total: f64, target: f64) -> Option<f64> {
    if total <= 0.0 {
        return None;
    }
    let affordable = 1.0 - target;
    if affordable <= 0.0 {
        return None;
    }
    Some((bad / total) / affordable)
}

/// The good/bad counts for one SLO over one window, read off the rollup.
async fn counts(
    state: &AppState,
    slo: &Slo,
    start_ns: i64,
    end_ns: i64,
) -> Result<(f64, f64, Option<f64>), PhotonError> {
    let fam = family(slo);
    let query = slo_filter_query(slo);
    let filter =
        crate::services_rollup::metric_filter(&query, state.metrics_query.promoted_attributes())?;
    let series_req = |metric: String, agg: Agg| MetricSeriesRequest {
        metric,
        agg: Some(agg),
        // No group_by: one SLO is one number. Splitting by label here would make a service with
        // three environments look like three separate budgets when the SLO covers all of them.
        group_by: vec![],
        filter: filter.clone(),
        start_ts_nanos: start_ns,
        end_ts_nanos: end_ns,
        buckets: 1,
    };
    let sum_of = |res: photon_query::QuerySeriesResult| -> f64 {
        res.series
            .iter()
            .flat_map(|s| s.points.iter())
            .filter_map(|p| p.v)
            .sum()
    };

    match slo.kind {
        SloKind::Availability => {
            let (req, errs) = futures::try_join!(
                state
                    .metrics_query
                    .query_series(series_req(format!("{fam}requests"), Agg::Sum)),
                state
                    .metrics_query
                    .query_series(series_req(format!("{fam}errors"), Agg::Sum)),
            )?;
            let total = sum_of(req).max(0.0);
            // Clamped to the total: a rollup bucket boundary can, in principle, put an error in a
            // window whose request landed in the previous one. Reporting bad > total would produce
            // an SLI above 100% error, which is not a thing.
            let bad = sum_of(errs).max(0.0).min(total);
            Ok((total, bad, None))
        }
        SloKind::Latency => {
            let bound_ns = (slo.threshold_ms as f64) * (MS as f64);
            let res = state
                .metrics_query
                .latency_sli(
                    series_req(format!("{fam}{}", photon_query::LATENCY_METRIC), Agg::Count),
                    bound_ns,
                )
                .await?;
            let mut good = 0.0;
            let mut total = 0.0;
            for s in &res.series {
                for (g, t) in &s.buckets {
                    good += g;
                    total += t;
                }
            }
            let measured = (res.measured_bound / MS as f64).max(0.0);
            Ok((total, (total - good).max(0.0), Some(measured)))
        }
    }
}

/// Compute one SLO's budget and burn rates.
pub(crate) async fn compute_budget(
    state: &AppState,
    slo: Slo,
    now_ns: i64,
) -> Result<SloBudget, PhotonError> {
    let nominal_start = now_ns - (slo.window_days as i64) * 86_400 * SEC_NS;

    // The rollup is the only source. Where it does not reach, the answer is smaller and says so.
    let coverage = match &state.rollup {
        Some(status) => status.coverage().await.unwrap_or(None),
        None => None,
    };
    let (first, published_through) = coverage.unwrap_or((nominal_start, now_ns));
    let start_ns = nominal_start.max(first);
    let end_ns = now_ns.min(published_through);
    let partial = start_ns > nominal_start || end_ns < now_ns;

    if end_ns <= start_ns {
        // Nothing measurable. An empty budget, explicitly partial, beats a 500 — the page then
        // says "no data yet", which is the truth on a fresh install.
        return Ok(empty_budget(slo, start_ns, end_ns, true));
    }

    // The whole window, plus the four burn windows, concurrently: five cheap rollup reads beat one
    // sequential chain behind a page load.
    let windows: [(i64, i64); 4] = [
        (end_ns - 3_600 * SEC_NS, end_ns),
        (end_ns - 300 * SEC_NS, end_ns),
        (end_ns - 6 * 3_600 * SEC_NS, end_ns),
        (end_ns - 1_800 * SEC_NS, end_ns),
    ];
    let (whole, w) = futures::try_join!(
        counts(state, &slo, start_ns, end_ns),
        futures::future::try_join_all(windows.iter().map(|(s, e)| counts(
            state,
            &slo,
            (*s).max(first),
            *e
        ))),
    )?;

    let (total, bad, measured) = whole;
    let budget_events = (1.0 - slo.target) * total;
    let remaining_events = budget_events - bad;
    let rate = |i: usize| burn_rate(w[i].1, w[i].0, slo.target).unwrap_or(0.0);

    let fast = BurnWindow {
        long: "1h".into(),
        short: "5m".into(),
        long_rate: rate(0),
        short_rate: rate(1),
        threshold: FAST_BURN,
        firing: rate(0) >= FAST_BURN && rate(1) >= FAST_BURN,
    };
    let slow = BurnWindow {
        long: "6h".into(),
        short: "30m".into(),
        long_rate: rate(2),
        short_rate: rate(3),
        threshold: SLOW_BURN,
        firing: rate(2) >= SLOW_BURN && rate(3) >= SLOW_BURN,
    };

    // Time to exhaustion at the CURRENT burn, not the window's average: the question a burn rate
    // answers is "how long do I have", and the answer must reflect what is happening now.
    let window_secs = ((end_ns - start_ns) as f64 / SEC_NS as f64).max(1.0);
    let bad_per_sec = if fast.long_rate > 0.0 {
        (w[0].1) / ((windows[0].1 - windows[0].0.max(first)) as f64 / SEC_NS as f64).max(1.0)
    } else {
        0.0
    };
    let exhausts_in_secs = (bad_per_sec > 0.0 && remaining_events > 0.0)
        .then(|| remaining_events / bad_per_sec)
        // A budget already spent exhausts in zero seconds, which is a more useful thing to render
        // than "not burning".
        .or((remaining_events <= 0.0 && total > 0.0).then_some(0.0));
    let _ = window_secs;

    Ok(SloBudget {
        slo,
        window_start_ms: start_ns / MS,
        window_end_ms: end_ns / MS,
        partial,
        total_events: total,
        bad_events: bad,
        sli: (total > 0.0).then(|| (total - bad) / total),
        budget_events,
        remaining_events,
        remaining_fraction: (budget_events > 0.0).then(|| remaining_events / budget_events),
        exhausts_in_secs,
        fast,
        slow,
        measured_threshold_ms: measured,
    })
}

/// Google's SRE-workbook multi-window burn thresholds: 14.4× over an hour spends a 30-day budget in
/// two days, and 6× over six hours spends it in five. They are constants rather than settings
/// because a per-SLO burn threshold is a knob nobody has the information to turn.
const FAST_BURN: f64 = 14.4;
const SLOW_BURN: f64 = 6.0;

fn empty_budget(slo: Slo, start_ns: i64, end_ns: i64, partial: bool) -> SloBudget {
    let zero = |long: &str, short: &str, threshold: f64| BurnWindow {
        long: long.into(),
        short: short.into(),
        long_rate: 0.0,
        short_rate: 0.0,
        threshold,
        firing: false,
    };
    SloBudget {
        slo,
        window_start_ms: start_ns / MS,
        window_end_ms: end_ns / MS,
        partial,
        total_events: 0.0,
        bad_events: 0.0,
        sli: None,
        budget_events: 0.0,
        remaining_events: 0.0,
        remaining_fraction: None,
        exhausts_in_secs: None,
        fast: zero("1h", "5m", FAST_BURN),
        slow: zero("6h", "30m", SLOW_BURN),
        measured_threshold_ms: None,
    }
}

// --- Handlers -------------------------------------------------------------------------------

/// The store, or the 404 body for a node that has none.
///
/// `Result<_, Box<Response>>` rather than `Result<_, Response>`: an axum `Response` is a large
/// value, and every handler here returns this on the happy path too — boxing keeps the error arm
/// from setting the size of every `Result` in the module.
fn store(state: &AppState) -> Result<&std::sync::Arc<dyn SloStore>, Box<Response>> {
    state.slos.as_ref().ok_or_else(|| {
        Box::new(
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "SLOs are not enabled on this node" })),
            )
                .into_response(),
        )
    })
}

fn bad_request(msg: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": msg.to_string() })),
    )
        .into_response()
}

fn server_error(e: PhotonError) -> Response {
    // A validation failure surfaces as `PhotonError::Query`, which is the caller's fault, not
    // ours — mapping it to 500 would make a typo look like an outage.
    match e {
        PhotonError::Query(msg) => bad_request(msg),
        other => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": other.to_string() })),
        )
            .into_response(),
    }
}

pub(crate) async fn list(State(state): State<AppState>, Path(service): Path<String>) -> Response {
    let s = match store(&state) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match s.list(&service).await {
        Ok(slos) => Json(json!({ "slos": slos })).into_response(),
        Err(e) => server_error(e),
    }
}

pub(crate) async fn create(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Json(input): Json<SloInput>,
) -> Response {
    let s = match store(&state) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match s.create(&service, &input).await {
        Ok(slo) => (StatusCode::CREATED, Json(json!({ "slo": slo }))).into_response(),
        Err(e) => server_error(e),
    }
}

pub(crate) async fn update(
    State(state): State<AppState>,
    Path((_service, id)): Path<(String, String)>,
    Json(input): Json<SloInput>,
) -> Response {
    let s = match store(&state) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match s.update(&id, &input).await {
        Ok(Some(slo)) => Json(json!({ "slo": slo })).into_response(),
        Ok(None) => not_found(),
        Err(e) => server_error(e),
    }
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    Path((_service, id)): Path<(String, String)>,
) -> Response {
    let s = match store(&state) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match s.delete(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(e) => server_error(e),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "no such SLO" })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub(crate) struct BudgetParams {
    /// Epoch NANOSECONDS to treat as "now". Present for tests and for looking at a past moment;
    /// the UI does not send it.
    #[serde(default)]
    now: Option<String>,
}

pub(crate) async fn budget(
    State(state): State<AppState>,
    Path((_service, id)): Path<(String, String)>,
    Query(params): Query<BudgetParams>,
) -> Response {
    let s = match store(&state) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let slo = match s.get(&id).await {
        Ok(Some(slo)) => slo,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e),
    };
    let now_ns = match params.now.as_deref() {
        Some(v) => match v.parse::<i64>() {
            Ok(n) => n,
            Err(_) => return bad_request("`now` must be epoch nanoseconds"),
        },
        None => now_nanos(),
    };
    match compute_budget(&state, slo, now_ns).await {
        Ok(b) => Json(b).into_response(),
        Err(e) => server_error(e),
    }
}

fn now_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod route_tests {
    //! The HTTP surface: status codes, the session gate, and that a validation failure reads as a
    //! 400 rather than a 500.
    use super::*;
    use axum::http::StatusCode;
    use serde_json::Value;

    async fn request(
        router: axum::Router,
        req: axum::http::Request<axum::body::Body>,
    ) -> (StatusCode, Value) {
        use tower::ServiceExt;
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn body(v: &Value) -> axum::body::Body {
        axum::body::Body::from(serde_json::to_vec(v).unwrap())
    }

    fn valid() -> Value {
        json!({
            "name": "checkout availability",
            "environment": "prod",
            "kind": "availability",
            "target": 0.999,
            "window_days": 30
        })
    }

    #[tokio::test]
    async fn create_list_patch_delete_round_trip() {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;

        let (status, created) = request(
            router.clone(),
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/services/api/slos")
                .header(axum::http::header::COOKIE, cookie.clone())
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(body(&valid()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["slo"]["id"].as_str().unwrap().to_string();

        let (status, listed) = request(
            router.clone(),
            axum::http::Request::builder()
                .uri("/api/services/api/slos")
                .header(axum::http::header::COOKIE, cookie.clone())
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["slos"].as_array().unwrap().len(), 1);

        let mut edit = valid();
        edit["target"] = json!(0.99);
        let (status, patched) = request(
            router.clone(),
            axum::http::Request::builder()
                .method("PATCH")
                .uri(format!("/api/services/api/slos/{id}"))
                .header(axum::http::header::COOKIE, cookie.clone())
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(body(&edit))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(patched["slo"]["target"], json!(0.99));

        let (status, _) = request(
            router.clone(),
            axum::http::Request::builder()
                .method("DELETE")
                .uri(format!("/api/services/api/slos/{id}"))
                .header(axum::http::header::COOKIE, cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn an_invalid_target_is_a_400_not_a_500() {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let mut bad = valid();
        bad["target"] = json!(1.5);
        let (status, out) = request(
            router,
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/services/api/slos")
                .header(axum::http::header::COOKIE, cookie)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(body(&bad))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(out["error"].as_str().unwrap().contains("target"));
    }

    #[tokio::test]
    async fn a_budget_for_an_unknown_slo_is_a_404() {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let (status, _) = request(
            router,
            axum::http::Request::builder()
                .uri("/api/services/api/slos/nope/budget")
                .header(axum::http::header::COOKIE, cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_slo_routes_need_a_session() {
        let router = crate::test_router();
        let (status, _) = request(
            router,
            axum::http::Request::builder()
                .uri("/api/services/api/slos")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[cfg(test)]
mod budget_tests {
    //! End-to-end budget reads: real rollup metric points through a real metrics engine, so the
    //! metric names, the filter compilation and the histogram ladder are all exercised for real
    //! rather than mocked into agreement.
    use super::*;
    use photon_core::metric_record::MetricPoint;
    use photon_core::metric_schema::metric_type;
    use photon_query::MetricsQueryEngine;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    const SEC: i64 = 1_000_000_000;
    const NOW: i64 = 1_800_000_000 * SEC;
    /// Delta temporality, restated (`photon-query`'s constant is crate-private) exactly as
    /// `photon-server/src/rollup/points.rs` restates it — these fixtures ARE rollup points.
    const TEMPORALITY_DELTA: i32 = 1;

    struct FixedCoverage(Option<(i64, i64)>);
    #[async_trait]
    impl crate::services_rollup::RollupStatus for FixedCoverage {
        async fn coverage(&self) -> Result<Option<(i64, i64)>, PhotonError> {
            Ok(self.0)
        }
    }

    fn attrs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn counter(name: &str, ts: i64, value: f64, extra: &[(&str, &str)]) -> MetricPoint {
        let mut a = attrs(&[("service.name", "api"), ("deployment.environment", "prod")]);
        for (k, v) in extra {
            a.insert(k.to_string(), v.to_string());
        }
        MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::SUM,
            temporality: Some(TEMPORALITY_DELTA),
            timestamp_nanos: ts,
            value: Some(value),
            attributes: a,
            ..Default::default()
        }
    }

    /// One rollup latency histogram point on the real `LATENCY_BOUNDS_NS` ladder.
    /// `fast` samples land in the 1ms bucket, `slow` ones in the 2s bucket.
    fn duration(name: &str, ts: i64, fast: u64, slow: u64, extra: &[(&str, &str)]) -> MetricPoint {
        let bounds = photon_query::LATENCY_BOUNDS_NS;
        let mut counts = vec![0u64; bounds.len() + 1];
        counts[0] = fast;
        // index 10 is the `(1s, 2s]` bucket — comfortably above any threshold a test asks for.
        counts[10] = slow;
        let mut a = attrs(&[("service.name", "api"), ("deployment.environment", "prod")]);
        for (k, v) in extra {
            a.insert(k.to_string(), v.to_string());
        }
        let payload = serde_json::json!({
            "count": fast + slow,
            "sum": 0,
            "bucket_counts": counts,
            "explicit_bounds": bounds.iter().map(|b| *b as f64).collect::<Vec<_>>(),
        });
        MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::HISTOGRAM,
            temporality: Some(TEMPORALITY_DELTA),
            timestamp_nanos: ts,
            histogram: Some(payload.to_string()),
            attributes: a,
            ..Default::default()
        }
    }

    /// Ingest → compact → engine, mirroring `tenants.rs`'s `engine_with`.
    async fn engine_with(points: Vec<MetricPoint>) -> (MetricsQueryEngine, tempfile::TempDir) {
        use photon_core::config::WalConfig;
        use photon_core::metric_record::MetricBatchBuilder;
        use photon_core::metric_schema::MetricSchema;

        let tmp = tempfile::tempdir().unwrap();
        let hot_dir = tmp.path().to_path_buf();
        let schema = MetricSchema::new(&[
            "service.name".to_string(),
            "host.name".to_string(),
            "deployment.environment".to_string(),
            "operation".to_string(),
        ]);
        let wal = Arc::new(
            photon_wal::DiskWal::open_arrow(
                hot_dir.join("wal-metrics"),
                schema.arrow.clone(),
                WalConfig {
                    segment_max_bytes: 1,
                    segment_max_age_secs: 0,
                    group_commit_max_delay_ms: 0,
                },
            )
            .await
            .unwrap(),
        );
        for p in &points {
            let mut b = MetricBatchBuilder::new(&schema);
            b.append(p);
            wal.append(b.finish().unwrap()).await.unwrap();
            wal.sync().await.unwrap();
        }
        let seal = MetricPoint {
            metric_name: "__seal__".to_string(),
            metric_type: metric_type::GAUGE,
            timestamp_nanos: 1,
            value: Some(0.0),
            attributes: attrs(&[("service.name", "__seal__")]),
            ..Default::default()
        };
        let mut tail = MetricBatchBuilder::new(&schema);
        tail.append(&seal);
        wal.append(tail.finish().unwrap()).await.unwrap();
        wal.sync().await.unwrap();

        let storage = photon_storage::Storage {
            hot: Arc::new(object_store::local::LocalFileSystem::new_with_prefix(&hot_dir).unwrap()),
            durable: None,
            hot_dir: Some(hot_dir.clone()),
        };
        let replicator = Arc::new(photon_storage::Replicator::new(storage.clone()));
        let compactor =
            photon_compact::MetricsCompactor::new(wal.clone(), storage, replicator, schema.clone());
        while compactor.run_once().await.unwrap().is_some() {}
        (MetricsQueryEngine::new(hot_dir, schema).unwrap(), tmp)
    }

    async fn state_with(
        points: Vec<MetricPoint>,
        coverage: Option<(i64, i64)>,
    ) -> (crate::AppState, tempfile::TempDir) {
        let (engine, tmp) = engine_with(points).await;
        let store: Arc<dyn SloStore> = Arc::new(SqliteSloStore::open_in_memory().unwrap());
        let state =
            crate::test_state_for_slo(engine, Some(Arc::new(FixedCoverage(coverage))), Some(store));
        (state, tmp)
    }

    fn availability_slo() -> Slo {
        Slo {
            id: "s".into(),
            service: "api".into(),
            environment: "prod".into(),
            operation: String::new(),
            name: "n".into(),
            kind: SloKind::Availability,
            target: 0.99,
            threshold_ms: 0,
            window_days: 1,
        }
    }

    /// Even minute-spaced points across `[NOW - span, NOW)`.
    fn spread(
        name: &str,
        span_secs: i64,
        per_point: f64,
        n: i64,
        extra: &[(&str, &str)],
    ) -> Vec<MetricPoint> {
        (0..n)
            .map(|i| {
                let ts = NOW - span_secs * SEC + (i * span_secs * SEC) / n;
                counter(name, ts, per_point, extra)
            })
            .collect()
    }

    #[tokio::test]
    async fn budget_is_computed_from_rollup_counters() {
        // 1,000 requests, 20 errors, over a day. Target 99% → a budget of 10 events, so 20 bad
        // events is exactly twice the budget: spent, and then some.
        let mut points = spread("photon.service.requests", 86_400, 100.0, 10, &[]);
        points.extend(spread("photon.service.errors", 86_400, 2.0, 10, &[]));
        let (state, _tmp) = state_with(points, Some((NOW - 2 * 86_400 * SEC, NOW))).await;

        let b = compute_budget(&state, availability_slo(), NOW)
            .await
            .unwrap();
        assert_eq!(b.total_events, 1000.0);
        assert_eq!(b.bad_events, 20.0);
        assert!((b.sli.unwrap() - 0.98).abs() < 1e-9);
        assert!((b.budget_events - 10.0).abs() < 1e-9);
        assert!(
            (b.remaining_events + 10.0).abs() < 1e-9,
            "the budget is overspent"
        );
        assert!(b.remaining_fraction.unwrap() < 0.0);
        assert!(!b.partial, "the rollup covers the whole window");
    }

    #[tokio::test]
    async fn budget_over_a_window_longer_than_rollup_coverage_is_reported_partial() {
        // The SLO asks for a day; the rollup only holds the last hour. Answering with the hour's
        // numbers while calling it a day would be a fabricated month-to-date figure.
        let mut points = spread("photon.service.requests", 3_600, 100.0, 10, &[]);
        points.extend(spread("photon.service.errors", 3_600, 1.0, 10, &[]));
        let (state, _tmp) = state_with(points, Some((NOW - 3_600 * SEC, NOW))).await;

        let b = compute_budget(&state, availability_slo(), NOW)
            .await
            .unwrap();
        assert!(b.partial);
        assert_eq!(b.window_start_ms, (NOW - 3_600 * SEC) / 1_000_000);
        assert_eq!(b.total_events, 1000.0);
    }

    #[tokio::test]
    async fn no_rollup_coverage_at_all_is_an_empty_partial_budget_not_an_error() {
        let (state, _tmp) = state_with(vec![], Some((NOW + SEC, NOW + 2 * SEC))).await;
        let b = compute_budget(&state, availability_slo(), NOW)
            .await
            .unwrap();
        assert!(b.partial);
        assert_eq!(b.sli, None);
        assert!(!b.fast.firing && !b.slow.firing);
    }

    #[tokio::test]
    async fn burn_rate_uses_two_windows() {
        // Errors only in the last 5 minutes: the short window is on fire, the 6h window barely
        // notices. The fast pair fires (both its windows see it); the slow pair does not.
        let mut points = spread("photon.service.requests", 6 * 3_600, 100.0, 24, &[]);
        // 80 errors inside the last 5 minutes, against 100 requests there. Over the hour that is
        // 80/500 = 16% bad, or 16× a 1% budget; over six hours it is 80/2500 = 3.2×.
        points.push(counter(
            "photon.service.requests",
            NOW - 120 * SEC,
            100.0,
            &[],
        ));
        points.push(counter("photon.service.errors", NOW - 120 * SEC, 80.0, &[]));
        let (state, _tmp) = state_with(points, Some((NOW - 2 * 86_400 * SEC, NOW))).await;

        let b = compute_budget(&state, availability_slo(), NOW)
            .await
            .unwrap();
        assert!(
            b.fast.short_rate > b.fast.threshold,
            "5m: {}",
            b.fast.short_rate
        );
        assert!(
            b.fast.long_rate > b.fast.threshold,
            "1h: {}",
            b.fast.long_rate
        );
        assert!(b.fast.firing);
        assert!(
            b.slow.long_rate < b.slow.threshold,
            "a 5-minute burst must not fire the 6h pair: {}",
            b.slow.long_rate
        );
        assert!(!b.slow.firing);
    }

    #[tokio::test]
    async fn a_burst_the_short_window_has_moved_past_does_not_fire_the_fast_pair() {
        // The confirming window is what stops a burst that ended 40 minutes ago from paging
        // someone now: the 1h window still sees it, the 5m window does not.
        let mut points = spread("photon.service.requests", 6 * 3_600, 100.0, 24, &[]);
        points.push(counter(
            "photon.service.requests",
            NOW - 2_400 * SEC,
            100.0,
            &[],
        ));
        points.push(counter(
            "photon.service.errors",
            NOW - 2_400 * SEC,
            80.0,
            &[],
        ));
        let (state, _tmp) = state_with(points, Some((NOW - 2 * 86_400 * SEC, NOW))).await;

        let b = compute_budget(&state, availability_slo(), NOW)
            .await
            .unwrap();
        assert!(b.fast.long_rate > b.fast.threshold, "1h still sees it");
        assert_eq!(b.fast.short_rate, 0.0, "5m has moved past it");
        assert!(!b.fast.firing);
    }

    #[tokio::test]
    async fn operation_scoped_slo_ignores_other_operations() {
        // Two operations, only one of them broken. An SLO on the healthy one must not inherit the
        // other's errors — not in its numerator, and not in its denominator either.
        let mut points = spread(
            "photon.operation.requests",
            86_400,
            100.0,
            10,
            &[("operation", "GET /healthy")],
        );
        points.extend(spread(
            "photon.operation.requests",
            86_400,
            100.0,
            10,
            &[("operation", "GET /broken")],
        ));
        points.extend(spread(
            "photon.operation.errors",
            86_400,
            50.0,
            10,
            &[("operation", "GET /broken")],
        ));
        let (state, _tmp) = state_with(points, Some((NOW - 2 * 86_400 * SEC, NOW))).await;

        let mut slo = availability_slo();
        slo.operation = "GET /healthy".into();
        let b = compute_budget(&state, slo, NOW).await.unwrap();
        assert_eq!(b.total_events, 1000.0, "only its own requests");
        assert_eq!(
            b.bad_events, 0.0,
            "and none of the other operation's errors"
        );
        assert_eq!(b.sli, Some(1.0));
    }

    #[tokio::test]
    async fn a_latency_slo_counts_requests_under_the_threshold() {
        // 900 fast (1ms) and 100 slow (~2s) requests. At a 100ms threshold the slow ones are the
        // bad events, so the SLI is 0.9 against a 0.99 target — badly overspent.
        let points: Vec<MetricPoint> = (0..10)
            .map(|i| {
                duration(
                    "photon.service.latency",
                    NOW - 86_400 * SEC + (i * 86_400 * SEC) / 10,
                    90,
                    10,
                    &[],
                )
            })
            .collect();
        let (state, _tmp) = state_with(points, Some((NOW - 2 * 86_400 * SEC, NOW))).await;

        let mut slo = availability_slo();
        slo.kind = SloKind::Latency;
        slo.threshold_ms = 100;
        let b = compute_budget(&state, slo, NOW).await.unwrap();
        assert_eq!(b.total_events, 1000.0);
        assert_eq!(b.bad_events, 100.0);
        assert!((b.sli.unwrap() - 0.9).abs() < 1e-9);
        assert_eq!(
            b.measured_threshold_ms,
            Some(100.0),
            "100ms is a ladder edge"
        );
    }

    #[tokio::test]
    async fn a_latency_threshold_between_ladder_edges_reports_what_it_measured() {
        // 300ms is not on the ladder (…200ms, 500ms…), so it measures at 200ms and says so.
        // Silently measuring 500ms and calling it 300ms is the failure mode this prevents.
        let points: Vec<MetricPoint> = (0..10)
            .map(|i| {
                duration(
                    "photon.service.latency",
                    NOW - 86_400 * SEC + (i * 86_400 * SEC) / 10,
                    90,
                    10,
                    &[],
                )
            })
            .collect();
        let (state, _tmp) = state_with(points, Some((NOW - 2 * 86_400 * SEC, NOW))).await;

        let mut slo = availability_slo();
        slo.kind = SloKind::Latency;
        slo.threshold_ms = 300;
        let b = compute_budget(&state, slo, NOW).await.unwrap();
        assert_eq!(b.measured_threshold_ms, Some(200.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: &str) -> SloInput {
        SloInput {
            name: "checkout availability".into(),
            environment: "prod".into(),
            operation: String::new(),
            kind: kind.into(),
            target: 0.999,
            threshold_ms: if kind == "latency" { 300 } else { 0 },
            window_days: 30,
        }
    }

    fn slo(kind: SloKind, operation: &str) -> Slo {
        Slo {
            id: "x".into(),
            service: "api".into(),
            environment: "prod".into(),
            operation: operation.into(),
            name: "n".into(),
            kind,
            target: 0.999,
            threshold_ms: 300,
            window_days: 30,
        }
    }

    #[tokio::test]
    async fn create_then_list_round_trips() {
        let s = SqliteSloStore::open_in_memory().unwrap();
        let made = s.create("api", &input("availability")).await.unwrap();
        let listed = s.list("api").await.unwrap();
        assert_eq!(listed, vec![made]);
    }

    #[tokio::test]
    async fn a_second_slo_with_the_same_name_here_is_rejected() {
        let s = SqliteSloStore::open_in_memory().unwrap();
        s.create("api", &input("availability")).await.unwrap();
        let again = s.create("api", &input("availability")).await;
        assert!(matches!(again, Err(PhotonError::Query(m)) if m.contains("already exists")));
    }

    #[tokio::test]
    async fn the_same_name_under_a_different_operation_is_a_different_slo() {
        // The point of operation scope: a checkout endpoint can be held to a tighter line than the
        // admin pages in the same service, under the same objective name.
        let s = SqliteSloStore::open_in_memory().unwrap();
        s.create("api", &input("availability")).await.unwrap();
        let mut scoped = input("availability");
        scoped.operation = "POST /checkout".into();
        assert!(s.create("api", &scoped).await.is_ok());
        assert_eq!(s.list("api").await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_latency_slo_without_a_threshold_is_rejected() {
        let s = SqliteSloStore::open_in_memory().unwrap();
        let mut bad = input("latency");
        bad.threshold_ms = 0;
        assert!(matches!(
            s.create("api", &bad).await,
            Err(PhotonError::Query(m)) if m.contains("threshold_ms")
        ));
    }

    #[tokio::test]
    async fn a_target_of_one_is_rejected_because_its_budget_is_zero() {
        let s = SqliteSloStore::open_in_memory().unwrap();
        let mut bad = input("availability");
        bad.target = 1.0;
        assert!(matches!(
            s.create("api", &bad).await,
            Err(PhotonError::Query(_))
        ));
        bad.target = 0.0;
        assert!(matches!(
            s.create("api", &bad).await,
            Err(PhotonError::Query(_))
        ));
    }

    #[tokio::test]
    async fn update_edits_in_place_and_delete_removes() {
        let s = SqliteSloStore::open_in_memory().unwrap();
        let made = s.create("api", &input("availability")).await.unwrap();
        let mut edit = input("availability");
        edit.target = 0.99;
        let updated = s.update(&made.id, &edit).await.unwrap().unwrap();
        assert_eq!(updated.target, 0.99);
        assert_eq!(
            updated.id, made.id,
            "editing a target must not re-key the SLO"
        );
        assert!(s.delete(&made.id).await.unwrap());
        assert!(s.list("api").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn updating_something_that_is_not_there_is_none_not_an_error() {
        let s = SqliteSloStore::open_in_memory().unwrap();
        assert!(s
            .update("nope", &input("availability"))
            .await
            .unwrap()
            .is_none());
        assert!(!s.delete("nope").await.unwrap());
    }

    #[test]
    fn an_operation_scoped_slo_reads_the_operation_family() {
        // Reading the service family for an operation-scoped SLO would measure the whole service;
        // reading the operation family for a service SLO would sum every operation and roughly
        // double it. See `docs/subsystems/rollup.md`.
        assert_eq!(family(&slo(SloKind::Availability, "")), "photon.service.");
        assert_eq!(
            family(&slo(SloKind::Availability, "GET /a")),
            "photon.operation."
        );
    }

    #[test]
    fn the_filter_carries_every_scope_term() {
        let q = slo_filter_query(&slo(SloKind::Availability, "GET /a"));
        assert!(q.contains("service.name:api"));
        assert!(q.contains("deployment.environment:prod"));
        assert!(q.contains("operation:"), "{q}");
        // No operation term at all when the SLO is service-wide — an empty term would filter to
        // the operations literally named "".
        let plain = slo_filter_query(&slo(SloKind::Availability, ""));
        assert!(!plain.contains("operation:"));
    }

    #[test]
    fn burn_rate_is_one_when_the_budget_lasts_exactly_the_window() {
        // 0.1% bad against a 99.9% target: the budget is spent precisely at the window's end.
        assert_eq!(burn_rate(1.0, 1000.0, 0.999).unwrap().round(), 1.0);
        // Ten times the affordable rate.
        assert_eq!(burn_rate(10.0, 1000.0, 0.999).unwrap().round(), 10.0);
        assert_eq!(burn_rate(0.0, 1000.0, 0.999).unwrap(), 0.0);
    }

    #[test]
    fn no_traffic_is_not_a_burn_rate_of_zero() {
        // "Nothing is burning" and "nothing happened" are different facts. Collapsing them would
        // paint a dead service as comfortably within budget.
        assert!(burn_rate(0.0, 0.0, 0.999).is_none());
    }

    #[test]
    fn an_slo_id_is_stable_and_identity_scoped() {
        let a = slo_id("api", "prod", "", "checkout");
        assert_eq!(a, slo_id("api", "prod", "", "checkout"));
        assert_ne!(a, slo_id("api", "staging", "", "checkout"));
        assert_ne!(a, slo_id("api", "prod", "GET /a", "checkout"));
        // The separator matters: without it, ("ap", "iprod") and ("api", "prod") would collide.
        assert_ne!(
            slo_id("ap", "iprod", "", "n"),
            slo_id("api", "prod", "", "n")
        );
    }
}

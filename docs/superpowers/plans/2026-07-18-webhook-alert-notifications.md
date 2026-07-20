# Webhook Alert & Notification Engine — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a system-wide alert engine that evaluates per-signal rules (metrics/logs/traces/RUM) against the query engines and delivers webhooks, with reusable channels, a UI at `/alerts`, and the existing uptime up/down bridged onto the same delivery + incident history.

**Architecture:** A new pure crate `photon-alerts` (domain + state machine + SQLite store + webhook Notifier + evaluation loop) sits beside `photon-uptime`. It never depends on `photon-query`; instead `photon-server` implements a `ConditionSource` trait seam over the three query engines. `photon-api` gains `/api/alerts/*` CRUD (mirroring the uptime handler pattern) and the embedded Vue UI gains an `/alerts` route. Design source of truth: `docs/superpowers/specs/2026-07-18-webhook-alert-notifications-design.md`.

**Tech Stack:** Rust (axum 0.7, tonic, tokio, rusqlite bundled, reqwest, async-trait, serde), DataFusion query engines (indirect via seam), Vue 3 + Vite + Tailwind + Reka UI + TanStack Query/Table/Virtual, bun.

## Global Constraints

- **No commits between tasks.** Each task ends by staging (`git add`) its files and **leaving the working tree dirty** — never run `git commit`. The human batches and commits. (Overrides the writing-plans "commit" step.)
- **Terminology is fixed:** the alert lifecycle is **OK · Pending · Triggered · Resolved**. Never use "firing" in code, JSON, or UI copy.
- **Package manager is `bun`, never `npm`.** Lockfile is `bun.lock`.
- **`PhotonError` gets exactly one new variant, `Alerts(String)`** — a deliberate one-time core addition for the new crate. Do not add others.
- **Dependency co-pinning:** do NOT bump `arrow`/`datafusion`/`object_store`/`parquet`/`opentelemetry-proto`/`tonic`/`prost`. New utility deps (`hmac`, `sha2`) are independent of those clusters and are fine to add.
- **`photon-alerts` must not depend on `photon-query`** (compiler-enforced boundary). It reaches signal data only through the `ConditionSource` seam.
- **DataFusion column access:** dotted names use `col_ref(name)` (not `col()`), but this only matters inside Task 6 which lives in `photon-server` — see `docs/conventions.md`.
- **New `.vue` files may be `<script setup lang="ts">`**; they are gated by `bun run type-check`. Frontend mutations return `{ ok, error }` and don't throw.
- **Keep docs in sync** (Task 17) — required by `CLAUDE.md`.

## Execution Waves (parallelism map)

Worktree isolation is unavailable (no-commit rule), so parallel tasks in a wave **edit strictly disjoint files** and run in the same working tree. Rust parallel tasks serialize on cargo's build-dir lock automatically; frontend tasks serialize on vite. **Backend and frontend run fully in parallel from t=0** — the frontend is built against the documented API contract with the existing mock fallback.

| Wave | Tasks (run concurrently) | Depends on |
|---|---|---|
| **W0** | **T1** (core + crate scaffold) ‖ **T11** (FE router/nav/shell) ‖ **T12** (FE queries+api+mocks) | — |
| **W1** | **T2** (state) ‖ **T3** (notify) ‖ **T4** (store) ‖ **T13** (FE Rules tab) ‖ **T14** (FE dialog) ‖ **T15** (FE Incidents) ‖ **T16** (FE Channels) | T1 (backend); T11+T12 (frontend) |
| **W2** | **T5** (scheduler) | T2, T3, T4 |
| **W3** | **T6** (ConditionSource in server) ‖ **T7** (photon-api handlers) | T1 (+T5 not required by T7) |
| **W4** | **T8** (server wiring) | T5, T6, T7 |
| **W5** | **T9** (uptime bridge) | T3, T4, T8 |
| **W6** | **T10** (e2e + full verify) ‖ **T17** (docs) | all |

Backend critical path: T1 → {T2,T3,T4} → T5 → {T6,T7} → T8 → T9 → T10. Frontend critical path: {T11,T12} → {T13,T14,T15,T16}. The two paths only rejoin at T10.

---

## File Structure

**New crate `crates/photon-alerts/`** (mirrors `photon-uptime` layout):
- `Cargo.toml` — deps: `photon-core`, `async-trait`, `serde`, `serde_json`, `rusqlite`, `reqwest`, `tokio`, `hmac`, `sha2`.
- `src/lib.rs` — module declarations + re-exports.
- `src/model.rs` — all domain types (single source of shared types; every other module + task depends on it).
- `src/source.rs` — `ConditionSource` trait + `SeriesSample`.
- `src/state.rs` — pure `apply()` state machine (T2).
- `src/notify.rs` — `Notifier` trait, `WebhookNotifier`, payload builder, HMAC, `FakeNotifier` (T3).
- `src/store/mod.rs` — `AlertStore` trait + module decls.
- `src/store/sqlite.rs` — `SqliteAlertStore` (T4).
- `src/store/mem.rs` — `MemStore` test fake (T4).
- `src/scheduler.rs` — `run()` loop + `process_sample()` (T5).

**Modified backend:**
- `crates/photon-core/src/lib.rs` — `PhotonError::Alerts` variant (T1).
- `crates/photon-core/src/config.rs` — `AlertsConfig` + `Config.alerts` (T1).
- `Cargo.toml` (root) — workspace member + `hmac`/`sha2` workspace deps (T1).
- `crates/photon-server/src/alerts_source.rs` — `ConditionSource` impl (T6, new file).
- `crates/photon-server/src/main.rs` — `spawn_alerts` + wiring + uptime-bridge notifier (T8, T9).
- `crates/photon-api/src/alerts.rs` — handlers + `AlertsApi` (T7, new file).
- `crates/photon-api/src/lib.rs` — `ApiServer`/`AppStateInner` field, `with_alerts`, routes (T7).
- `crates/photon-uptime/src/model.rs`, `src/store/{mod,sqlite}.rs` — `channel_ids` on monitors (T9).
- `crates/photon-server/tests/alerts_e2e.rs` — end-to-end (T10, new file).

**New/modified frontend** (`frontend/src/`):
- `router/index.js` — `/alerts` route (T11).
- `components/common/NavRail.vue` — Alerts entry (T11).
- `views/AlertsView.vue` — shell + 3 tabs (T11).
- `components/alerts/` — `AlertStatBand.vue`, `AlertRulesTable.vue`, `AlertRuleRow.vue`, `AlertRuleDialog.vue`, `ConditionBuilder.vue`, `IncidentsTable.vue`, `ChannelsGrid.vue`, `ChannelCard.vue`, `ChannelDialog.vue` (T11 stubs → T13–T16).
- `lib/alertsQueries.ts` — composables + mutations (T12).
- `lib/core/api.ts` + mock module — client methods + mock fallback (T12).

**Docs (T17):** `docs/subsystems/alerts.md` (new), `docs/architecture.md`, `docs/frontend.md`, `CLAUDE.md`, `docs/subsystems/uptime.md`.

**Approved UI mockup (frontend markup source of truth):** `.superpowers/brainstorm/37613-1784341076/content/alerts-final.html` — copy its structure/classes into the real components.

---

## Task 1: Core changes + `photon-alerts` scaffold  ·  Wave W0

**Files:**
- Modify: `crates/photon-core/src/lib.rs` (add `PhotonError::Alerts`)
- Modify: `crates/photon-core/src/config.rs` (add `AlertsConfig`, `Config.alerts`)
- Modify: `Cargo.toml` (root — workspace member + `hmac`/`sha2`)
- Create: `crates/photon-alerts/Cargo.toml`
- Create: `crates/photon-alerts/src/lib.rs`
- Create: `crates/photon-alerts/src/model.rs` (COMPLETE)
- Create: `crates/photon-alerts/src/source.rs` (COMPLETE — trait + `SeriesSample`)
- Create: `crates/photon-alerts/src/state.rs` (empty stub — `// filled in Task 2`)
- Create: `crates/photon-alerts/src/notify.rs` (empty stub — `// filled in Task 3`)
- Create: `crates/photon-alerts/src/scheduler.rs` (empty stub — `// filled in Task 5`)
- Create: `crates/photon-alerts/src/store/mod.rs` (COMPLETE — `AlertStore` trait + `pub mod sqlite; pub mod mem;`)
- Create: `crates/photon-alerts/src/store/sqlite.rs` (empty stub — `// filled in Task 4`)
- Create: `crates/photon-alerts/src/store/mem.rs` (empty stub — `// filled in Task 4`)

**Interfaces (Produces — every later task consumes these exact types):** see the `model.rs`, `source.rs`, and `store/mod.rs` code below. This task locks the shared vocabulary so W1 tasks can proceed in parallel.

- [ ] **Step 1: Add the error variant.** In `crates/photon-core/src/lib.rs`, add one arm to the `PhotonError` enum (place it next to the other per-crate variants; match the existing `#[error(...)]` style):

```rust
    #[error("alerts: {0}")]
    Alerts(String),
```

- [ ] **Step 2: Add config.** In `crates/photon-core/src/config.rs`, add the struct and wire it into `Config` (mirror `UptimeConfig`'s `#[serde(default)]` + defaulter-fn pattern):

```rust
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertsConfig {
    /// Default per-rule evaluation cadence when a rule doesn't override it.
    #[serde(default = "AlertsConfig::default_interval")]
    pub interval_default: String, // e.g. "60s"
    /// Max concurrent rule evaluations in flight.
    #[serde(default = "AlertsConfig::default_worker_concurrency")]
    pub worker_concurrency: usize,
}
impl AlertsConfig {
    fn default_interval() -> String { "60s".into() }
    fn default_worker_concurrency() -> usize { 16 }
}
impl Default for AlertsConfig {
    fn default() -> Self {
        Self { interval_default: Self::default_interval(), worker_concurrency: Self::default_worker_concurrency() }
    }
}
```

In `struct Config`, add: `#[serde(default)] pub alerts: AlertsConfig,`. If `Config` has a manual `Default`/test fixtures, add `alerts: AlertsConfig::default()` there too.

- [ ] **Step 3: Root Cargo.toml.** Add `"crates/photon-alerts"` to `[workspace] members`. Under `[workspace.dependencies]` add:

```toml
hmac = "0.12"
sha2 = "0.10"
```

- [ ] **Step 4: Crate Cargo.toml.** Create `crates/photon-alerts/Cargo.toml`:

```toml
[package]
name = "photon-alerts"
version = "0.1.0"
edition = "2021"

[dependencies]
photon-core = { path = "../photon-core" }
async-trait = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
rusqlite = { workspace = true }
reqwest = { workspace = true }
tokio = { workspace = true }
hmac = { workspace = true }
sha2 = { workspace = true }
```

- [ ] **Step 5: `lib.rs`.**

```rust
//! System-wide alert engine: per-signal rules → webhook channels. Pure domain + state machine
//! + SQLite store + delivery; the evaluation loop is generic over the `ConditionSource` seam
//! (implemented in `photon-server` over the query engines).
pub mod model;
pub mod notify;
pub mod scheduler;
pub mod source;
pub mod state;
pub mod store;
```

- [ ] **Step 6: `model.rs` (complete).**

```rust
//! Pure domain types for the alerts vertical. No I/O. All timestamps are Unix ms.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

pub type RuleId = String;
pub type ChannelId = String;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind { Webhook }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ChannelKind,
    pub url: String,
    #[serde(default)]
    pub secret: Option<String>,
    /// Extra request headers as a JSON object, e.g. `{"Authorization":"Bearer x"}`.
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_true() -> bool { true }

#[derive(Clone, Debug, Deserialize)]
pub struct ChannelInput {
    pub name: String,
    #[serde(rename = "type", default = "ChannelInput::default_kind")]
    pub kind: ChannelKind,
    pub url: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
}
impl ChannelInput { fn default_kind() -> ChannelKind { ChannelKind::Webhook } }

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cmp { Gt, Gte, Lt, Lte }
impl Cmp {
    pub fn test(self, value: f64, threshold: f64) -> bool {
        match self { Cmp::Gt => value > threshold, Cmp::Gte => value >= threshold,
                     Cmp::Lt => value < threshold, Cmp::Lte => value <= threshold }
    }
    pub fn symbol(self) -> &'static str {
        match self { Cmp::Gt => ">", Cmp::Gte => ">=", Cmp::Lt => "<", Cmp::Lte => "<=" }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity { Info, Warning, Critical }

// ---- per-signal conditions ----
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricAgg { Avg, Min, Max, Sum, Last, P50, P90, P95, P99, Rate, Increase }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricCondition {
    pub metric_name: String,
    #[serde(default)]
    pub label_filters: BTreeMap<String, String>,
    #[serde(default)]
    pub group_by: Vec<String>,
    pub agg: MetricAgg,
    pub window_secs: i64,
    pub cmp: Cmp,
    pub threshold: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogCondition {
    pub query: String,
    #[serde(default)]
    pub group_by: Option<String>, // only "service.name" supported in v1
    pub window_secs: i64,
    pub cmp: Cmp,
    pub threshold: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind { ErrorRate, LatencyP50, LatencyP90, LatencyP95, LatencyP99, RequestRate }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceCondition {
    pub service: String,
    #[serde(default)]
    pub operation: Option<String>,
    pub kind: TraceKind,
    pub window_secs: i64,
    pub cmp: Cmp,
    pub threshold: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RumKind { VitalLcpP75, VitalInpP75, VitalClsP75, VitalFcpP75, VitalTtfbP75, ErrorCount }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RumCondition {
    pub app_id: String,
    #[serde(default)]
    pub route: Option<String>,
    pub kind: RumKind,
    pub window_secs: i64,
    pub cmp: Cmp,
    pub threshold: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "lowercase")]
pub enum Condition {
    Metrics(MetricCondition),
    Logs(LogCondition),
    Traces(TraceCondition),
    Rum(RumCondition),
}
impl Condition {
    pub fn window_secs(&self) -> i64 {
        match self { Condition::Metrics(c) => c.window_secs, Condition::Logs(c) => c.window_secs,
                     Condition::Traces(c) => c.window_secs, Condition::Rum(c) => c.window_secs }
    }
    pub fn cmp(&self) -> Cmp {
        match self { Condition::Metrics(c) => c.cmp, Condition::Logs(c) => c.cmp,
                     Condition::Traces(c) => c.cmp, Condition::Rum(c) => c.cmp }
    }
    pub fn threshold(&self) -> f64 {
        match self { Condition::Metrics(c) => c.threshold, Condition::Logs(c) => c.threshold,
                     Condition::Traces(c) => c.threshold, Condition::Rum(c) => c.threshold }
    }
    pub fn signal(&self) -> &'static str {
        match self { Condition::Metrics(_) => "metrics", Condition::Logs(_) => "logs",
                     Condition::Traces(_) => "traces", Condition::Rum(_) => "rum" }
    }
    /// Human one-liner for payloads/incidents, e.g. `avg(system.cpu.utilization) > 0.90`.
    pub fn summary(&self) -> String {
        match self {
            Condition::Metrics(c) => format!("{:?}({}) {} {}", c.agg, c.metric_name, c.cmp.symbol(), c.threshold),
            Condition::Logs(c) => format!("count({}) {} {}", c.query, c.cmp.symbol(), c.threshold),
            Condition::Traces(c) => format!("{:?}({}) {} {}", c.kind, c.service, c.cmp.symbol(), c.threshold),
            Condition::Rum(c) => format!("{:?}({}) {} {}", c.kind, c.app_id, c.cmp.symbol(), c.threshold),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub id: RuleId,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub enabled: bool,
    pub condition: Condition,
    pub for_secs: i64,
    pub interval_secs: i64,
    pub severity: Severity,
    pub channel_ids: Vec<ChannelId>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RuleInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub condition: Condition,
    #[serde(default)]
    pub for_secs: i64,
    #[serde(default = "RuleInput::default_interval")]
    pub interval_secs: i64,
    #[serde(default = "RuleInput::default_severity")]
    pub severity: Severity,
    #[serde(default)]
    pub channel_ids: Vec<ChannelId>,
}
impl RuleInput {
    fn default_interval() -> i64 { 60 }
    fn default_severity() -> Severity { Severity::Warning }
}

#[derive(Clone, Debug, Serialize)]
pub struct Incident {
    pub id: i64,
    pub rule_id: RuleId,
    /// Canonical series key (`""` for an aggregate rule).
    pub series_key: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub peak_value: f64,
    pub severity: Severity,
    pub summary: String,
}

/// One evaluated series returned by a `ConditionSource`.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesSample {
    /// Ordered label pairs identifying the series (empty → aggregate).
    pub key: Vec<(String, String)>,
    pub value: f64,
}
impl SeriesSample {
    /// Stable canonical key: `k=v,k=v` sorted by key. `""` when empty.
    pub fn series_key(&self) -> String {
        let mut kv: Vec<String> = self.key.iter().map(|(k, v)| format!("{k}={v}")).collect();
        kv.sort();
        kv.join(",")
    }
    /// The label map as JSON for payloads.
    pub fn labels_json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.key.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect(),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlertPhase { Ok, Pending, Triggered }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeriesState {
    pub phase: AlertPhase,
    pub since: i64,
    pub last_value: f64,
}
impl SeriesState {
    pub fn ok() -> Self { Self { phase: AlertPhase::Ok, since: 0, last_value: 0.0 } }
    pub fn triggered_since(since: i64) -> Self { Self { phase: AlertPhase::Triggered, since, last_value: 0.0 } }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transition { Triggered, Resolved }

#[derive(Clone, Debug)]
pub enum SchedulerCommand { Upsert(Box<Rule>), Remove(RuleId) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cmp_tests() {
        assert!(Cmp::Gt.test(2.0, 1.0));
        assert!(!Cmp::Lte.test(2.0, 1.0));
    }
    #[test]
    fn series_key_is_sorted_and_stable() {
        let s = SeriesSample { key: vec![("b".into(), "2".into()), ("a".into(), "1".into())], value: 0.0 };
        assert_eq!(s.series_key(), "a=1,b=2");
        assert_eq!(SeriesSample { key: vec![], value: 0.0 }.series_key(), "");
    }
    #[test]
    fn condition_roundtrips_tagged() {
        let j = r#"{"signal":"metrics","metric_name":"system.cpu.utilization","agg":"avg","window_secs":300,"cmp":"gt","threshold":0.9}"#;
        let c: Condition = serde_json::from_str(j).unwrap();
        assert_eq!(c.signal(), "metrics");
        assert_eq!(c.threshold(), 0.9);
    }
}
```

- [ ] **Step 7: `source.rs` (complete).**

```rust
//! The seam between the alert engine and the signal data. Implemented in `photon-server` over
//! the three query engines + the uptime store; faked in tests. Keeps `photon-alerts` free of
//! any dependency on `photon-query`.
use crate::model::{Condition, SeriesSample};
use async_trait::async_trait;
use photon_core::PhotonError;

#[async_trait]
pub trait ConditionSource: Send + Sync + 'static {
    /// Sample `cond` as of `now_ms`, returning one value per evaluated series (empty `group_by`
    /// → a single series with an empty key). `Ok(vec![])` = "nothing matched/crossed" (a valid
    /// result that drives resolves); `Err` = "could not evaluate this tick" (state left unchanged).
    async fn sample(&self, cond: &Condition, now_ms: i64) -> Result<Vec<SeriesSample>, PhotonError>;
}
```

- [ ] **Step 8: `store/mod.rs` (complete — trait only; impls in Task 4).**

```rust
//! The alerts persistence seam. Real impl: `SqliteAlertStore` (`sqlite.rs`); test fake: `MemStore`.
use crate::model::{Channel, ChannelInput, Incident, Rule, RuleInput, Severity};
use async_trait::async_trait;
use photon_core::PhotonError;

pub mod mem;
pub mod sqlite;

#[async_trait]
pub trait AlertStore: Send + Sync + 'static {
    // channels
    async fn list_channels(&self) -> Result<Vec<Channel>, PhotonError>;
    async fn get_channel(&self, id: &str) -> Result<Option<Channel>, PhotonError>;
    async fn create_channel(&self, input: ChannelInput) -> Result<Channel, PhotonError>;
    async fn update_channel(&self, id: &str, input: ChannelInput) -> Result<Option<Channel>, PhotonError>;
    async fn delete_channel(&self, id: &str) -> Result<bool, PhotonError>;
    // rules
    async fn list_rules(&self) -> Result<Vec<Rule>, PhotonError>;
    async fn get_rule(&self, id: &str) -> Result<Option<Rule>, PhotonError>;
    async fn create_rule(&self, input: RuleInput) -> Result<Rule, PhotonError>;
    async fn update_rule(&self, id: &str, input: RuleInput) -> Result<Option<Rule>, PhotonError>;
    async fn delete_rule(&self, id: &str) -> Result<bool, PhotonError>;
    async fn set_rule_enabled(&self, id: &str, enabled: bool) -> Result<Option<Rule>, PhotonError>;
    // incidents
    async fn open_incident(&self, rule_id: &str, series_key: &str, started_at: i64, value: f64,
                           severity: Severity, summary: &str) -> Result<i64, PhotonError>;
    async fn bump_incident_peak(&self, incident_id: i64, value: f64) -> Result<(), PhotonError>;
    async fn close_incident(&self, incident_id: i64, ended_at: i64) -> Result<(), PhotonError>;
    /// The open incident id for a (rule, series), if any.
    async fn open_incident_for(&self, rule_id: &str, series_key: &str) -> Result<Option<i64>, PhotonError>;
    /// All currently-open incidents — used to rebuild `Triggered` state on startup.
    async fn list_open_incidents(&self) -> Result<Vec<Incident>, PhotonError>;
    /// `status`: `Some("triggered")` (ended_at IS NULL), `Some("resolved")`, or `None` (all).
    async fn list_incidents(&self, status: Option<&str>, rule_id: Option<&str>, limit: u32)
        -> Result<Vec<Incident>, PhotonError>;
    async fn prune_incidents(&self, before: i64) -> Result<u64, PhotonError>;
}
```

- [ ] **Step 9: empty stubs.** Create `state.rs`, `notify.rs`, `scheduler.rs`, `store/sqlite.rs`, `store/mem.rs`, each containing only a `//! filled in Task N` doc comment (empty modules compile).

- [ ] **Step 10: Verify it compiles.**

Run: `cargo build -p photon-alerts && cargo test -p photon-alerts --lib`
Expected: builds; the 3 `model.rs` tests PASS.

- [ ] **Step 11: Stage (no commit).**

```bash
git add crates/photon-alerts Cargo.toml crates/photon-core/src/lib.rs crates/photon-core/src/config.rs
# DO NOT COMMIT — leave the working tree dirty for batched review.
```

---

## Task 2: Pure state machine (`state.rs`)  ·  Wave W1  ·  ‖ T3, T4

**Files:** Modify `crates/photon-alerts/src/state.rs` · Test: inline `#[cfg(test)]`.

**Interfaces:**
- Consumes (from T1): `AlertPhase`, `SeriesState`, `Transition` (`model.rs`).
- Produces: `pub fn apply(prev: SeriesState, breaching: bool, value: f64, for_secs: i64, now: i64) -> (SeriesState, Option<Transition>)`.

- [ ] **Step 1: Write the failing tests.** Replace `state.rs` with the tests first:

```rust
//! Pure per-(rule,series) lifecycle: Ok → Pending → Triggered. No I/O; exhaustively table-tested.
#[cfg(test)]
mod tests {
    use super::apply;
    use crate::model::{AlertPhase, SeriesState, Transition};

    #[test]
    fn immediate_trigger_when_for_zero() {
        let (s, t) = apply(SeriesState::ok(), true, 9.0, 0, 100);
        assert_eq!(s.phase, AlertPhase::Triggered);
        assert_eq!(t, Some(Transition::Triggered));
    }
    #[test]
    fn pending_then_trigger_after_for_elapses() {
        let (s1, t1) = apply(SeriesState::ok(), true, 9.0, 300, 0);
        assert_eq!(s1.phase, AlertPhase::Pending);
        assert_eq!(t1, None);
        let (s2, t2) = apply(s1, true, 9.5, 300, 200_000); // 200s < 300s
        assert_eq!(s2.phase, AlertPhase::Pending);
        assert_eq!(t2, None);
        let (s3, t3) = apply(s2, true, 9.9, 300, 300_000); // 300s ≥ 300s
        assert_eq!(s3.phase, AlertPhase::Triggered);
        assert_eq!(t3, Some(Transition::Triggered));
    }
    #[test]
    fn no_reemit_while_triggered_and_tracks_last_value() {
        let (s, t) = apply(SeriesState::triggered_since(0), true, 12.0, 0, 500);
        assert_eq!(s.phase, AlertPhase::Triggered);
        assert_eq!(s.last_value, 12.0);
        assert_eq!(t, None);
    }
    #[test]
    fn resolve_from_triggered() {
        let (s, t) = apply(SeriesState::triggered_since(0), false, 1.0, 0, 700);
        assert_eq!(s.phase, AlertPhase::Ok);
        assert_eq!(t, Some(Transition::Resolved));
    }
    #[test]
    fn pending_clears_without_emit() {
        let (s1, _) = apply(SeriesState::ok(), true, 9.0, 300, 0);
        let (s2, t2) = apply(s1, false, 1.0, 300, 50_000);
        assert_eq!(s2.phase, AlertPhase::Ok);
        assert_eq!(t2, None);
    }
    #[test]
    fn ok_stays_ok_without_emit() {
        let (s, t) = apply(SeriesState::ok(), false, 0.0, 0, 1);
        assert_eq!(s.phase, AlertPhase::Ok);
        assert_eq!(t, None);
    }
}
```

- [ ] **Step 2: Run → fail.** `cargo test -p photon-alerts state::` → FAIL (`apply` not found).

- [ ] **Step 3: Implement.** Add above the tests module:

```rust
use crate::model::{AlertPhase, SeriesState, Transition};

pub fn apply(prev: SeriesState, breaching: bool, value: f64, for_secs: i64, now: i64)
    -> (SeriesState, Option<Transition>)
{
    if breaching {
        match prev.phase {
            AlertPhase::Triggered =>
                (SeriesState { phase: AlertPhase::Triggered, since: prev.since, last_value: value }, None),
            AlertPhase::Pending => {
                if now - prev.since >= for_secs * 1000 {
                    (SeriesState { phase: AlertPhase::Triggered, since: now, last_value: value }, Some(Transition::Triggered))
                } else {
                    (SeriesState { phase: AlertPhase::Pending, since: prev.since, last_value: value }, None)
                }
            }
            AlertPhase::Ok => {
                if for_secs <= 0 {
                    (SeriesState { phase: AlertPhase::Triggered, since: now, last_value: value }, Some(Transition::Triggered))
                } else {
                    (SeriesState { phase: AlertPhase::Pending, since: now, last_value: value }, None)
                }
            }
        }
    } else {
        match prev.phase {
            AlertPhase::Triggered =>
                (SeriesState { phase: AlertPhase::Ok, since: now, last_value: value }, Some(Transition::Resolved)),
            _ => (SeriesState { phase: AlertPhase::Ok, since: now, last_value: value }, None),
        }
    }
}
```

Note: `for_secs` is **seconds**; `since`/`now` are **ms** — hence `for_secs * 1000`.

- [ ] **Step 4: Run → pass.** `cargo test -p photon-alerts state::` → all 6 PASS.

- [ ] **Step 5: Stage.** `git add crates/photon-alerts/src/state.rs` — no commit.

---

## Task 3: Webhook delivery (`notify.rs`)  ·  Wave W1  ·  ‖ T2, T4

**Files:** Modify `crates/photon-alerts/src/notify.rs` · Test: inline.

**Interfaces:**
- Consumes (T1): `Channel`, `Rule`, `SeriesSample`, `Severity`.
- Produces:
  - `pub enum NotifyStatus { Triggered, Resolved }`
  - `pub fn build_payload(rule: &Rule, labels: &serde_json::Value, status: NotifyStatus, value: f64, threshold: f64, started_at: i64, at: i64, incident_id: i64) -> serde_json::Value`
  - `#[async_trait] pub trait Notifier { async fn deliver(&self, channel: &Channel, payload: serde_json::Value); }`
  - `pub struct WebhookNotifier` + `WebhookNotifier::new()`
  - `pub struct FakeNotifier` (records `(channel_id, status)`), test double.

- [ ] **Step 1: Failing tests.**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn rule() -> Rule {
        Rule { id: "r1".into(), name: "High CPU".into(), description: None, enabled: true,
            condition: Condition::Metrics(MetricCondition { metric_name: "system.cpu.utilization".into(),
                label_filters: Default::default(), group_by: vec!["host.name".into()], agg: MetricAgg::Avg,
                window_secs: 300, cmp: Cmp::Gt, threshold: 0.9 }),
            for_secs: 300, interval_secs: 60, severity: Severity::Warning, channel_ids: vec!["c1".into()],
            created_at: 0, updated_at: 0 }
    }

    #[test]
    fn payload_has_stable_shape_and_status() {
        let labels = serde_json::json!({ "host.name": "web-01" });
        let p = build_payload(&rule(), &labels, NotifyStatus::Triggered, 0.94, 0.90, 1000, 2000, 7);
        assert_eq!(p["status"], "triggered");
        assert_eq!(p["rule"]["name"], "High CPU");
        assert_eq!(p["rule"]["signal"], "metrics");
        assert_eq!(p["series"]["host.name"], "web-01");
        assert_eq!(p["value"], 0.94);
        assert_eq!(p["threshold"], 0.90);
        assert_eq!(p["incident_id"], 7);
    }

    #[test]
    fn hmac_signature_is_deterministic() {
        let sig = sign("shhh", br#"{"a":1}"#);
        assert_eq!(sig, sign("shhh", br#"{"a":1}"#));
        assert!(sig.starts_with("sha256="));
        assert_ne!(sig, sign("other", br#"{"a":1}"#));
    }

    #[tokio::test]
    async fn fake_notifier_records() {
        let n = FakeNotifier::default();
        let ch = Channel { id: "c1".into(), name: "ops".into(), kind: ChannelKind::Webhook,
            url: "http://x".into(), secret: None, headers: None, created_at: 0, updated_at: 0 };
        n.deliver(&ch, serde_json::json!({"status":"resolved"})).await;
        assert_eq!(n.calls.lock().unwrap().as_slice(), &[("c1".to_string(), "resolved".to_string())]);
    }
}
```

- [ ] **Step 2: Run → fail.** `cargo test -p photon-alerts notify::` → FAIL.

- [ ] **Step 3: Implement.** Above the tests:

```rust
//! Alert delivery. v1 = a generic JSON webhook per channel, optional HMAC-SHA256 signature and
//! custom headers. Delivery is detached, retried ≤3×, non-fatal — never blocks the eval loop.
use crate::model::{Channel, Rule};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum NotifyStatus { Triggered, Resolved }
impl NotifyStatus {
    fn as_str(self) -> &'static str { match self { NotifyStatus::Triggered => "triggered", NotifyStatus::Resolved => "resolved" } }
}

pub fn build_payload(rule: &Rule, labels: &serde_json::Value, status: NotifyStatus,
    value: f64, threshold: f64, started_at: i64, at: i64, incident_id: i64) -> serde_json::Value
{
    serde_json::json!({
        "status": status.as_str(),
        "rule": { "id": rule.id, "name": rule.name,
                  "severity": rule.severity, "signal": rule.condition.signal() },
        "series": labels,
        "condition": rule.condition.summary(),
        "value": value, "threshold": threshold,
        "started_at": started_at, "at": at, "incident_id": incident_id,
    })
}

/// `sha256=<hex>` HMAC of `body` under `secret`.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut hex = String::from("sha256=");
    for b in bytes { hex.push_str(&format!("{b:02x}")); }
    hex
}

#[async_trait]
pub trait Notifier: Send + Sync {
    /// Fire-and-forget: returns immediately; delivery + retries happen in a detached task.
    async fn deliver(&self, channel: &Channel, payload: serde_json::Value);
}

pub struct WebhookNotifier { client: reqwest::Client }
impl WebhookNotifier {
    pub fn new() -> Self {
        Self { client: reqwest::Client::builder().timeout(Duration::from_secs(10)).build().expect("reqwest client") }
    }
}
impl Default for WebhookNotifier { fn default() -> Self { Self::new() } }

#[async_trait]
impl Notifier for WebhookNotifier {
    async fn deliver(&self, channel: &Channel, payload: serde_json::Value) {
        let client = self.client.clone();
        let url = channel.url.clone();
        let secret = channel.secret.clone();
        let headers = channel.headers.clone();
        let body = serde_json::to_vec(&payload).unwrap_or_default();
        tokio::spawn(async move {
            for attempt in 1..=3u32 {
                let mut req = client.post(&url)
                    .header("content-type", "application/json")
                    .body(body.clone());
                if let Some(s) = &secret { req = req.header("X-Photon-Signature", sign(s, &body)); }
                if let Some(serde_json::Value::Object(map)) = &headers {
                    for (k, v) in map { if let Some(v) = v.as_str() { req = req.header(k, v); } }
                }
                match req.send().await {
                    Ok(r) if r.status().is_success() => return,
                    Ok(r) => eprintln!("alert webhook {url}: HTTP {} (attempt {attempt})", r.status()),
                    Err(e) => eprintln!("alert webhook {url}: {e} (attempt {attempt})"),
                }
                if attempt < 3 { tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await; }
            }
        });
    }
}

/// Test double: records `(channel_id, status_string)` per `deliver`.
#[derive(Default)]
pub struct FakeNotifier { pub calls: std::sync::Mutex<Vec<(String, String)>> }
#[async_trait]
impl Notifier for FakeNotifier {
    async fn deliver(&self, channel: &Channel, payload: serde_json::Value) {
        let status = payload.get("status").and_then(|v| v.as_str()).unwrap_or("").to_string();
        self.calls.lock().unwrap().push((channel.id.clone(), status));
    }
}
```

- [ ] **Step 4: Run → pass.** `cargo test -p photon-alerts notify::` → PASS.
- [ ] **Step 5: Stage.** `git add crates/photon-alerts/src/notify.rs` — no commit.

---

## Task 4: SQLite store + Mem fake (`store/sqlite.rs`, `store/mem.rs`)  ·  Wave W1  ·  ‖ T2, T3

**Files:** Modify `crates/photon-alerts/src/store/sqlite.rs`, `crates/photon-alerts/src/store/mem.rs` · Test: inline in each.

**Interfaces:**
- Consumes (T1): `AlertStore` trait, all model types.
- Produces: `pub struct SqliteAlertStore` + `SqliteAlertStore::open(path: &str) -> Result<Self, PhotonError>`; `pub struct MemStore` + `MemStore::new()`.

**Reference:** copy the connection/PRAGMA/`err` helper pattern verbatim from `crates/photon-uptime/src/store/sqlite.rs:1-110` (busy-timeout PRAGMA, `Mutex<Connection>`, `fn err(e)`). Store `condition`/`channel_ids`/`headers` as JSON `TEXT` (`serde_json::to_string`). Generate ids with a counter+timestamp scheme like uptime's monitor ids (read `create_monitor` there).

- [ ] **Step 1: Schema + open.** In `sqlite.rs`, define:

```rust
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS alert_channels (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
  url TEXT NOT NULL, secret TEXT, headers TEXT, created_at INTEGER, updated_at INTEGER);
CREATE TABLE IF NOT EXISTS alert_rules (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT, enabled INTEGER NOT NULL,
  signal TEXT NOT NULL, condition TEXT NOT NULL, for_secs INTEGER NOT NULL,
  interval_secs INTEGER NOT NULL, severity TEXT NOT NULL, channel_ids TEXT NOT NULL,
  created_at INTEGER, updated_at INTEGER);
CREATE TABLE IF NOT EXISTS alert_incidents (
  id INTEGER PRIMARY KEY AUTOINCREMENT, rule_id TEXT NOT NULL, series_key TEXT NOT NULL,
  started_at INTEGER NOT NULL, ended_at INTEGER, peak_value REAL NOT NULL,
  severity TEXT NOT NULL, summary TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_alert_incidents_open ON alert_incidents(rule_id, series_key) WHERE ended_at IS NULL;
"#;
```

`open()` mirrors uptime: `Connection::open(path)`, set the busy PRAGMA batch, `execute_batch(SCHEMA)`, wrap in `Mutex`.

- [ ] **Step 2: Write failing store tests** (put in `sqlite.rs`, use a temp file via `std::env::temp_dir()` + a per-test unique name derived from a static `AtomicU64` counter — do NOT use `Date`/random):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp() -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!("photon-alerts-test-{}.db", N.fetch_add(1, Ordering::Relaxed)));
        p.to_string_lossy().into_owned()
    }
    fn cond() -> Condition {
        Condition::Metrics(MetricCondition { metric_name: "m".into(), label_filters: Default::default(),
            group_by: vec![], agg: MetricAgg::Avg, window_secs: 60, cmp: Cmp::Gt, threshold: 1.0 })
    }
    fn rule_input() -> RuleInput {
        RuleInput { name: "r".into(), description: None, enabled: true, condition: cond(),
            for_secs: 0, interval_secs: 60, severity: Severity::Warning, channel_ids: vec![] }
    }

    #[tokio::test]
    async fn rule_crud_roundtrips() {
        let s = SqliteAlertStore::open(&tmp()).unwrap();
        let r = s.create_rule(rule_input()).await.unwrap();
        assert_eq!(s.list_rules().await.unwrap().len(), 1);
        assert!(s.get_rule(&r.id).await.unwrap().is_some());
        let off = s.set_rule_enabled(&r.id, false).await.unwrap().unwrap();
        assert!(!off.enabled);
        assert!(s.delete_rule(&r.id).await.unwrap());
        assert!(s.list_rules().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn incident_open_close_and_rebuild() {
        let s = SqliteAlertStore::open(&tmp()).unwrap();
        let id = s.open_incident("r1", "host=web-01", 100, 0.94, Severity::Warning, "avg>0.9").await.unwrap();
        assert_eq!(s.open_incident_for("r1", "host=web-01").await.unwrap(), Some(id));
        assert_eq!(s.list_open_incidents().await.unwrap().len(), 1);
        s.bump_incident_peak(id, 0.99).await.unwrap();
        s.close_incident(id, 200).await.unwrap();
        assert_eq!(s.open_incident_for("r1", "host=web-01").await.unwrap(), None);
        assert_eq!(s.list_incidents(Some("resolved"), None, 10).await.unwrap().len(), 1);
        assert_eq!(s.list_incidents(Some("triggered"), None, 10).await.unwrap().len(), 0);
    }
}
```

- [ ] **Step 3: Run → fail.** `cargo test -p photon-alerts store::sqlite` → FAIL.

- [ ] **Step 4: Implement `SqliteAlertStore`.** Implement `AlertStore` for `SqliteAlertStore`. Serialize `condition` via `serde_json::to_string(&input.condition)`, `channel_ids` via `serde_json::to_string`, `headers` passthrough. `severity`/`kind` serialize via `serde_json::to_value(..).as_str()` or a small `match`. Row→struct helpers deserialize the JSON columns. `list_incidents(status)` maps `"triggered"`→`WHERE ended_at IS NULL`, `"resolved"`→`WHERE ended_at IS NOT NULL`, ordered `started_at DESC LIMIT ?`. `open_incident` returns `conn.last_insert_rowid()`. Follow the exact `self.conn.lock().unwrap()` + `params![]` + `.map_err(err)` idiom from uptime's `sqlite.rs`.

- [ ] **Step 5: Run → pass.** `cargo test -p photon-alerts store::sqlite` → PASS.

- [ ] **Step 6: Implement `MemStore` (mem.rs)** — an in-memory `AlertStore` backed by `Mutex<HashMap>`s + an incident id counter (mirror `crates/photon-uptime/src/store/mod.rs`'s `MemStore`). Add one test: `create_rule` then `list_rules` returns it. Run `cargo test -p photon-alerts store::mem` → PASS.

- [ ] **Step 7: Stage.** `git add crates/photon-alerts/src/store` — no commit.

---

## Task 5: Evaluation loop (`scheduler.rs`)  ·  Wave W2  ·  needs T2, T3, T4

**Files:** Modify `crates/photon-alerts/src/scheduler.rs` · Test: inline (uses `MemStore` + `FakeNotifier` + a fake `ConditionSource`).

**Interfaces:**
- Consumes: `apply` (T2), `Notifier`/`build_payload`/`NotifyStatus` (T3), `AlertStore` (T4), `ConditionSource`/`SeriesSample` (T1), model types.
- Produces:
  - `pub async fn process_sample<S: AlertStore, N: Notifier>(store, notifier, rule: &Rule, states: &mut HashMap<String, SeriesState>, samples: Vec<SeriesSample>, now: i64) -> Result<(), PhotonError>`
  - `pub async fn run<S, C, N>(store: Arc<S>, source: Arc<C>, notifier: Arc<N>, cmd_rx: mpsc::Receiver<SchedulerCommand>, concurrency: usize)` where `S: AlertStore, C: ConditionSource, N: Notifier + 'static`.

- [ ] **Step 1: Failing test for `process_sample`.**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::notify::FakeNotifier;
    use crate::store::mem::MemStore;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn rule(for_secs: i64) -> Rule {
        Rule { id: "r1".into(), name: "cpu".into(), description: None, enabled: true,
            condition: Condition::Metrics(MetricCondition { metric_name: "m".into(), label_filters: Default::default(),
                group_by: vec!["host.name".into()], agg: MetricAgg::Avg, window_secs: 60, cmp: Cmp::Gt, threshold: 0.9 }),
            for_secs, interval_secs: 60, severity: Severity::Warning, channel_ids: vec!["c1".into()],
            created_at: 0, updated_at: 0 }
    }
    fn sample(host: &str, v: f64) -> SeriesSample {
        SeriesSample { key: vec![("host.name".into(), host.into())], value: v }
    }

    #[tokio::test]
    async fn per_series_trigger_and_resolve_notify_once_each() {
        let store = Arc::new(MemStore::new());
        // channel c1 must exist for delivery fan-out to resolve a Channel
        store.create_channel(ChannelInput { name: "c1".into(), kind: ChannelKind::Webhook,
            url: "http://x".into(), secret: None, headers: None }).await.unwrap();
        // rewrite the rule's channel_ids to the created channel id:
        let created = &store.list_channels().await.unwrap()[0];
        let mut r = rule(0); r.channel_ids = vec![created.id.clone()];
        let notifier = FakeNotifier::default();
        let mut states: HashMap<String, SeriesState> = HashMap::new();

        // web-01 breaches, web-02 fine → one Triggered notify.
        process_sample(&*store, &notifier, &r, &mut states, vec![sample("web-01", 0.95), sample("web-02", 0.1)], 1000).await.unwrap();
        assert_eq!(notifier.calls.lock().unwrap().len(), 1);
        assert_eq!(store.list_open_incidents().await.unwrap().len(), 1);

        // web-01 recovers → one Resolved notify; incident closed.
        process_sample(&*store, &notifier, &r, &mut states, vec![sample("web-01", 0.2), sample("web-02", 0.1)], 2000).await.unwrap();
        let calls = notifier.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].1, "resolved");
        assert_eq!(store.list_open_incidents().await.unwrap().len(), 0);
    }
}
```

- [ ] **Step 2: Run → fail.** `cargo test -p photon-alerts scheduler::` → FAIL.

- [ ] **Step 3: Implement `process_sample` + `run`.**

```rust
//! The alert evaluation loop: owns per-(rule,series) state, samples due rules via `ConditionSource`,
//! folds each series through `apply`, opens/closes incidents, and fans deliveries out to channels.
//! Mirrors `photon-uptime::scheduler`. Non-fatal: an eval/query error leaves state unchanged.
use crate::model::*;
use crate::notify::{build_payload, NotifyStatus, Notifier};
use crate::source::ConditionSource;
use crate::state::apply;
use crate::store::AlertStore;
use photon_core::PhotonError;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

/// Fold one full sample of a rule's series into durable state + notifications.
pub async fn process_sample<S: AlertStore, N: Notifier>(
    store: &S, notifier: &N, rule: &Rule,
    states: &mut HashMap<String, SeriesState>, samples: Vec<SeriesSample>, now: i64,
) -> Result<(), PhotonError> {
    let threshold = rule.condition.threshold();
    let cmp = rule.condition.cmp();
    let mut seen: Vec<String> = Vec::with_capacity(samples.len());

    for s in &samples {
        let key = s.series_key();
        seen.push(key.clone());
        let prev = *states.get(&key).unwrap_or(&SeriesState::ok());
        let breaching = cmp.test(s.value, threshold);
        let (next, transition) = apply(prev, breaching, s.value, rule.for_secs, now);
        states.insert(key.clone(), next);
        apply_transition(store, notifier, rule, &key, s, transition, now).await?;
        if matches!(next.phase, AlertPhase::Triggered) {
            if let Some(id) = store.open_incident_for(&rule.id, &key).await? {
                store.bump_incident_peak(id, s.value).await?;
            }
        }
    }

    // Series that were Triggered but vanished from a successful sample → resolve them.
    let vanished: Vec<String> = states.keys().filter(|k| !seen.contains(k)).cloned().collect();
    for key in vanished {
        let prev = states[&key];
        if matches!(prev.phase, AlertPhase::Triggered) {
            let s = SeriesSample { key: vec![], value: prev.last_value };
            apply_transition(store, notifier, rule, &key, &s, Some(Transition::Resolved), now).await?;
        }
        states.insert(key, SeriesState::ok());
    }
    Ok(())
}

async fn apply_transition<S: AlertStore, N: Notifier>(
    store: &S, notifier: &N, rule: &Rule, key: &str, s: &SeriesSample,
    transition: Option<Transition>, now: i64,
) -> Result<(), PhotonError> {
    let threshold = rule.condition.threshold();
    match transition {
        Some(Transition::Triggered) => {
            let id = store.open_incident(&rule.id, key, now, s.value, rule.severity, &rule.condition.summary()).await?;
            fan_out(store, notifier, rule, s, NotifyStatus::Triggered, s.value, threshold, now, now, id).await?;
        }
        Some(Transition::Resolved) => {
            let (started, id) = match store.open_incident_for(&rule.id, key).await? {
                Some(id) => { store.close_incident(id, now).await?; (now, id) }
                None => (now, 0),
            };
            fan_out(store, notifier, rule, s, NotifyStatus::Resolved, s.value, threshold, started, now, id).await?;
        }
        None => {}
    }
    Ok(())
}

async fn fan_out<S: AlertStore, N: Notifier>(
    store: &S, notifier: &N, rule: &Rule, s: &SeriesSample, status: NotifyStatus,
    value: f64, threshold: f64, started_at: i64, at: i64, incident_id: i64,
) -> Result<(), PhotonError> {
    let payload = build_payload(rule, &s.labels_json(), status, value, threshold, started_at, at, incident_id);
    for cid in &rule.channel_ids {
        match store.get_channel(cid).await? {
            Some(ch) => notifier.deliver(&ch, payload.clone()).await,
            None => eprintln!("alert rule {}: channel {cid} not found, skipping", rule.id),
        }
    }
    Ok(())
}

struct Slot { rule: Rule, states: HashMap<String, SeriesState>, next_due: i64 }

pub async fn run<S, C, N>(
    store: Arc<S>, source: Arc<C>, notifier: Arc<N>,
    mut cmd_rx: mpsc::Receiver<SchedulerCommand>, concurrency: usize,
) where S: AlertStore, C: ConditionSource, N: Notifier + 'static {
    let mut slots: HashMap<RuleId, Slot> = HashMap::new();
    // Seed rules + rebuild Triggered state from open incidents.
    let open = store.list_open_incidents().await.unwrap_or_default();
    if let Ok(rules) = store.list_rules().await {
        for rule in rules {
            let mut states = HashMap::new();
            for inc in open.iter().filter(|i| i.rule_id == rule.id) {
                states.insert(inc.series_key.clone(), SeriesState::triggered_since(inc.started_at));
            }
            slots.insert(rule.id.clone(), Slot { rule, states, next_due: now_ms() });
        }
    }

    let sem = Arc::new(Semaphore::new(concurrency.max(1)));
    let (done_tx, mut done_rx) = mpsc::channel::<(RuleId, Vec<SeriesSample>)>(1024);
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(1000));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = now_ms();
                for slot in slots.values_mut() {
                    if !slot.rule.enabled || slot.next_due > now { continue; }
                    slot.next_due = now + slot.rule.interval_secs.max(1) * 1000;
                    let (source, sem, done_tx) = (source.clone(), sem.clone(), done_tx.clone());
                    let (rid, cond) = (slot.rule.id.clone(), slot.rule.condition.clone());
                    tokio::spawn(async move {
                        let Ok(_permit) = sem.acquire_owned().await else { return };
                        match source.sample(&cond, now_ms()).await {
                            Ok(samples) => { let _ = done_tx.send((rid, samples)).await; }
                            Err(e) => eprintln!("alert eval {rid}: {e}"), // state unchanged: skip tick
                        }
                    });
                }
            }
            Some((rid, samples)) = done_rx.recv() => {
                if let Some(slot) = slots.get_mut(&rid) {
                    let (rule, states) = (slot.rule.clone(), &mut slot.states);
                    if let Err(e) = process_sample(&*store, &*notifier, &rule, states, samples, now_ms()).await {
                        eprintln!("alert process {rid}: {e}");
                    }
                }
            }
            cmd = cmd_rx.recv() => match cmd {
                Some(SchedulerCommand::Upsert(r)) => {
                    let states = slots.remove(&r.id).map(|s| s.states).unwrap_or_default();
                    slots.insert(r.id.clone(), Slot { rule: *r, states, next_due: now_ms() });
                }
                Some(SchedulerCommand::Remove(id)) => { slots.remove(&id); }
                None => break,
            }
        }
    }
}
```

- [ ] **Step 4: Run → pass.** `cargo test -p photon-alerts scheduler::` → PASS. Then `cargo test -p photon-alerts` (whole crate) → PASS; `cargo clippy -p photon-alerts --all-targets` → clean.
- [ ] **Step 5: Stage.** `git add crates/photon-alerts/src/scheduler.rs` — no commit.

---

## Task 6: `ConditionSource` over the query engines (`photon-server`)  ·  Wave W3  ·  ‖ T7

**Files:** Create `crates/photon-server/src/alerts_source.rs`; register `mod alerts_source;` in `crates/photon-server/src/main.rs`. Test: inline where feasible + covered by T10 e2e.

**Interfaces:**
- Consumes: `ConditionSource`, `SeriesSample`, `Condition` + the four condition structs (T1); the query engines `QueryEngine`, `SpanQueryEngine`, `MetricsQueryEngine` (photon-query); the uptime store is **not** needed here (uptime bridges separately in T9).
- Produces: `pub struct EngineConditionSource { logs: QueryEngine, spans: SpanQueryEngine, metrics: MetricsQueryEngine }` implementing `ConditionSource`.

**Engine-call reference (verified signatures):**
- metrics → `MetricsQueryEngine::query_series(MetricSeriesRequest) -> QuerySeriesResult` (`crates/photon-query/src/metric_query.rs:153`). Build `MetricSeriesRequest` the way `crates/photon-api/src/metrics.rs::query` does; window = `[now - window_secs*1e9, now]` ns; buckets small (e.g. 1). Extract the per-series aggregate matching `agg` from the result's series.
- logs → `QueryEngine::count_matching(QueryRequest) -> u64` (`crates/photon-query/src/count.rs:14`). Parse `cond.query` with the log grammar (`photon_core::query`) into `ResolvedQuery` exactly as `crates/photon-api/src/search.rs` does; set `QueryRequest { start_ts_nanos, end_ts_nanos, services, severities: vec![], text: None, query: Some(resolved), limit: 0 }`.
- traces → `SpanQueryEngine::red_metrics(SpanQueryRequest, RedGroup, &thresholds, default_ms) -> Vec<RedRow>` (`crates/photon-query/src/red.rs:68`) for `error_rate`/`request_rate`; `SpanQueryEngine::latency(..)` (`span_latency.rs:45`) for `latency_pXX`. Mirror `crates/photon-api/src/red.rs` + `traces_agg.rs` for request construction. `RedRow` carries the error %/rps; pick the row for `cond.service`.
- rum → `MetricsQueryEngine::rum_vitals(service, start_ns, end_ns) -> Vec<VitalSummary>` (`rum_vitals.rs:140`) for vitals (take the p75 for the requested vital); `QueryEngine::rum_errors(service, start_ns, end_ns, limit, route, query) -> Vec<ErrorIssue>` (`rum_errors.rs:105`) for `error_count` (sum issue counts). Here `service` = `app_id`.

- [ ] **Step 1: Verify engine `Clone`-ability.** Read the struct defs of `QueryEngine`/`SpanQueryEngine`/`MetricsQueryEngine` in `photon-query`. If they derive `Clone` (Arc-backed), `EngineConditionSource` can hold owned clones. If NOT `Clone`, hold `Arc<…>` instead and have `photon-server` construct one shared instance passed to both this source and `ApiServer::new` — **decide here and record it**, because T8 wires it.

- [ ] **Step 2: Write the struct + `sample` skeleton.**

```rust
//! Implements the alerts `ConditionSource` seam over the three read engines. Lives in
//! photon-server (the only crate allowed to depend on both photon-alerts and photon-query).
use async_trait::async_trait;
use photon_alerts::model::*;
use photon_alerts::source::ConditionSource;
use photon_core::PhotonError;
use photon_query::{MetricsQueryEngine, QueryEngine, SpanQueryEngine};

pub struct EngineConditionSource {
    pub logs: QueryEngine,       // or Arc<…> per Step 1
    pub spans: SpanQueryEngine,
    pub metrics: MetricsQueryEngine,
}

#[async_trait]
impl ConditionSource for EngineConditionSource {
    async fn sample(&self, cond: &Condition, now_ms: i64) -> Result<Vec<SeriesSample>, PhotonError> {
        let now_ns = now_ms * 1_000_000;
        match cond {
            Condition::Metrics(c) => self.sample_metrics(c, now_ns).await,
            Condition::Logs(c) => self.sample_logs(c, now_ns).await,
            Condition::Traces(c) => self.sample_traces(c, now_ns).await,
            Condition::Rum(c) => self.sample_rum(c, now_ns).await,
        }
    }
}
```

- [ ] **Step 3: Implement the four `sample_*` methods** per the Engine-call reference. Each returns `Vec<SeriesSample>` where `key` carries the `group_by` labels (metrics), `[("service.name", svc)]` (traces/logs-grouped), or `[]` (aggregate). Reuse the request-construction helpers from the matching `photon-api` handler modules — do not re-derive DataFusion expressions here. Window start = `now_ns - c.window_secs * 1_000_000_000`.

- [ ] **Step 4: Add a metrics-path unit test** using whatever in-memory engine fixture `photon-query`'s own tests use (grep `photon-query/tests` + `metric_query.rs` `#[cfg(test)]` for the constructor). If constructing an engine in a unit test is impractical, note that and rely on the T10 e2e to cover this path (state that explicitly — do not leave it silently untested).

- [ ] **Step 5: Build.** `cargo build -p photon-server` → compiles.
- [ ] **Step 6: Stage.** `git add crates/photon-server/src/alerts_source.rs crates/photon-server/src/main.rs` — no commit.

---

## Task 7: API handlers + routes (`photon-api`)  ·  Wave W3  ·  ‖ T6

**Files:** Create `crates/photon-api/src/alerts.rs`; modify `crates/photon-api/src/lib.rs`. Test: inline `#[tokio::test]` like `uptime.rs`'s `create_then_list_via_api_layer`.

**Interfaces:**
- Consumes: `AlertStore`, `ConditionSource`, model types (photon-alerts); `mpsc::Sender<SchedulerCommand>`.
- Produces: `pub struct AlertsApi { pub store: Arc<dyn AlertStore>, pub cmd_tx: mpsc::Sender<SchedulerCommand>, pub source: Arc<dyn ConditionSource> }`; `pub fn with_alerts(self, Option<AlertsApi>) -> Self` on `ApiServer`; handlers.

**Reference:** copy the handler/`ApiErr`/`State<AppState>` shape from `crates/photon-api/src/uptime.rs` verbatim (it's the closest twin). Copy the `with_uptime` + `AppStateInner.uptime` + route-registration pattern from `lib.rs:150,210,275`.

- [ ] **Step 1:** In `lib.rs`, add `alerts: Option<alerts::AlertsApi>` to both `ApiServer` and `AppStateInner`; initialize `alerts: None` in `ApiServer::new`; copy it into `AppState` where the other optional handles are copied; add:

```rust
pub fn with_alerts(mut self, alerts: Option<alerts::AlertsApi>) -> Self { self.alerts = alerts; self }
```

Add `mod alerts;` and register routes in the `protected` router (only meaningful when attached; handlers 503 when `s.alerts` is `None`, matching how uptime handlers behave):

```rust
.route("/alerts/rules", get(alerts::list_rules).post(alerts::create_rule))
.route("/alerts/rules/:id", get(alerts::get_rule).patch(alerts::update_rule).delete(alerts::delete_rule))
.route("/alerts/rules/:id/test", post(alerts::test_rule))
.route("/alerts/preview", post(alerts::preview))
.route("/alerts/channels", get(alerts::list_channels).post(alerts::create_channel))
.route("/alerts/channels/:id", get(alerts::get_channel).patch(alerts::update_channel).delete(alerts::delete_channel))
.route("/alerts/channels/:id/test", post(alerts::test_channel))
.route("/alerts/incidents", get(alerts::list_incidents))
```

- [ ] **Step 2:** In `alerts.rs`, define `AlertsApi` and the handlers. CRUD delegates to `store`; `create_rule`/`update_rule`/`delete_rule` **also** send the matching `SchedulerCommand` (`Upsert(Box::new(rule))` / `Remove(id)`) on `cmd_tx` so the loop live-reloads. `preview` takes a `Condition` body, calls `source.sample(&cond, now_ms())`, and returns each series with its value + a `would_trigger` bool (`cond.cmp().test(value, cond.threshold())`). `test_channel` builds a sample payload and calls the same delivery path (construct a throwaway `WebhookNotifier`). `list_incidents` reads `?status=&rule_id=&limit=` query params (reuse `query_params.rs` helpers).

- [ ] **Step 3:** Add a `create_then_list_via_api_layer`-style test: build an `ApiServer` with `with_alerts(Some(AlertsApi { store: Arc::new(MemStore::new()), cmd_tx, source: Arc::new(fake) }))`, POST a rule, GET the list, assert 1. (Copy the harness from `uptime.rs`'s test.)

- [ ] **Step 4:** `cargo test -p photon-api alerts` → PASS; `cargo build -p photon-api` → compiles.
- [ ] **Step 5: Stage.** `git add crates/photon-api/src/alerts.rs crates/photon-api/src/lib.rs` — no commit.

---

## Task 8: Server wiring (`photon-server`)  ·  Wave W4  ·  needs T5, T6, T7

**Files:** Modify `crates/photon-server/src/main.rs`. Covered by T10 e2e.

**Reference:** mirror `spawn_uptime` (`main.rs:777`) and the `.with_uptime(Some(uptime_api))` call (`main.rs:435`).

- [ ] **Step 1:** Add `fn spawn_alerts(cfg: &AlertsConfig, db_path: &str, source: Arc<dyn ConditionSource>) -> Result<photon_api::alerts::AlertsApi, Box<dyn std::error::Error>>` that: opens `SqliteAlertStore::open(db_path)?` (→ `Arc`), makes an `mpsc::channel::<SchedulerCommand>(256)`, builds `Arc::new(WebhookNotifier::new())`, spawns `photon_alerts::scheduler::run(store.clone(), source.clone(), notifier, cmd_rx, cfg.worker_concurrency)`, and returns `AlertsApi { store, cmd_tx, source }`.

- [ ] **Step 2:** In `main`, construct the query engines once. Per Task 6 Step 1's decision: build `EngineConditionSource` (holding engine clones or a shared `Arc`), wrap `Arc::new(...)`, call `spawn_alerts(&cfg.alerts, &cfg.storage.db_path, source)`, then `.with_alerts(Some(alerts_api))` on the `ApiServer` builder. Ensure the engines passed to `ApiServer::new` and to `EngineConditionSource` are the same data (clone if `Clone`, else share the `Arc`).

- [ ] **Step 3:** `cargo build -p photon-server` → compiles. `cargo build --workspace` → compiles.
- [ ] **Step 4: Stage.** `git add crates/photon-server/src/main.rs` — no commit.

---

## Task 9: Uptime → alerts bridge  ·  Wave W5  ·  needs T3, T4, T8

**Files:** Modify `crates/photon-uptime/src/model.rs`, `src/store/{mod,sqlite}.rs` (add `channel_ids`); modify `crates/photon-server/src/main.rs` (bridging notifier).

- [ ] **Step 1: Schema migration.** In `crates/photon-uptime/src/store/sqlite.rs`, after `execute_batch(SCHEMA)`, run an idempotent column-add so existing DBs upgrade:

```rust
// Additive migration: monitors gained `channel_ids` (JSON array) for the alerts bridge.
let _ = conn.execute("ALTER TABLE monitors ADD COLUMN channel_ids TEXT", []);
```

(The `let _ =` swallows the "duplicate column" error on already-migrated DBs — the standard rusqlite additive-migration idiom.)

- [ ] **Step 2:** Add `#[serde(default)] pub channel_ids: Vec<String>` to `Monitor` and `MonitorInput` (`model.rs`), read/write it in the sqlite `row_to_monitor`/`insert_monitor`/`update_monitor` (JSON `TEXT`, `NULL`→`vec![]`). Add a test: create a monitor with `channel_ids=["c1"]`, read it back, assert it round-trips. Run `cargo test -p photon-uptime`.

- [ ] **Step 3: Bridging notifier.** In `photon-server`, define a struct implementing **`photon_uptime::notify::Notifier`** that holds `Arc<dyn AlertStore>` + `Arc<WebhookNotifier>` (alerts). On `notify(ev)`: map the uptime `Transition` → alert `NotifyStatus`; `open_incident`/`close_incident` on the AlertStore (`rule_id = format!("uptime:{}", monitor.id)`, `series_key = ""`); build a payload via `photon_alerts::notify::build_payload`-style JSON (or a small local builder) and `deliver` to each channel in `monitor.channel_ids`. Pass this notifier into `spawn_uptime` (replace the `WebhookNotifier::new` line so uptime uses the bridge; keep resolving the monitor's own `webhook_url` too for backward-compat — deliver to both).

- [ ] **Step 4:** `cargo build --workspace` → compiles; `cargo test -p photon-uptime` → PASS.
- [ ] **Step 5: Stage.** `git add crates/photon-uptime crates/photon-server/src/main.rs` — no commit.

---

## Task 10: End-to-end test + full verification  ·  Wave W6

**Files:** Create `crates/photon-server/tests/alerts_e2e.rs` (model on `crates/photon-server/tests/rum_e2e.rs`).

- [ ] **Step 1:** Write an e2e that boots an `ApiServer` with `with_alerts` wired to a `MemStore` + a **controllable fake `ConditionSource`** (returns a value you set) + a real `WebhookNotifier`, pointed at a `tokio`-spawned local HTTP server that records received bodies. Steps in the test: create a channel (its URL = the local server) → create a rule referencing it (`for_secs: 0`, threshold 0.9) → drive one `process_sample` with value 0.95 → assert the local server received a `"status":"triggered"` body and an incident is open → drive value 0.1 → assert a `"resolved"` body + incident closed.

- [ ] **Step 2:** `cargo test -p photon-server alerts_e2e` → PASS.
- [ ] **Step 3: Full backend gate.** `cargo test` (workspace) → PASS; `cargo clippy --all-targets` → clean; `cargo fmt`.
- [ ] **Step 4: Manual smoke** (per `/run` skill): build frontend (`cd frontend && bun install && bun run build`), `cargo run -p photon-server -- photon.toml`, log in, create a channel + a metrics rule via the UI, use **Test now** in the dialog and **Test** on the channel; confirm the webhook hits a local listener (`nc -l 9999` or a webhook.site URL). Record the observed result.
- [ ] **Step 5: Stage.** `git add crates/photon-server/tests/alerts_e2e.rs` — no commit.

---

## Task 11: Frontend shell — route, nav, tabs, stubs  ·  Wave W0  ·  ‖ T12 and all backend

**Files:** Modify `frontend/src/router/index.js`, `frontend/src/components/common/NavRail.vue`; create `frontend/src/views/AlertsView.vue` + empty stub `.vue` files for all 9 `components/alerts/*` (each just `<template><div/></template>` so imports resolve).

**Reference markup:** `.superpowers/brainstorm/37613-1784341076/content/alerts-final.html` (NavRail group, tab strip, stat band structure).

- [ ] **Step 1:** Add a lazy route `{ path: '/alerts', component: () => import('../views/AlertsView.vue') }` inside the authed section of `router/index.js` (it inherits the existing `beforeEach` guard).
- [ ] **Step 2:** In `NavRail.vue`, add an **Alerts** item to the **Manage** group (Lucide `bell` icon), route `/alerts`. Ensure `AppShell` group-highlight logic includes `/alerts` (grep how `/data` is grouped).
- [ ] **Step 3:** Create `AlertsView.vue`: a `px-5` page with `<h1>Alerts</h1>` + subtitle, the stat band (StatTile-style), and 3 tabs URL-synced via `?tab=rules|incidents|channels` using `lib/core/useUrlState.ts` (copy `DataView.vue`'s `?tab=` pattern). Render `<AlertRulesTable>`, `<IncidentsTable>`, `<ChannelsGrid>` per tab (imported from the stubs).
- [ ] **Step 4:** Create the 9 stub components so `AlertsView` compiles.
- [ ] **Step 5:** `cd frontend && bun run type-check` → clean; `bun run build` → succeeds; visually confirm `/alerts` renders 3 empty tabs.
- [ ] **Step 6: Stage.** `git add frontend/src/router/index.js frontend/src/components/common/NavRail.vue frontend/src/views/AlertsView.vue frontend/src/components/alerts` — no commit.

---

## Task 12: Frontend data layer — queries, api, mocks  ·  Wave W0  ·  ‖ T11 and all backend

**Files:** Create `frontend/src/lib/alertsQueries.ts`; modify `frontend/src/lib/core/api.ts` + the mock module it falls back to. Test: `frontend/src/lib/alertsQueries.test.ts` (vitest).

**Reference:** `frontend/src/lib/uptimeQueries.js` (poll interval, mutation `{ok,error}` contract, toast wiring).

- [ ] **Step 1:** In `api.ts`, add typed client methods for every `/api/alerts/*` route (§10 of the spec). Add mock-fallback fixtures (a couple of rules, channels, incidents) matching the payload shapes, so the UI works offline exactly like the other signals' mocks.
- [ ] **Step 2:** In `alertsQueries.ts`, add `useRules()`, `useChannels()`, `useIncidents(filters)` (poll ~15s like `useMonitors`), `usePreview(condition)` (calls `POST /api/alerts/preview`), and mutations `useCreateRule/useUpdateRule/useDeleteRule/useToggleRule/useTestRule` + channel equivalents + `useTestChannel`. Mutations return `{ ok, error }`, invalidate the relevant query keys, and fire toasts.
- [ ] **Step 3:** Vitest: assert `useRules` builds the expected query key and that a create mutation posts the right body shape (mirror an existing `*Queries` test). `bun run test` → PASS; `bun run type-check` → clean.
- [ ] **Step 4: Stage.** `git add frontend/src/lib/alertsQueries.ts frontend/src/lib/alertsQueries.test.ts frontend/src/lib/core/api.ts <mock module>` — no commit.

---

## Task 13: Rules tab UI  ·  Wave W1  ·  ‖ T14, T15, T16  ·  needs T11, T12

**Files:** Implement `frontend/src/components/alerts/AlertStatBand.vue`, `AlertRulesTable.vue`, `AlertRuleRow.vue`.

**Reference markup:** the Rules table + stat band in `alerts-final.html` (status pill `triggered · N`/`ok`/`pending`/`paused`, signal chip via `lib/signalMeta.ts`, condition summary, `for`, channels, enable toggle).

- [ ] **Step 1:** `AlertStatBand.vue` — 4 StatTiles (Triggered / Active rules / Paused / Channels) from `useRules()` + `useIncidents()` + `useChannels()`.
- [ ] **Step 2:** `AlertRulesTable.vue` — headless TanStack Table over `useRules()`; a `+ New alert` button emitting `open-create`; row → `AlertRuleRow`. `AlertRuleRow.vue` — the status pill (derive from the rule's incident state via `useIncidents`), signal chip, condition summary (build the same one-liner as `Condition::summary`), channels, and the enable toggle wired to `useToggleRule`. Clicking a row emits `edit(rule)`.
- [ ] **Step 3:** Wire `AlertsView` to open `AlertRuleDialog` (from T14) on `open-create`/`edit`.
- [ ] **Step 4:** `bun run type-check` + `bun run build` → clean; visually confirm the Rules tab lists mock rules with correct pills/chips.
- [ ] **Step 5: Stage** the three files — no commit.

---

## Task 14: Create/Edit dialog + condition builder  ·  Wave W1  ·  ‖ T13, T15, T16  ·  needs T11, T12

**Files:** Implement `frontend/src/components/alerts/AlertRuleDialog.vue`, `ConditionBuilder.vue`.

**Reference markup:** the Create dialog in `alerts-final.html` — signal segmented control, the plain-English **sentence builder** whose blanks are dropdowns (per-signal fields), the live **"will trigger on N series now"** preview, channel multi-select, severity.

- [ ] **Step 1:** `ConditionBuilder.vue` — a `v-model:condition`. A signal segmented control (Metrics/Logs/Traces/RUM) swaps the field set; emits a valid `Condition` object matching the spec's per-signal JSON. Field widgets: metric name (autocomplete from `/api/metrics/catalog`), agg/cmp selects, window/threshold/for inputs, group-by chips; logs → a query input (reuse the log-grammar display mirror `lib/core/queryLang.ts`); traces → service + kind; rum → app + kind + route.
- [ ] **Step 2:** Live preview: debounce-call `usePreview(condition)`; render "will trigger on N series now" (green/red) from the response.
- [ ] **Step 3:** `AlertRuleDialog.vue` — Reka UI dialog wrapping `ConditionBuilder` + name/description + channel multi-select (`useChannels`) + severity + `for`. Footer: **Test now** (`useTestRule` on a saved rule, else preview), **Cancel**, **Create/Save** (`useCreateRule`/`useUpdateRule`). Follow `MonitorForm.vue`/`MonitorDetailDialog.vue` for structure.
- [ ] **Step 4:** `bun run type-check` + `bun run build` → clean; visually confirm switching signal swaps fields and preview updates.
- [ ] **Step 5: Stage** the two files — no commit.

---

## Task 15: Incidents tab UI  ·  Wave W1  ·  ‖ T13, T14, T16  ·  needs T11, T12

**Files:** Implement `frontend/src/components/alerts/IncidentsTable.vue`.

**Reference markup:** the **Variant 1** Incidents layout in `alerts-final.html` — one card, **Triggered now** grouped at the top (red pill + subtle left accent), then **Resolved · 24h**; value vs threshold + duration per row; no full-width gradient banner. (The "Silence" action is a spec non-goal — omit it in v1.)

- [ ] **Step 1:** `IncidentsTable.vue` — `useIncidents({status:'triggered'})` and `useIncidents({status:'resolved'})`; render two grouped sections in one card with the shared table styling; StatePill reused for status; show series labels, value/threshold, started, duration.
- [ ] **Step 2:** `bun run type-check` + `bun run build` → clean; visually confirm the two groups render from mock incidents.
- [ ] **Step 3: Stage** the file — no commit.

---

## Task 16: Channels tab UI  ·  Wave W1  ·  ‖ T13, T14, T15  ·  needs T11, T12

**Files:** Implement `frontend/src/components/alerts/ChannelsGrid.vue`, `ChannelCard.vue`, `ChannelDialog.vue`.

**Reference markup:** the Channels cards in `alerts-final.html` (name, masked URL, health, `#rules` using it, Test/Edit, `+ Add channel`).

- [ ] **Step 1:** `ChannelsGrid.vue` — grid over `useChannels()` + `+ Add channel` opening `ChannelDialog`. `ChannelCard.vue` — one channel: masked URL, `Test` (`useTestChannel`), `Edit`. `ChannelDialog.vue` — form (name, url, optional secret, optional headers JSON) → `useCreateChannel`/`useUpdateChannel`.
- [ ] **Step 2:** `bun run type-check` + `bun run build` → clean; visually confirm channel CRUD against mocks + a working Test button.
- [ ] **Step 3: Stage** the three files — no commit.

---

## Task 17: Documentation  ·  Wave W6  ·  ‖ T10

**Files:** Create `docs/subsystems/alerts.md`; modify `docs/architecture.md`, `docs/frontend.md`, `CLAUDE.md`, `docs/subsystems/uptime.md`.

- [ ] **Step 1:** Write `docs/subsystems/alerts.md` (backend engine + `ConditionSource` seam + `AlertStore` + API table + UI), modeled on `uptime.md`.
- [ ] **Step 2:** `architecture.md` — add `photon-alerts` to the crate graph + crate-reference table; add the `/api/alerts/*` routes to the API-surface list.
- [ ] **Step 3:** `frontend.md` — add `/alerts` route, `components/alerts/`, `lib/alertsQueries.ts`.
- [ ] **Step 4:** `CLAUDE.md` — add the crate to the architecture list, the `/api/alerts/*` routes, the `/alerts` frontend route, and the `[alerts]` config.
- [ ] **Step 5:** `uptime.md` — note the shared-channel bridge (`channel_ids` on monitors).
- [ ] **Step 6:** Re-verify internal doc links resolve. **Stage** all docs — no commit.

---

## Self-Review (completed by plan author)

**Spec coverage:** every spec section maps to a task — architecture/crate seam → T1,T6; data model + SQLite → T1,T4; lifecycle state machine → T2; evaluation loop → T5; ConditionSource per-signal → T6; delivery + HMAC → T3; error handling → T3,T5 (non-fatal, state-unchanged-on-error); API surface (incl. `preview`/`test`) → T7; frontend (route/tabs/dialog/incidents/channels + terminology) → T11–T16; uptime bridge → T9; config `[alerts]` → T1,T8; testing → per-task + T10; docs → T17; data-freshness caveat is inherent (T6 reads compacted data) and documented in T17. Non-goals (silencing/grouping/re-notify/Slack) are excluded — the T15 note drops the mockup's "Silence" button.

**Placeholder scan:** no "TBD/TODO/handle appropriately". T6's `sample_*` bodies and the frontend markup intentionally reference exact existing files/line-numbers and the approved mockup rather than reproducing 100s of lines — each such step names the precise source to copy from.

**Type consistency:** shared types are defined once in T1 `model.rs`; `apply`, `process_sample`, `run`, `AlertStore` methods, `AlertsApi` fields, and `EngineConditionSource` all use those names consistently across T2–T8.

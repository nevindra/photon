# Webhook alert & notification engine — design

- **Status:** approved for planning
- **Date:** 2026-07-18
- **Scope:** one implementation plan (a new `photon-alerts` crate + API + UI + server wiring)

## 1. Summary

Photon watches five signals but can only *notify* on one of them: the uptime vertical fires a
webhook when a monitor goes up/down. This design adds a **system-wide alert engine** so a user can
define rules over **metrics, logs, traces, and RUM**, and get a **webhook** when a condition holds —
using the same delivery machinery uptime already proved. Rules and destinations are managed in the UI
(stored in the shared control-plane SQLite DB), not in config.

The uptime up/down case is **folded in** (bridged onto the shared channels + incident history) so
there is *one* notification system, not two.

## 2. Goals & non-goals

**Goals (v1)**
- Per-signal structured alert rules (metrics / logs / traces / RUM), UI-managed, runtime-editable.
- Reusable **notification channels** (generic JSON webhook; optional HMAC signing + custom headers).
- A near-real-time evaluation loop with a `for`-duration (sustained breach) and per-series
  fire/resolve, modeled on the uptime scheduler + pure state machine.
- Incident history (currently-triggered + resolved), mirroring uptime incidents.
- Uptime transitions delivered through the same channels and recorded in the same incident history.

**Non-goals (v1)** — explicitly deferred, each a clean follow-up:
- No Alertmanager-style **grouping / routing / inhibition / silencing**. (The "Silence" action shown
  in the mockup is deferred; pausing a rule via its toggle is the v1 mute.)
- No **re-notification** cadence — one webhook on `triggered`, one on `resolved` (exactly uptime's
  behavior today).
- No **Slack/Discord/PagerDuty-formatted** payloads — one generic JSON webhook shape (a provider can
  consume it via its own inbound-webhook parser).
- No **maintenance windows**, no notification **templating**, no per-user routing.

## 3. Key constraint — data freshness (accepted for v1)

Metrics/logs/traces/RUM are evaluated by the **query engines**, which read **compacted Parquet**.
Data becomes queryable only after its WAL segment closes and compacts (seconds to ~1–2 minutes
depending on cadence). Therefore these alerts are **near-real-time, not instant** — comparable to a
Prometheus scrape+evaluate latency. Mitigations: the default evaluation interval is 60s and condition
windows are sized to cover the lag. **Uptime alerts stay sub-second** because they are evaluated
inline from probe results, not from the query path. This latency is an accepted property of v1; a
future change could peek at the WAL for fresher evaluation.

## 4. Architecture

### 4.1 Crate placement

A new crate, kept pure and I/O-testable exactly like `photon-uptime`:

```
photon-alerts → photon-core        NEW. Domain types + pure state machine + SQLite AlertStore
                                   + webhook Notifier + the scheduler run-loop (generic over seams).
photon-api    → …, photon-alerts   CRUD handlers + /api/alerts routes + embedded UI. (photon-api
                                   already depends on photon-uptime; alerts follows the same edge.)
photon-server → all               Implements ConditionSource over the 3 query engines + uptime store;
                                   spawns the evaluation loop; wires the SQLite AlertStore; passes a
                                   live-reload command sender to ApiServer.
```

`photon-alerts` **must not** depend on `photon-query`. It defines a **`ConditionSource` trait seam**
that `photon-server` implements — the same seam pattern used across the codebase (`Wal`, `Prober`,
`UptimeStore`, `RumSink`, …):

```rust
pub struct SeriesSample { pub key: Vec<(String, String)>, pub value: f64 }

#[async_trait]
pub trait ConditionSource: Send + Sync {
    /// One sample per evaluated series. Empty `group_by` → a single aggregate series (key = []).
    /// An `Ok(vec![])` means "no series crossed / matched" (valid, drives resolves); an `Err`
    /// means "could not evaluate this tick" (state is left unchanged — never a false resolve).
    async fn sample(&self, cond: &Condition, now_ms: i64) -> Result<Vec<SeriesSample>, PhotonError>;
}
```

### 4.2 `photon-alerts` module layout (mirrors `photon-uptime`)

- `model.rs` — `Channel`, `Rule`, `Condition` (per-signal enum), `Incident`, `SeriesState`,
  `AlertPhase`, `Transition`, `RuleInput`/`ChannelInput`, `SchedulerCommand`.
- `state.rs` — the pure `apply()` state machine (§6). No I/O, exhaustively table-tested.
- `source.rs` — the `ConditionSource` trait + `SeriesSample`.
- `notify.rs` — `Notifier` trait + `WebhookNotifier` (generalized from `uptime/notify.rs`) + payload
  builder + HMAC. `FakeNotifier` test double.
- `scheduler.rs` — `run(...)` evaluation loop, generic over `AlertStore` + `ConditionSource` +
  `Notifier`; `process_sample(...)` (the per-rule fold, unit-testable like uptime's `process_result`).
- `store/` — `AlertStore` trait, `SqliteAlertStore` impl, `MemStore` fake.
- `lib.rs` — re-exports.

### 4.3 `PhotonError`

`photon-alerts` needs an error variant. Per the "a variant pre-declared per crate" convention, add a
single new **`PhotonError::Alerts(String)`** variant to `photon-core/src/lib.rs`. This is a
deliberate, one-time core addition made when the crate is introduced (not a parallel-dev edit race).

## 5. Data model (shared control-plane SQLite DB — same DB as uptime monitors & `rum_apps`)

Three tables. Timestamps are Unix ms (matching uptime's `now_ms()`).

### `alert_channels`
| col | type | notes |
|---|---|---|
| `id` | TEXT PK | server-assigned |
| `name` | TEXT UNIQUE | display name |
| `kind` | TEXT | `'webhook'` (only value in v1) |
| `url` | TEXT | destination |
| `secret` | TEXT NULL | if set → `X-Photon-Signature: sha256=<hmac(body)>` |
| `headers` | TEXT NULL | JSON object of extra request headers (e.g. `Authorization`) |
| `created_at`/`updated_at` | INTEGER | |

### `alert_rules`
| col | type | notes |
|---|---|---|
| `id` | TEXT PK | |
| `name` | TEXT | |
| `description` | TEXT NULL | |
| `enabled` | INTEGER | 0/1 |
| `signal` | TEXT | `metrics`\|`logs`\|`traces`\|`rum` |
| `condition` | TEXT | per-signal JSON (below) |
| `for_secs` | INTEGER | sustained-breach duration before `triggered` (0 = immediate) |
| `interval_secs` | INTEGER | evaluation cadence (default 60) |
| `severity` | TEXT | `info`\|`warning`\|`critical` (carried in payload; no routing in v1) |
| `channel_ids` | TEXT | JSON array of `alert_channels.id` |
| `created_at`/`updated_at` | INTEGER | |

### `alert_incidents`
| col | type | notes |
|---|---|---|
| `id` | INTEGER PK | |
| `rule_id` | TEXT | FK-ish (soft) |
| `series_key` | TEXT | canonical serialization of the series labels (`""` for aggregate) |
| `started_at` | INTEGER | |
| `ended_at` | INTEGER NULL | NULL = currently triggered |
| `peak_value` | REAL | worst value observed while triggered |
| `severity` | TEXT | snapshot of the rule's severity at open time |
| `summary` | TEXT | human string, e.g. `avg(system.cpu.utilization)=0.94 > 0.90` |

Per-series runtime state is **rebuilt on startup** from `alert_incidents` (open row ⇒ `Triggered`),
so a restart never re-fires or drops a resolve. The `Pending` sub-state is transient and not
persisted (re-derived on the next breach), exactly as uptime does not persist `consecutive_failures`.

### 5.1 `condition` JSON per signal

Represented in Rust as `#[serde(tag = "signal")] enum Condition { Metrics(..), Logs(..), Traces(..),
Rum(..) }`. `cmp` ∈ `gt|gte|lt|lte`.

**metrics** → `MetricsQueryEngine.query_series` / distribution quantiles:
```json
{ "signal":"metrics", "metric_name":"system.cpu.utilization",
  "label_filters":{"service.name":"api"}, "group_by":["host.name"],
  "agg":"avg", "window_secs":300, "cmp":"gt", "threshold":0.9 }
```
`agg` ∈ `avg|min|max|sum|last|p50|p90|p95|p99|rate|increase`. Each distinct `group_by` combo is its
own series with independent fire/resolve; empty `group_by` → one aggregate series.

**logs** → `QueryEngine.count_matching` (the `query` reuses the log grammar in
`photon-core/src/query/`, parsed server-side):
```json
{ "signal":"logs", "query":"severity:error service.name:payments",
  "group_by":null, "window_secs":600, "cmp":"gt", "threshold":100 }
```
`group_by` optionally `"service.name"` for per-service counts. Threshold is on the **match count**.

**traces** → `SpanQueryEngine.red_metrics` / `latency`:
```json
{ "signal":"traces", "service":"checkout-api", "operation":null,
  "kind":"error_rate", "window_secs":300, "cmp":"gt", "threshold":5.0 }
```
`kind` ∈ `error_rate` (percent) | `latency_p50|p90|p95|p99` (ms) | `request_rate` (rps).

**rum** → `MetricsQueryEngine.rum_vitals` / `QueryEngine.rum_errors`:
```json
{ "signal":"rum", "app_id":"storefront", "route":null,
  "kind":"vital_lcp_p75", "window_secs":900, "cmp":"gt", "threshold":2500 }
```
`kind` ∈ `vital_{lcp|inp|cls|fcp|ttfb}_p75` | `error_count`.

## 6. Lifecycle — pure state machine

Three phases per **(rule, series)**: `Ok → Pending → Triggered` (terminology chosen with the user:
**OK · Pending · Triggered · Resolved**).

```rust
pub enum AlertPhase { Ok, Pending, Triggered }
pub enum Transition { Triggered, Resolved }
pub struct SeriesState { pub phase: AlertPhase, pub since: i64, pub last_value: f64 }

/// Fold one evaluation of one series into its state.
pub fn apply(prev: SeriesState, breaching: bool, value: f64, for_secs: i64, now: i64)
    -> (SeriesState, Option<Transition>);
```

Rules:
- **breaching**
  - `Ok` → if `for_secs == 0`: `Triggered` (emit **Triggered**); else `Pending{since:now}` (no emit).
  - `Pending` → if `now - since >= for_secs`: `Triggered` (emit **Triggered**); else stay `Pending`.
  - `Triggered` → stay `Triggered` (no re-emit; update `last_value`/`peak`).
- **not breaching**
  - `Triggered` → `Ok` (emit **Resolved**).
  - `Pending`/`Ok` → `Ok` (no emit).

A **series absent** from a *successful* sample is treated as not-breaching (so a `Triggered` series
resolves). Since a failed sample leaves state untouched (§7), transient query lag cannot spuriously
resolve. A staleness grace could be added later if flapping appears.

## 7. Evaluation loop (`photon-alerts::scheduler::run`, spawned by `photon-server`)

Mirrors `photon-uptime::scheduler` almost exactly:

- One task owns `HashMap<(RuleId, SeriesKey), SeriesState>` + per-rule `next_due`.
- A ~1s `tick` dispatches due rules' `ConditionSource.sample()` onto a bounded pool (a `Semaphore`,
  `[alerts].worker_concurrency`, default 16). Samples flow back over an mpsc channel so **all state
  mutation stays single-threaded**.
- For each returned series: compute `breaching = cmp(value, threshold)`, fold via `apply()`, then on
  a transition: open/close the `alert_incidents` row and hand an `AlertNotification` to the
  `Notifier` for every `channel_id` on the rule.
- Live-reload: an mpsc `SchedulerCommand::{Upsert(Rule), Remove(RuleId)}` from the API applies
  create/edit/delete without a restart (uptime's exact pattern).
- **Non-fatal:** a panic or `Err` in one rule's evaluation is logged and cannot resolve state or kill
  the loop.

## 8. Delivery (`notify.rs`, generalized from `uptime/notify.rs`)

`WebhookNotifier` POSTs a **stable generic JSON** body per `channel_id`:

```json
{ "status":"triggered",
  "rule":{ "id":"…","name":"web-01 high CPU","severity":"warning","signal":"metrics" },
  "series":{ "host.name":"web-01" },
  "condition":"avg(system.cpu.utilization) > 0.90 for 5m",
  "value":0.94, "threshold":0.90,
  "started_at":1730000000000, "at":1730000300000, "incident_id":123 }
```

- A resolve reuses the shape with `"status":"resolved"`.
- If `channel.secret` is set: header `X-Photon-Signature: sha256=<hex hmac of the raw body>`; merge
  `channel.headers`.
- **Reliability = uptime's proven pattern:** delivery is detached into its own tokio task, ≤3
  attempts with backoff, failures logged, **never blocks the eval loop**. `Notifier::notify` returns
  immediately.

## 9. Error handling & invariants

- **Eval error** → log, skip tick, state unchanged (no false resolve).
- **Delivery error** → retry ≤3 then log & give up; loop unaffected.
- **Dangling `channel_id`** (channel deleted) → skip it, log; other channels still deliver.
- **Startup** → rebuild `Triggered` series from open `alert_incidents` rows.
- Adding the crate must not touch any load-bearing invariant in `docs/architecture.md` — alerts are
  strictly a **read-path consumer** (query engines) plus a new SQLite table group; no WAL/Parquet/
  manifest change.

## 10. API surface (`photon-api/src/alerts.rs`; session-authed; `with_alerts` → 404 if unattached)

```
GET/POST         /api/alerts/rules
GET/PATCH/DELETE /api/alerts/rules/:id
POST             /api/alerts/rules/:id/test     # evaluate this saved rule now, return would-fire series
POST             /api/alerts/preview            # dry-run a draft condition → current series+values (powers the dialog)
GET/POST         /api/alerts/channels
GET/PATCH/DELETE /api/alerts/channels/:id
POST             /api/alerts/channels/:id/test  # send a sample webhook to this channel
GET              /api/alerts/incidents          # ?status=triggered|resolved&rule_id=&limit= — triggered-now + history
```

`photon-api` holds `Arc<dyn AlertStore>` + the `SchedulerCommand` mpsc sender (both attached via
`ApiServer::with_alerts`). CRUD writes go through `AlertStore` and emit an `Upsert`/`Remove` to the
scheduler — the uptime handler pattern.

## 11. Frontend — new `/alerts` route

Follows the uptime/RUM/data playbook; new `.vue` may be `<script setup lang="ts">`.

- **Route:** `/alerts` → `AlertsView.vue`, three URL-synced tabs (`?tab=rules|incidents|channels`)
  like `DataView.vue`. A stat band at top (**Triggered / Active rules / Paused / Channels**).
- **NavRail:** a top-level **Alerts** entry in the **Manage** group (next to Data), cross-signal.
  `AppShell` highlights it by route.
- **Rules tab** — `AlertRulesTable`/`AlertRuleRow`: status pill (`triggered · N` / `ok` / `pending` /
  `paused`), signal chip (reuse `signalMeta.ts` colours), condition summary, `for`, channels, enable
  toggle. `＋ New alert` opens the create dialog.
- **Create/Edit dialog** — `AlertRuleDialog` + `ConditionBuilder`: a **plain-English sentence** whose
  blanks are dropdowns ("Alert when [avg] of [metric] over [5m] is [above] [0.9] for [5m], grouped by
  [host.name]"). Switching the **signal** segmented control swaps the sentence to that signal's
  fields. A live **"will trigger on N series now"** preview is backed by `POST /api/alerts/preview`.
  Then channel multi-select + severity.
- **Incidents tab** — `IncidentsTable`: one card, **Triggered now** grouped at the top (red status
  pill + subtle left accent), then **Resolved · 24h** below — the same table language as Rules (no
  full-width gradient banner). Value vs threshold + duration per row.
- **Channels tab** — `ChannelsGrid`/`ChannelCard`: per-channel card (URL masked, health from last
  delivery, `#rules` using it), `Test`/`Edit`, and `＋ Add channel` (`ChannelDialog`).
- **Queries:** `lib/alertsQueries.ts` — `useRules`/`useChannels`/`useIncidents` (poll ~15s like
  `useMonitors`) + create/update/delete/test mutations, toast-wired; mutations return `{ok, error}`.

## 12. Uptime integration (bridge, low risk)

Uptime already computes `WentDown`/`Recovered`; no re-evaluation is added. v1 bridges those
transitions into the shared path:
- `monitors` gain optional `channel_ids`; on a transition the uptime scheduler emits an
  `AlertNotification` through the shared `Notifier` and writes an `alert_incidents` row (so uptime
  incidents appear in the Alerts → Incidents view alongside data alerts, reusing channels).
- The existing per-monitor `webhook_url` keeps working (deprecate later). Uptime's probe engine and
  SQLite tables are otherwise untouched. Full migration of uptime onto rules is out of scope.

## 13. Config

Optional `[alerts]` section, all tuning (like `[uptime]`): `interval_default` (60s),
`worker_concurrency` (16). No rule/channel config surface — everything is UI/SQLite. Validate in
`Config::validate`.

## 14. Testing

- **State machine** (`state.rs`): exhaustive table tests — immediate vs `for`-duration fire, resolve,
  per-series independence, no re-emit while `Triggered`, no false-resolve on absent-within-error.
- **`process_sample`**: fire/resolve opens/closes incident + notifies once (uptime's
  `process_result` test shape).
- **`ConditionSource`** impl (in `photon-server`): per-signal unit tests over small in-memory engines
  / existing query fixtures, asserting the right engine call + series mapping.
- **Notifier**: `FakeNotifier` records calls; payload schema-stability test; HMAC signature test.
- **`AlertStore`**: `MemStore` fake + `SqliteAlertStore` CRUD/incident/rebuild tests.
- **e2e** (`photon-server/tests/alerts_e2e.rs`, like `rum_e2e.rs`): create channel + rule → drive a
  breach through a fake `ConditionSource` (or seed data) → assert an incident opens and a webhook is
  delivered to a local test HTTP server; then clear the breach → assert resolve.
- **Frontend** (vitest): `alertsQueries` composables + `ConditionBuilder` validation.

## 15. Docs to update (same change)

- New `docs/subsystems/alerts.md` (backend + API + UI, like `uptime.md`).
- `docs/architecture.md`: crate graph, crate-reference table, API-surface list.
- `docs/frontend.md`: `/alerts` route + `components/alerts/` + `alertsQueries.ts`.
- `CLAUDE.md`: crate list, `/api/alerts/*` routes, `/alerts` frontend route, `[alerts]` config.
- `docs/subsystems/uptime.md`: note the shared-channel bridge.

## 16. Open questions (safe defaults chosen; flag at review)

1. **Absent-series resolve** — v1 resolves a `Triggered` series that drops out of a successful
   sample. Alternative: require K empty evals first. Default: resolve immediately.
2. **`Pending` visibility** — surface a `pending` pill in the UI (chosen) vs hide it until
   `Triggered`. Default: show it.
3. **Uptime bridge depth** — v1 keeps uptime's own state machine and only bridges delivery/history.
   Confirm we don't want a deeper unification now.

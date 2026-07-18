# Alerts

A **system-wide webhook alert & notification engine**: rules that watch metrics, logs, traces, and
RUM, and send a webhook when a condition holds. Folds in uptime's existing up/down webhooks so there
is one notification system, not two. A new crate (`photon-alerts`), kept pure and I/O-testable
exactly like `photon-uptime` — no WAL/Parquet/manifest involvement; it is strictly a **read-path
consumer** of the three query engines plus its own SQLite table group.

> Shared conventions: [`../conventions.md`](../conventions.md). Frontend patterns:
> [`../frontend.md`](../frontend.md).

**Terminology (load-bearing):** a rule/series moves through **OK · Pending · Triggered · Resolved**
— never "firing".

## Backend (`photon-alerts`)

Module layout mirrors `photon-uptime`:

- **`model.rs`** — domain types: `Channel`/`ChannelInput` (`kind` always `Webhook` in v1), the
  per-signal `Condition` enum (`Metrics`/`Logs`/`Traces`/`Rum`, `#[serde(tag = "signal")]`) each
  carrying a `Cmp` (`gt`/`gte`/`lt`/`lte`) + `threshold` + `window_secs`, `Rule`/`RuleInput`,
  `Incident`, `SeriesSample` (`key: Vec<(String,String)>` + `value: f64`), `AlertPhase`
  (`Ok`/`Pending`/`Triggered`), `SeriesState`, `Transition` (`Triggered`/`Resolved`),
  `SchedulerCommand` (`Upsert(Box<Rule>)`/`Remove(RuleId)`).
- **`state.rs`** — the pure `apply(prev: SeriesState, breaching: bool, value: f64, for_secs: i64, now: i64) -> (SeriesState, Option<Transition>)`
  fold: one evaluation tick of one **(rule, series)**. `Ok`→(`for_secs==0`? `Triggered` : `Pending`),
  `Pending`→`Triggered` once `now - since >= for_secs * 1000`, `Triggered` stays `Triggered` (no
  re-emit, tracks `last_value`), not-breaching always collapses to `Ok` (emitting `Resolved` only
  from `Triggered`). No I/O; exhaustively table-tested.
- **`source.rs`** — the `ConditionSource` trait seam (see below).
- **`notify.rs`** — `Notifier` trait + `WebhookNotifier` (generalized from `uptime/notify.rs`) +
  `build_payload`/`sign` (HMAC-SHA256) + `FakeNotifier` test double.
- **`scheduler.rs`** — `run(store, source, notifier, cmd_rx, concurrency)`: one task owns
  `HashMap<(RuleId, SeriesKey), SeriesState>` + per-rule `next_due`; due rules' `ConditionSource::sample`
  calls fan out onto a bounded pool (a `Semaphore` sized by `[alerts].worker_concurrency`), results
  flow back over an mpsc channel so all state mutation stays single-threaded. `process_sample` folds
  one rule's returned series through `state::apply`, and on a transition opens/closes an
  `alert_incidents` row and hands an `AlertNotification` to the `Notifier` for every `channel_id` on
  the rule. `SchedulerCommand::{Upsert,Remove}` (sent by the API on rule CRUD) applies create/edit/
  delete live, no restart needed. **Non-fatal:** a panic or `Err` in one rule's evaluation is logged
  and never resolves state or kills the loop.
- **`store/`** — the `AlertStore` trait (`list/get/create/update/delete_rule`,
  `set_rule_enabled`, the same shape for channels, plus `open_incident`/`bump_incident_peak`/
  `close_incident`/`open_incident_for`/`list_open_incidents`/`list_incidents`/`prune_incidents`),
  `SqliteAlertStore` (real impl), `mem::MemStore` (test fake).

### The `ConditionSource` seam

`photon-alerts` must not depend on `photon-query` (kept pure like `photon-uptime`), so it defines a
trait `photon-server` implements:

```rust
pub struct SeriesSample { pub key: Vec<(String, String)>, pub value: f64 }

#[async_trait]
pub trait ConditionSource: Send + Sync + 'static {
    async fn sample(&self, cond: &Condition, now_ms: i64) -> Result<Vec<SeriesSample>, PhotonError>;
}
```

`Ok(vec![])` means "nothing matched/crossed" — a valid result that drives resolves. `Err` means
"could not evaluate this tick" — state is left unchanged (never a false resolve).

**`EngineConditionSource`** (`crates/photon-server/src/alerts_source.rs`) is the real implementation,
built from clones of the same three `Clone`-able (Arc-backed) engines handed to `ApiServer::new`.
Per-signal mapping:

| Signal | Engine call | Notes |
|---|---|---|
| **metrics** | `MetricsQueryEngine::query_series` (1 bucket over the window) | `agg` maps to `photon_core::metric_agg::Agg`; **`p95` is an explicit `PhotonError`** — the engine only reassembles p50/p90/p99. `label_filters` compiles to a `MetricResolvedQuery` via `MetricFieldResolver`; `group_by` → one series per distinct combo. |
| **logs** | `QueryEngine::count_matching` | `query` is parsed with the same log grammar (`photon_core::query::parse` + `FieldResolver`) used by `/api/search`. Ungrouped → one aggregate series (emitted even at 0, so both `>` and `<` thresholds can fire/resolve). `group_by` supports only `"service.name"`/`"service"` (one count per distinct service; only services with matches are emitted). |
| **traces** | `SpanQueryEngine::red_metrics` (per-service, or per-service+operation when `operation` is pinned) | One RED row carries `count`/`error_count`/p50/p90/p99 at once. `error_rate` and `request_rate` are derived; `latency_p50/p90/p99` read the t-digest columns directly. **`latency_p95` is an explicit `PhotonError`** (RED exposes only p50/p90/p99). No matching row ⇒ `Ok(vec![])` (resolve). |
| **rum** | `MetricsQueryEngine::rum_vitals` (vitals) / `QueryEngine::rum_errors` (errors) | Web-Vitals kinds take the requested vital's p75; `error_count` sums `rum_errors` occurrence counts (route-scoped when `route` is set, capped at 10,000 fingerprints). `service` = `app_id`. |

### Data model (shared control-plane SQLite DB)

Same DB as UI users, uptime monitors, and `rum_apps` (`[storage].db_path`). Three tables
(`crates/photon-alerts/src/store/sqlite.rs`), Unix-ms timestamps:

- **`alert_channels`** — `id`, `name` (unique), `kind` (`'webhook'` only), `url`, `secret` (nullable
  — if set, deliveries carry `X-Photon-Signature: sha256=<hmac(body)>`), `headers` (nullable JSON
  object merged into the request), `created_at`/`updated_at`.
- **`alert_rules`** — `id`, `name`, `description` (nullable), `enabled`, `signal`
  (`metrics`|`logs`|`traces`|`rum`), `condition` (per-signal JSON, tagged by `signal`), `for_secs`
  (sustained-breach duration before `Triggered`; `0` = immediate), `interval_secs` (eval cadence,
  default 60), `severity` (`info`|`warning`|`critical` — carried in the payload, no routing in v1),
  `channel_ids` (JSON array of `alert_channels.id`), `created_at`/`updated_at`.
- **`alert_incidents`** — `id`, `rule_id` (soft FK — also used for the uptime bridge's synthetic
  `uptime:<monitor.id>` ids), `series_key` (canonical `k=v,k=v` serialization, `""` for an aggregate
  rule), `started_at`, `ended_at` (`NULL` = currently `Triggered`), `peak_value`, `severity`
  (snapshot at open time), `summary` (human string, e.g. `Avg(system.cpu.utilization) > 0.9`).

Per-series runtime state is **rebuilt on startup** from open `alert_incidents` rows (⇒ `Triggered`),
so a restart never re-fires or drops a resolve. `Pending` is transient and not persisted — re-derived
on the next breach, exactly as uptime doesn't persist `consecutive_failures`.

### Config (`[alerts]`, optional tuning)

`interval_default` (default `"60s"`, default per-rule evaluation cadence) and
`worker_concurrency` (default `16`, max concurrent rule evaluations in flight). The subsystem is
**always on**; omitting `[alerts]` just accepts these defaults. No rule/channel config surface —
everything is UI/SQLite-managed, wired via `photon-server`'s `spawn_alerts`.

### Data-freshness caveat (accepted for v1)

Metrics/logs/traces/RUM conditions are evaluated by the **query engines**, which read **compacted
Parquet** — data becomes queryable only after its WAL segment closes and compacts (seconds to
~1–2 minutes depending on cadence). These alerts are therefore **near-real-time, not instant**,
comparable to a Prometheus scrape+evaluate latency; the default 60s interval and condition
`window_secs` are sized to cover the lag. **Uptime alerts stay sub-second** — they're evaluated
inline from probe results, not through the query path (see the bridge below).

## API

Attached via `ApiServer::with_alerts`; routes 404 unless attached. Handler: `crates/photon-api/src/alerts.rs`
(session-authed, like every other `/api/*` route). `AlertsApi` holds `Arc<dyn AlertStore>` + the
scheduler's `SchedulerCommand` mpsc sender + the `Arc<dyn ConditionSource>`; every rule create/update/
delete also sends an `Upsert`/`Remove` to the running scheduler so the live evaluation loop reflects
the change without a restart — the same pattern `uptime.rs` uses.

| Route | Purpose |
|---|---|
| `GET/POST /api/alerts/rules` | list / create rules |
| `GET/PATCH/DELETE /api/alerts/rules/:id` | read / partial-update (`RulePatch`, all fields optional — also how the rules-table enable/pause toggle sends just `{"enabled":false}`) / delete |
| `POST /api/alerts/rules/:id/test` | evaluate this saved rule's condition right now; returns the series that would (not) breach |
| `POST /api/alerts/preview` | dry-run a draft `Condition` body (not yet saved) → current series+values — powers the create/edit dialog's live preview |
| `GET/POST /api/alerts/channels` | list / create channels |
| `GET/PATCH/DELETE /api/alerts/channels/:id` | read / partial-update (`ChannelPatch`) / delete |
| `POST /api/alerts/channels/:id/test` | send a sample webhook (`status: "triggered"`, rule id `"test"`) to this channel |
| `GET /api/alerts/incidents` | `?status=triggered\|resolved&rule_id=&limit=` — currently-triggered + resolved history (used for both the Incidents tab and each rule row's live status pill) |

`POST /api/alerts/rules/:id/test` and `POST /api/alerts/preview` both return the same
`{ series: [{ series_key, value, breaching }] }` shape (`PreviewResult`/`PreviewSeries` in
`alerts.rs`) — one samples a saved rule's condition, the other an arbitrary draft condition.

## Delivery

`WebhookNotifier` (`notify.rs`) POSTs a stable generic JSON body per `channel_id`:

```json
{ "status": "triggered",
  "rule": { "id": "…", "name": "web-01 high CPU", "severity": "warning", "signal": "metrics" },
  "series": { "host.name": "web-01" },
  "condition": "Avg(system.cpu.utilization) > 0.9",
  "value": 0.94, "threshold": 0.9,
  "started_at": 1730000000000, "at": 1730000300000, "incident_id": 123 }
```

A resolve reuses the shape with `"status":"resolved"`. If `channel.secret` is set, delivery adds
`X-Photon-Signature: sha256=<hex hmac of the raw body>` and merges `channel.headers`. Delivery is
detached into its own task, retried a few times with backoff, failures logged — **never blocks the
eval loop**; `Notifier::deliver` returns immediately (mirrors `photon-uptime`'s proven notify path).
A dangling `channel_id` (its channel was deleted) is skipped and logged; other channels on the rule
still deliver.

## Uptime bridge

Uptime keeps its own Up/Down state machine and its own per-monitor/global `webhook_url` delivery —
no re-evaluation is added. v1 bridges those transitions onto the shared alerts path so uptime
incidents show up in the same Incidents view and can reuse notification channels; see
[`uptime.md`](uptime.md#backend-photon-uptime) for the full mechanics
(`UptimeAlertBridge`, synthetic `rule_id = "uptime:<monitor.id>"`, `channel_ids` on monitors).

## UI

`/alerts` → `AlertsView.vue` (Manage group in `NavRail`, next to Data): an `AlertStatBand` (Triggered
/ Active rules / Paused / Channels), then three URL-synced tabs (`?tab=rules|incidents|channels`,
same pattern as `DataView.vue`).

**Components** (`frontend/src/components/alerts/`):
- **Rules tab** — `AlertRulesTable`/`AlertRuleRow`: a status pill (`triggered · N` when the rule has
  open incidents, `ok`, or `paused` when disabled — derived per-row from
  `useIncidents({status:'triggered', rule_id})`, not a separate `pending` pill in v1), a signal chip
  (`signalMeta.ts` colors), the condition summary (mirrors `Condition::summary()`), `for` duration,
  channel names, and the enable/pause `Switch` (`PATCH .../rules/:id {enabled}` — the enable toggle
  **is** the v1 mute, there's no separate pause route). "+ New alert" opens `AlertRuleDialog`.
- **`AlertRuleDialog` + `ConditionBuilder`** — the create/edit dialog: a plain-English sentence whose
  blanks are dropdowns, switching field sets per signal via a `Segmented` control; a live "would
  trigger on N series now" preview backed by `POST /api/alerts/preview` (debounced via `usePreview`);
  channel multi-select + severity; `for_secs` lives on the dialog (not `ConditionBuilder`, since it's
  a `Rule` column, not part of the per-signal `condition` JSON).
- **Incidents tab** — `IncidentsTable`: one card, **Triggered now** (longest-running first) grouped
  above **Resolved · 24h** (most-recent first, windowed client-side — the API has no time filter),
  value/threshold reconstructed by joining each incident's `rule_id` against `useRules()`.
- **Channels tab** — `ChannelsGrid`/`ChannelCard`: per-channel card (masked URL, `#rules` using it,
  a session-local "health" derived from that card's own `Test` click — there's no persisted delivery
  log yet), `Test`/`Edit`, and "+ Add channel" (`ChannelDialog`).

**Queries** (`frontend/src/lib/alertsQueries.ts`): `useRules`/`useChannels`/`useIncidents` (poll
~15s, like `useMonitors`) + `usePreview` (the dialog's live preview, `enabled` gated on a non-null
condition) + create/update/delete/toggle/test mutations for rules and channels — every mutation's
`api.*` call already returns the non-throwing `{ ok, error }` shape, so `onSuccess` both invalidates
the relevant query key and toasts, branching on the result.

### Templates / quick setup

A **"Browse templates"** button on the Rules tab (and the empty state's "No rules yet — start from a
template" link) opens `TemplatePickerDialog.vue`: a **target-first** flow — pick a target type
(`Segmented`: Service · RUM app · Host · Global), then, for Service/App/Host, a concrete instance
from live data (`useServices`/`useRumApps`/the infra hosts query) — and the matching templates render
as `TemplateRow.vue` rows (plain-English condition + severity pill). Per row: **Apply** builds a
`RuleInput` from the template with the target substituted in and POSTs it straight through the
existing `POST /api/alerts/rules`; **Customize** instead opens the existing `AlertRuleDialog`
pre-seeded from the same draft (its new `:seed` prop, honored only in create mode). A shared "Notify"
channel multiselect on the picker feeds Apply (default none; a hint explains an empty rule is fine if
no channels exist yet).

This is **entirely frontend-only** — a static, read-only catalog of **23 templates**
(`frontend/src/lib/alertTemplates.ts`: 7 Service, 6 RUM app, 6 Host, 4 Global), each a typed constant
with a `build(target)` function that performs the substitution. No backend, no new API, no new
storage — every applied template is just seed data flowing through the create path that already
exists. Target substitution: **Service** → traces `service` + logs `service.name:<svc>` prepended to
`query`; **App** → rum `app_id`; **Host** → metrics `label_filters: { 'host.name': <host> }`;
**Global** → metrics `group_by: ['host.name']` (one series per host, no filter). Host templates work
unmodified because `alerts_source.rs::sample_metrics` already compiles `label_filters` into the query
filter (see the `ConditionSource` table above) — no backend change was needed.

`ConditionBuilder` now round-trips metric `label_filters`: seeded from the condition, re-emitted in
`builtCondition`, and rendered as removable key=value chips next to the existing group-by chips — so
Customizing a Host template keeps its `host.name` scope instead of silently losing it.

**Non-goal (explicit):** provider-native channel formatting (Slack/Discord/Teams/PagerDuty) — a
booked follow-up. Channels remain the single generic `Webhook` kind either way.

## Known limitations (v1)

- **Near-real-time, not instant** for metrics/logs/traces/RUM (see the data-freshness caveat above);
  uptime stays sub-second since it bypasses the query path entirely.
- **`p95` is unsupported** by both engines this seam calls: `MetricAgg::P95` and
  `TraceKind::LatencyP95` return an explicit `PhotonError` rather than silently approximating (the
  metrics/RED engines only reassemble p50/p90/p99).
- **No silencing, grouping, inhibition, or routing** (Alertmanager-style) — pausing a rule's enable
  toggle is the v1 mute.
- **No re-notification cadence** — exactly one webhook on `Triggered`, one on `Resolved`, per
  incident (mirrors uptime's existing behavior).
- **No maintenance windows, templating, or per-user routing.**
- **Log `group_by`** supports only `service.name`; other conditions are ungrouped (one aggregate
  series).
- **SSRF residual risk.** Channel webhook URLs are user-supplied and the POST is made server-side, so
  any UI user can make the server deliver to internal/link-local hosts (`169.254.169.254`, `10.0.0.0/8`,
  etc.). Accepted for v1 (single-tenant, self-hosted — the operator already controls the box); revisit
  with an egress allowlist / metadata-endpoint block once RBAC / multi-tenancy lands.
- **Data-absence / dead-man's-switch.** Metrics/RUM conditions evaluate over the series the query
  returns, so *total* data absence yields **no series** and therefore **no breach**. A `lt`/`lte`
  "heartbeat" alert (e.g. request rate `< 1`) will **not** fire when the data stops entirely — only
  when data is present *and* below the threshold. Detecting a signal going completely silent needs a
  separate liveness check (not modeled in v1).

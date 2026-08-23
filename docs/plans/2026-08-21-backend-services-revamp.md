# Backend (Services / APM) Revamp — Implementation Plan

> **For agentic workers:** implement this plan task-by-task, one fresh subagent per task. Steps use
> checkbox (`- [ ]`) syntax for tracking. **Do not commit between tasks** — leave the working tree
> dirty; the human controls commit granularity.

**Goal:** Turn the Backend world from a list of readings into a board of judgments. `/services` becomes a
cross-Site fleet board that answers "what is wrong, where, and is it the code or that Site?"; the service
detail splits into sub-routes that answer "which endpoint, which error, which dependency, which deploy?".
Along the way, three numbers that are currently wrong get fixed.

**Architecture:** No new storage engine. A background **rollup** task in `photon-server` summarises spans
into `photon.service.*` metrics on the existing metrics pipeline (ADR-0002), and every Backend read path
prefers the rollup with a raw-span fallback. `deployment.environment` joins `service.name`/`host.name` as a
promoted attribute and becomes the third global UI context dimension. RED is recomputed over **entry spans**
(ADR-0003). Backend Issues reuse `photon_core::rum::fingerprint` at ingest. Topology comes from one
query-time self-join serving both the focused map and the fleet graph.

**Tech Stack:** Rust (axum, tonic, rusqlite, DataFusion 43 / arrow 53), Vue 3 + TanStack Query,
TanStack Charts (new, for new chart shapes only), uPlot (retained for brush charts), SQLite control-plane DB.

**Design reference:** the approved visual mockup covers all seven screens (index, overview, operations,
one operation, dependencies, errors, deployments).

---

## Global Constraints

- **Never bump co-pinned deps** (`arrow 53` / `datafusion 43` / `parquet 53` / `object_store 0.11`;
  `opentelemetry-proto 0.27` / `tonic 0.12` / `prost 0.13`). Enabling a *feature* is fine.
- **Ingest ack = local WAL fsync.** Nothing in this plan may block or fail local ingest. The rollup task,
  fingerprinting, and version detection are all off the ack path or O(1) on it.
- **`PhotonError`:** use existing variants; never edit the enum.
- **Control-plane SQLite has no migration framework:** new `CREATE TABLE IF NOT EXISTS` only, never `ALTER`
  an existing table.
- **Conservative pruning stays conservative.** A missing `.idx` or an unknown range means *keep the file*.
- **Two-pass late materialization in `search` stays.** Do not collapse it.
- **`col_ref(name)`** for dotted column names; `col("service.name")` is wrong.
- **Frontend:** bun (never npm), no Pinia, new `.vue`/`.ts` files may be `<script setup lang="ts">`, gated by
  `bun run type-check`. Time bounds are ns strings, chart timestamps ms Numbers.
- **Rollup is derived data and must never be the only copy.** Every endpoint that reads it keeps a raw-span
  fallback path, and the fallback is tested.
- **Docs in the same change.** Update the matching `docs/subsystems/*.md` + `CLAUDE.md` per task; a
  `PostToolUse` hook reminds after source edits.
- **Metric namespace introduced by this plan:** `photon.service.*`. Attributes: `service`, `operation`,
  `environment` (plus `tenant`, stamped by central ingest as today).

## What this plan deliberately does NOT build

Recorded so nobody re-adds them by accident:

- **No Service Registry.** No team, on-call, repo, runbook, description, or tier. `service_settings` keeps
  its single `apdex_threshold_ms` column and gains only SLO definitions. Everything on a Backend page comes
  from telemetry, so nothing can go stale without someone noticing.
- **No Infrastructure tab.** Host and runtime detail stay in `/infra`. The service pages carry one
  `Hosts ↗` action and no infra numbers: one machine runs many services, so a host-level CPU figure on a
  service page is an accusation the data cannot support.
- **No dependency matrix.** The focused one-hop map and the fleet graph cover it.
- **No SLO budget or data-usage column on the index.** Both mix a 30-day number into a table whose every
  other column follows the active Window. Budget lives on the service Overview; usage lives on `/data`.
- **No baseline or anomaly model.** Previous-window comparison only, per `CONTEXT.md`.
- **No live tail of traces, no continuous profiling, no uptime-monitor↔service linking.**

**Open item, not scheduled:** `/infra/:host` could gain "services running on this machine" with per-service
RED. That is the direction the data actually supports (a host has many services) and has none of the
attribution problem above. Out of scope here; raise separately.

---

# Phase 0 — Fix what is wrong before building on it

Three numbers on the current pages are misleading. Everything downstream inherits them, so they go first.

### Task 1: Entry spans define a service's request rate `[done]`

Implements **ADR-0003**. Today `red_over` (`crates/photon-query/src/red.rs:116`) aggregates over *every* span
kind: a service that emits many INTERNAL spans looks several times busier than one that does not, and a
failing CLIENT span (a database call that was retried successfully) counts as a service error.

**Files:**
- Modify: `crates/photon-query/src/red.rs` (`RedRow`, `red_over`)
- Modify: `crates/photon-query/src/red_timeseries.rs` (`RedBucket`, `red_over_time`)
- Modify: `crates/photon-api/src/red.rs` (`red_row_to_json`), `crates/photon-api/src/services.rs`
- Modify: `docs/subsystems/services-apm.md`, `CLAUDE.md`

**Interfaces:**
- `RedRow` gains `span_count: u64` (all kinds) and `basis: RedBasis`; `count`/`error_count`/percentiles/
  Apdex bands are computed over the **entry** set only.
- `pub enum RedBasis { Entry, TraceRoot, AllSpans }` — which rule produced `count`, so the UI can be honest.
- `RedBucket` gains the same `span_count` + `basis`.
- JSON gains `span_count` and `basis` (`"entry" | "trace_root" | "all_spans"`).

**Entry-set cascade** (per group, evaluated in order, first non-empty wins):
1. `kind IN (2 /* SERVER */, 5 /* CONSUMER */)`
2. `parent_span_id IS NULL OR parent_span_id = ''` (trace roots)
3. all spans — and `basis = AllSpans`, which the UI renders as a `span-based` badge

Evaluate the cascade in **one** aggregate pass: project three boolean flags per span
(`is_entry`, `is_root`, `1`) and compute all three candidate aggregates side by side, then pick per group in
Rust. Do not issue three queries.

- [x] **Step 1: Write failing tests** in `red.rs`'s `#[cfg(test)]`:
  - `entry_spans_only_count_toward_rate` — a service with 1 SERVER + 9 INTERNAL spans reports `count == 1`,
    `span_count == 10`, `basis == Entry`.
  - `client_span_errors_do_not_error_the_service` — SERVER span OK + CLIENT span `status_code = 2` gives
    `error_count == 0` while `span_count == 2`.
  - `worker_without_entry_spans_falls_back_to_trace_roots` — only INTERNAL spans, one with no parent →
    `count == 1`, `basis == TraceRoot`.
  - `service_with_neither_falls_back_to_all_spans` — every span INTERNAL with a parent → `count == n`,
    `basis == AllSpans`.
  - `apdex_bands_are_computed_over_the_entry_set` — bands sum to `count`, not `span_count`.
- [x] **Step 2:** Implement in `red_over`, mirror in `red_over_time`.
- [x] **Step 3:** Surface `span_count` + `basis` through both API handlers; `rate` stays `count / window`.
- [x] **Step 4:** `cargo test -p photon-query -p photon-api`, `cargo clippy --all-targets`, `cargo fmt`.
- [x] **Step 5:** Update `docs/subsystems/services-apm.md` + the `CLAUDE.md` architecture paragraph.

### Task 2: Error taxonomy — 4xx and 5xx, not just span status `[done]`

`status_code == 2` is the only error signal today. An SDK that does not set span status hides real 5xx, and a
spike of 401s is invisible.

**Files:** `crates/photon-query/src/red.rs`, `red_timeseries.rs`, `crates/photon-api/src/{red,services}.rs`

**Interfaces:** `RedRow`/`RedBucket` gain `http_4xx: u64`, `http_5xx: u64`, read from the
`http.response.status_code` attribute (fall back to the legacy `http.status_code` key when absent).
`error_count` keeps its current meaning (`status_code == 2`) and is **not** redefined — the new counters sit
beside it so the Overview can chart both without changing what "error rate" means.

- [x] **Step 1: Failing tests** — `counts_4xx_and_5xx_from_http_attribute`,
  `falls_back_to_legacy_http_status_code_key`, `non_http_spans_contribute_to_neither`.
- [x] **Step 2:** Implement (attribute access via `get_field(col_ref(ATTRIBUTES), key)`; the key may also be
  promoted — pick the lookup by schema exactly as `red.rs` already does for `tenant`).
- [x] **Step 3:** Tests, clippy, fmt, docs.

### Task 3: `deployment.environment` becomes a first-class dimension `[done]`

**Files:**
- Modify: `crates/photon-core/src/default.toml`, `photon.example.toml`
- Modify: `crates/photon-core/src/config.rs` (documented default only — no new key)
- Modify: `frontend/src/lib/core/context.ts`, `frontend/src/components/common/ContextBar.vue`
- Modify: `frontend/src/lib/{logs,traces,metrics}/*Fields.ts` (autocomplete catalogs)
- Modify: `docs/architecture.md`, `docs/frontend.md`, `docs/conventions.md`, `CLAUDE.md`

**Interfaces:**
- `promoted_attributes` default becomes `["service.name", "host.name", "deployment.environment"]`.
  Older Parquet files simply lack the column; the query engines already handle either location, and pruning
  degrades to "keep the file", which is the conservative direction.
- `context.ts` exports `environment` (ref), `setEnvironment`, `clearEnvironment`, `envQueryTerm()`
  (`deployment.environment:<x>`), and adds `env` to the URL keys that `useUrlState` merge-preserves.
- `correlate()` carries `env` alongside time and tenant.
- Discovery: `GET /api/facet?field=deployment.environment` (the span facet route already exists).
- **Default selection:** on first load, if the facet contains `prod`, select it. Otherwise select nothing
  (all environments). The picker hides entirely when the facet returns fewer than two values, mirroring how
  the tenant picker hides on an empty registry.

- [x] **Step 1: Failing frontend tests** (`vitest`) — `context.test.ts`: env term appends to `q`; env
  survives a drill-in → back round trip; picker hidden on a single-valued facet; `prod` auto-selected when
  present.
- [x] **Step 2:** Implement `context.ts` + `ContextBar.vue` + field catalogs.
- [x] **Step 3:** Append `envQueryTerm()` in every query composable that already appends `tenantQueryTerm()`.
- [x] **Step 4:** `bun run test && bun run type-check`; update docs incl. the `CLAUDE.md` context paragraph.

### Task 4: One Attention Level for the Backend world `[done]`

`serviceHealth.ts` (`critical`/`degraded`/`healthy`/`idle`) and the Home board's Attention Level
(`Down`/`Critical`/`Warning`/`OK`/`Unknown`) are two severity vocabularies for the same subjects. `idle` is
the load-bearing bug: a service that falls from 5,000 req/s to zero reads as "idle" exactly when it matters.

**Files:**
- Modify: `frontend/src/lib/services/serviceHealth.ts` (becomes the single model)
- Modify: `frontend/src/lib/home/attention.ts` (import instead of re-deriving)
- Modify: `frontend/src/components/services/{HealthPill,HealthBanner,ServiceHealthCounts}.vue`
- Modify: `CONTEXT.md` (see the CONTEXT changes section below)

**Interfaces:**
```ts
export type AttentionLevel = 'down' | 'critical' | 'warning' | 'ok' | 'unknown'
export function serviceLevel(row: ServiceHealthRow, ctx: {
  prev?: ServiceHealthRow      // previous equal-length window — Silence detection
  siteStale?: boolean          // the Site stopped reporting: the numbers are blind, not healthy
  incidents?: AlertIncident[]  // firing incidents already resolved to this subject
}): { level: AttentionLevel; reasons: Reason[] }
```
Rules, in order:
1. `siteStale` → `unknown`, reason `no data since <time>`. Never `ok`, never `idle`.
2. Reported in `prev`, silent now → `down` (Silence).
3. No traffic in either window → `unknown`.
4. A firing incident raises the level to at least the incident's mapped level (reuse `INCIDENT_LEVEL` and
   `resolveIncident` from `attention.ts`).
5. Otherwise error rate **or** Apdex, whichever is worse, using the existing exported thresholds.

- [x] **Step 1: Failing tests** in `serviceHealth.test.ts` — one per rule, plus
  `incident_raises_but_never_lowers_the_level` and `stale_site_beats_every_other_rule`.
- [x] **Step 2:** Implement; delete the `idle` branch and the duplicated logic in `attention.ts`.
- [x] **Step 3:** `bun run test && bun run type-check`; update `docs/frontend.md`.

### Task 4b: Record the blast radius of the entry-span change `[done inline]`

Surfaced while implementing Task 1 and missing from the original plan: `photon-server`'s
`EngineConditionSource::sample_traces` (`crates/photon-server/src/alerts_source.rs`) calls
`red_metrics` too. So **trace alert rules are rescaled by the entry-span correction** — a threshold
written against the old inflated span count now fires differently. Nothing in the alerts code needs
to change (entry counts are what those rules always meant), but the silence would have been a nasty
surprise.

- [x] Note it at the call site in `alerts_source.rs`, including why an operation pinned to a CLIENT
      or INTERNAL span name still resolves (the basis is decided per group).
- [x] Open an `[Unreleased]` section in `CHANGELOG.md` with the upgrade note. Extend it as each
      later phase lands rather than writing it all at the end.

**Standing rule for the rest of this plan:** when a task changes the meaning of a number that already
existed, find every caller before declaring it done, and write the upgrade note in the same change.

---

# Phase 1 — The rollup

Implements **ADR-0002**. This is the load-bearing subsystem: sparklines, a cross-Site index that polls every
15s, 30-day error budgets, and before/after deploy diffs are all unaffordable without it. `red.rs`'s own
header already flags precomputed rollups as the known optimisation target.

### Task 5: `photon.service.*` rollup task `[done]`

**Files:**
- Create: `crates/photon-server/src/rollup/mod.rs`, `crates/photon-server/src/rollup/otlp.rs`
- Modify: `crates/photon-server/src/main.rs` (spawn + supervise, next to the compactor loops)
- Modify: `crates/photon-core/src/config.rs` (optional `[rollup]` block)
- Create: `docs/subsystems/rollup.md`; modify `CLAUDE.md`, `docs/architecture.md`

**Metrics emitted** (all carry `service`, `environment`; operation-scoped ones also carry `operation`):

| Metric | Kind | Unit | Notes |
|---|---|---|---|
| `photon.service.requests` | Sum (delta) | `{requests}` | entry-span count |
| `photon.service.spans` | Sum (delta) | `{spans}` | all kinds, for the honest second number |
| `photon.service.errors` | Sum (delta) | `{errors}` | `status_code == 2` |
| `photon.service.http_4xx` / `.http_5xx` | Sum (delta) | `{responses}` | from Task 2 |
| `photon.service.duration` | Histogram | `ns` | explicit buckets; `metric_dist.rs` already does quantiles. **Shipped as `photon.service.latency`** — the ladder was later extended past 10s, which requires a rename; see [`rollup.md`](../subsystems/rollup.md) |
| `photon.service.apdex.satisfied` / `.tolerating` / `.frustrated` | Sum (delta) | `{requests}` | |
| `photon.service.basis` | Gauge | `1` | `1` when `basis != Entry`, attribute `basis` — lets the UI badge a span-based service without a second query |

**Config** (`[rollup]`, all optional; omitting the block keeps defaults):
```toml
[rollup]
interval_secs   = 60    # how often a pass runs (default 60, min 15)
lag_secs        = 600   # re-roll this much recent time each pass, then seal (default 600)
top_operations  = 50    # per (service, environment); the rest fold into `__other__` (default 50, min 0)
```

**Cardinality rule (non-negotiable):** service level is always rolled up in full. Operation level is capped
at `top_operations` per `(service, environment)` **per pass**, ordered by entry count desc; everything else
is summed into a single `operation = "__other__"` series. `metric_query.rs:31` caps a query at
`MAX_SERIES = 1000`, and an operation name carrying an id (`GET /v1/charges/ch_1a2b3c`) would otherwise take
the metrics store down. A pass that folds anything logs the count — never silent.

**Late data:** each pass recomputes the window `[now - lag_secs, now)` and re-emits it. Because the counters
are **delta** sums keyed by bucket start, re-emitting replaces rather than double-counts only if the reader
de-duplicates by `(series, timestamp)`. Write the emitted bucket timestamps to exact minute boundaries and
document that a span arriving later than `lag_secs` never reaches the rollup. This is the rollup's one
honest inaccuracy and it belongs in `docs/subsystems/rollup.md`, not in a comment.

- [x] **Step 1: Failing tests** (`crates/photon-server/tests/rollup.rs`):
  - `emits_one_series_per_service_and_environment`
  - `operation_tail_folds_into_other_and_logs`
  - `re_rolling_the_lag_window_does_not_double_count`
  - `sealed_buckets_are_not_recomputed`
  - `basis_gauge_is_emitted_only_for_non_entry_services`
- [x] **Step 2:** Implement the pass over `SpanQueryEngine::red_timeseries` (grouped by service, then by
  operation for the top-N), building OTLP protobuf with the helper style already in
  `crates/photon-server/src/federation/otlp.rs`.
- [x] **Step 3:** Write through the local metrics ingest path, exactly as `photon-agent` does.
- [x] **Step 4:** Spawn + supervise in `main.rs`; a panicking pass must be logged and retried next interval,
  never take the process down.
- [x] **Step 5:** `cargo test`, clippy, fmt. Write `docs/subsystems/rollup.md`, link it from `CLAUDE.md` and
  `docs/architecture.md`.

**What actually shipped, and where it left the plan** (all three were forced by the code, not preferred):

1. **A third RED query, not `red_timeseries`.** The plan said to run the pass over
   `SpanQueryEngine::red_timeseries`. That method's own doc says it "only buckets by time, it does not
   group by service", so it would have meant one call per service per bucket. `photon-query/src/red_rollup.rs`
   groups by `(bucket, service, environment[, operation])` in one query instead, and skips the three
   t-digests per candidate in favour of the explicit-bucket latency histogram — percentiles pre-computed
   per bucket cannot be re-aggregated, so a wide-window read off pre-computed p99s would have been wrong.
2. **A watermark, not a re-rolled lag window.** The plan's late-data design was conditional: re-emitting
   "replaces rather than double-counts **only if** the reader de-duplicates by `(series, timestamp)`".
   `metric_query`'s bucket fold is `sum(value)` with no such key, so the conditional was never satisfied
   and a ten-pass lag window would have reported ten times the real traffic. Each bucket is now published
   exactly once, behind a SQLite watermark. Same inaccuracy as the plan documented (late spans are lost),
   different mechanism enforcing it.
3. **`MetricPoint`s written straight to the metrics WAL, not OTLP protobuf over HTTP.** The plan said to
   build protobuf and POST to our own ingest path "exactly as `photon-agent` does". `photon-agent` is a
   separate process and has no choice; in-process that is an encode, a self-authenticated HTTP round trip,
   and a decode back into the same `MetricPoint`s. `RumWalSink` already takes the direct route.

**Two design decisions the plan did not cover**, both caught by tests rather than by reading:

- **The two levels need separate metric families.** With both under `photon.service.requests`, a bare
  query summed the per-operation series onto the per-service one and reported 24 requests for a service
  that served 10. The operation level is now `photon.operation.*`.
- **An operation inherits its service's basis.** Resolving its own cascade made a `CLIENT`-only operation
  (`SELECT users`) fall through to `AllSpans` and report every outbound call as a request served — the
  exact conflation ADR-0003 exists to end. Inheriting also makes operations sum to their service, which is
  what makes `__other__`-by-aggregation and the operation table trustworthy.

**Also shipped beyond the plan's file list:** `[rollup]` env overrides (`PHOTON_ROLLUP_*`) +
`photon.example.toml` + `.env.example` + `deploy/README.md`; `rusqlite` added to `photon-server`;
`pub mod rollup` in `photon-server/src/lib.rs` so `tests/rollup.rs` can reach it.

### Task 6: Backend read paths prefer the rollup `[done]`

**Files:** `crates/photon-api/src/{red,services}.rs`; new `crates/photon-api/src/services_rollup.rs`

**Interfaces:** one internal seam:
```rust
enum RedSource { Rollup, RawSpans }
async fn red_rows(state: &AppState, req: &RedQuery) -> Result<(Vec<RedRow>, RedSource), PhotonError>
```
Prefer the rollup when the window is fully inside rollup coverage **and** the requested bucket width is
`>= 60s`. Otherwise fall back to the span engine. Every response carries `"source": "rollup" | "raw"` so the
UI can say where a number came from, and so a bug is visible instead of silent.

- [x] **Step 1: Failing tests** in `photon-api` — `narrow_window_falls_back_to_raw_spans`,
  `wide_window_uses_rollup`, `rollup_and_raw_agree_within_tolerance` (same fixture through both paths),
  `source_is_reported_in_the_response`.
- [x] **Step 2:** Implement; keep `/api/red` and `/api/services/:service/timeseries` response shapes
  unchanged apart from the additive `source`, `span_count`, `basis` fields.
- [x] **Step 3:** Tests, clippy, fmt, docs.

### Task 7: Summary-mode Sites ship their rollup `[done]`

Today a Site in `summary` mode tells Central almost nothing about its services, which would make the
cross-Site index (Phase 2) work only for Sites that can afford `full` mode. Because the rollup is metrics,
this is a small addition to the existing summary pusher.

**Files:** `crates/photon-server/src/federation/{mod.rs,otlp.rs}`, `crates/photon-core/src/config.rs`,
`docs/subsystems/federation.md`

**Interfaces:** `[federation].push_service_rollup` (bool, default `true` when `[federation]` is present).
The summary pusher additionally forwards the latest sealed `photon.service.*` buckets. Central stamps
`tenant` as it already does for every other metric, so no central-side change is required.

- [x] **Step 1: Failing tests** — `summary_mode_pushes_service_rollup_when_enabled`,
  `disabled_flag_pushes_only_the_federation_metrics`, `full_mode_does_not_double_push`.
- [x] **Step 2:** Implement; **Step 3:** tests, clippy, fmt, `docs/subsystems/federation.md`.

---

**Deviations, again forced by the code:**

- **`source` is per row, not per response.** `/api/red` returns a bare JSON array; wrapping it in
  `{rows, source}` would have broken the frontend. Per-row is genuinely additive and, if anything, more
  honest.
- **A third eligibility condition the plan did not have.** "Window inside coverage + bucket >= 60s" is not
  sufficient: `/api/red` takes a span-grammar query, and `status_code:2`, a body substring or `duration>1s`
  cannot be answered from aggregated metrics at all. Only queries built from the four keys a rollup series
  carries (`service.name`, `deployment.environment`, `operation`, `tenant`) are served.
- **Coverage needs a floor, not just a watermark.** A rollup enabled yesterday has nothing to say about last
  week, and answering that window with zeros reads exactly like an outage. The watermark row gained
  `first_bucket_ts`.
- **Task 7 needed a tee in BOTH modes, not just summary.** The plan assumed `full` mode already covers the
  rollup. It does not: the tee mirrors accepted OTLP batches, and the rollup writes straight to the metrics
  WAL without ever passing through OTLP ingest. `rollup::tee_plan` now decides Share (full) / Create
  (summary) / None — sharing in full mode matters, because a second tee means a second forwarder means every
  bucket delivered twice.

---

# Phase 2 — The index

### Task 8: `GET /api/services/overview` `[done]`

One request feeds the whole index: fleet rows already split per Site, plus per-row buckets for the
sparklines. N per-service calls would be unusable at 15s polling.

**Files:** create `crates/photon-api/src/services_overview.rs`; modify `crates/photon-api/src/lib.rs`
(route), `docs/architecture.md#api-surface`, `docs/subsystems/services-apm.md`

**Interfaces:** `GET /api/services/overview?start&end&buckets&q` →
```jsonc
{
  "source": "rollup",
  "window": { "start": "...", "end": "...", "buckets": 30 },
  "sites": [ { "name": "jkt", "status": "up", "last_seen_ms": 0, "requests": 4200.0,
               "critical": 2, "services": 9, "spark": [[ms, v], ...] } ],
  "rows": [ { "service": "payments-api", "tenant": "jkt", "environment": "prod",
              "count": 0, "span_count": 0, "basis": "entry", "error_count": 0,
              "http_4xx": 0, "http_5xx": 0, "p50": "…", "p90": "…", "p99": "…",
              "satisfied": 0, "tolerating": 0, "frustrated": 0,
              "spark": [[ms, v], ...], "error_spark": [[ms, v], ...] } ]
}
```
`buckets` is clamped by the existing `clamp_buckets` (`crates/photon-api/src/query_params.rs:23`). The `sites`
block merges the tenant registry's liveness (`GET /api/tenants/summary` logic, reused not duplicated) with
per-Site RED aggregates. On a Site install the registry is empty and `sites` is `[]`.

- [x] **Step 1: Failing tests** — `requires_session`, `empty_server_returns_empty_rows`,
  `rows_are_split_per_tenant`, `sites_block_is_empty_without_a_registry`,
  `stale_site_is_marked_and_its_rows_are_not_zeroed`, `clamps_a_dos_sized_bucket_count`.
- [x] **Step 2:** Implement over Task 6's `red_rows` seam.
- [x] **Step 3:** Tests, clippy, fmt, docs.

**Deviation:** `RedRow` gained an `environment` field (populated for `RedGroup::Service` only, exactly like
`tenant`), and `red_over` groups by it. The plan's row shape called for `environment` but neither read path
could produce it — the rollup could have grouped by it, the raw span path could not, so the index would have
changed shape depending on which source answered. Splitting in the engine keeps both identical.

### Task 9: Index — Site band, KPIs, attention cards `[done]`

**Files:**
- Modify: `frontend/src/views/ServicesListView.vue`
- Create: `frontend/src/components/services/{SiteBand,SiteCard,AttentionCard}.vue`
  (replace the existing `AttentionStrip`/`AttentionCard` pair)
- Create: `frontend/src/lib/services/servicesOverview.ts` (query composable + fleet merge)

**Behaviour:**
- Site band: one card per Site plus an `All sites` card, showing liveness dot, mode badge, requests/s,
  critical count, service count, sparkline, and a `last seen` footer. Clicking a card sets the tenant
  context; clicking the active one clears it. A stale Site is tinted and carries a `stale` badge.
- KPI row: Requests, Error rate, p99, Open incidents, Ingest — each with a previous-window delta chip.
- Attention cards: every subject whose level is not `ok`, worst first, capped at 8 with a "+N more" chip
  that scrolls to the table. Each card shows the level pill, name, Site badge, up to three reason lines,
  incident badge, Spread badge, and age.

**Fleet merge (Spread):** rows arrive per `(service, tenant, environment)`. Merge by `(service, environment)`
into one row carrying `spread: { unhealthy: n, total: m }`. Merge only Services — never Hosts or Monitors,
per `CONTEXT.md`.

- [x] **Step 1: Failing tests** — `servicesOverview.test.ts` (merge + spread + stale handling),
  `ServicesListView.test.ts` (band renders and filters; cards appear only for non-OK; cap and overflow chip).
- [x] **Step 2–3:** Implement, then `bun run test && bun run type-check`; update `docs/frontend.md`.

**Deviations:**

1. **The `sites` block carries registry facts only.** It was sending each Site's `services`/`requests`/
   `errors`/`spark` too — a second, separately-computed copy of numbers the fleet merge already walks
   the rows to produce. Two copies inside one response is how a band and the table under it start
   disagreeing. `siteStats()` now derives every number from the merged rows; the backend sends only
   `name`/`status`/`last_seen_ms`/`mode`/`ui_url`, which no row could carry.
2. **The KPI row is Requests · Error rate · p99 · Apdex · Open incidents.** The plan's fifth tile was
   "Ingest", which has no honest source on this page: the per-Site rate is central-only, and
   `/api/usage` is bucketed on a different clock than the Window — exactly the mixed-clock lie the
   route's own doc comment refuses. Spans (ADR-0003's second number) rides as the `sub` line under
   Requests instead, and Apdex takes the fifth tile.
3. **`red_row_to_json` is now shared with `/api/red`.** The overview handler had hand-rolled its own
   row shape and silently dropped `rate`, `error_rate` and `apdex` — all three DERIVED, all three
   read by the table. The frontend's `OverviewRow extends RedRow` says these are one shape, so one
   function now produces them.
4. **`useServicesOverview` keeps previous data.** Without it the Site band vanished the instant you
   clicked a card in it: the band is derived from the same query, so a pending state blanked the very
   control just used.
5. `ServiceHealthCounts.vue` is deleted — the fleet tally is now inline level badges on the table's
   meta row, and nothing else imported it.

### Task 10: Index — the dense table `[done]`

**Files:** rewrite `frontend/src/components/services/ServicesTable.vue`; create
`frontend/src/components/services/{RowSparkline,ApdexBandBar,ServiceRowExpansion}.vue`

**Columns (seven, all on the active Window — no mixed time scales):**
`Health · Service · <window> sparkline · Traffic · Errors · Latency p99 · Apdex`

- Traffic: a relative meter + entry rate, with `<span_count> spans` as a sub-line.
- Errors: an error-rate sparkline + percentage + previous-window trend chip.
- Latency: p99 with `p50 / p90` as a sub-line.
- Apdex: the satisfied/tolerating/frustrated band bar + score. The band data is already returned by
  `/api/red` today and has simply never been displayed on the index.
- Badges on the service cell: Site, Spread (`1 of 3`), incident count, `span-based`, `silent Nm`.

**Row expansion:** clicking a row reveals one child row per Site (same columns, dimmed) plus a three-panel
mini-dashboard: top operations, top issue, worst dependency. This is what replaces a separate compare mode.

- [x] **Step 1: Failing tests** — sort order is worst-first by level then error rate; expansion toggles
  children; a stale Site's child row renders `—` and `Unknown`, never `0/s` and `OK`; `span-based` badge
  appears iff `basis != "entry"`.
- [x] **Step 2–3:** Implement, test, type-check, docs.

**Deviations:**

1. **The row expands; only the NAME navigates.** The plan said "clicking a row reveals…" without
   saying what happened to navigation. Keeping both on the row means you can never actually compare
   two rows, which is the whole point of the expansion.
2. **The third panel is "slowest operations", not "top issue".** Backend Issues do not exist until
   Task 15, and a panel that renders "coming soon" is worse than one that answers a real question.
   Task 15 swaps it in.
3. **Errors carry an HTTP-class sub-line** (`N 5xx · N 4xx`) rather than only the rate. The counts
   have been on `/api/red` since Task 2 with nothing rendering them, and a 4xx is the caller's bad
   request while a 5xx is this service's own fault.

---

# Phase 3 — Detail sub-routes

### Task 11: Route split and sub-nav `[done]`

**Files:** `frontend/src/router/index.js`; create `frontend/src/components/services/ServiceSubNav.vue`;
modify `frontend/src/views/ServiceDetailView.vue`

Routes: `/services/:service` (Overview), `/services/:service/operations`,
`/services/:service/operations/:operation`, `/services/:service/errors`,
`/services/:service/errors/:fingerprint`, `/services/:service/dependencies`,
`/services/:service/deployments`, plus `/services/map` and `/services/errors` off the index.

Mirror `/rum`'s app-scoped sub-route pattern. Each sub-route owns its own queries so Overview stays fast
when Issues is expensive. `useBackTo` must return to the list's own history entry. Use the **nav-tabs**
primitive (route sub-nav), not Segmented and not Reka Tabs.

- [x] Steps: failing router tests → implement → `bun run test && bun run type-check` → `docs/frontend.md`.

**Deviations:**

1. **Only the tabs whose content exists are declared.** Overview, Operations (+ one operation) and
   Dependencies ship now. `/errors` and `/errors/:fingerprint` land with Task 15, `/deployments` with
   Task 19, `/services/map` with Task 17. A tab that leads to an empty page is worse than no tab.
2. **`ServiceScaffold.vue` is new** and not in the plan: three sub-views duplicating the AppShell,
   back button, sub-nav, Related menu and Apdex control is how those four end up subtly different on
   the tab you visit least.
3. **`lib/services/useServiceParam.ts` is new.** `route.params` yields an array when a param repeats
   and vue-router has already percent-decoded — two mistakes that are easy to make once per view.

### Task 12: Overview page `[done]`

**Files:** `frontend/src/views/ServiceDetailView.vue`; create
`frontend/src/components/services/{ServiceLogPanel,SloCard}.vue`

Content, in order: health banner (level, "unhealthy in 1 of 3 sites", reason line, `View traces`) →
KPI row (Requests, Error rate, p99, Apdex, Error budget) → four charts (Request volume with entry/total,
Errors by class with 4xx/5xx split, Latency p50/p90/p99, Apdex bands) each carrying deploy markers →
SLO cards (Task 20) → Key operations table + Logs panel side by side.

The **Logs panel** is new and closes a real hole: a service can be on fire in its logs while its spans look
normal. It shows a severity-stacked volume strip plus the five most frequent error messages, from
`/api/histogram` and `/api/facet` scoped to this service. Detail stays in `/logs` via `correlate()`.

ContextBar actions: `Related ▾`, `Hosts ↗` (to `/infra` pre-filtered), `Create alert`.

- [x] Steps: failing tests → implement → test, type-check → `docs/subsystems/services-apm.md`.

**Deviations:**

1. **No Error budget tile and no SLO cards yet** — both are Task 20, which owns the SLO model. The KPI
   row is Requests · Error rate · p99 · Apdex until then.
2. **No deploy markers yet** — Task 18 detects deploys; markers land with Task 19.
3. **`ServiceTimeseriesBucket`'s TS type was missing two thirds of the route's payload** (`count`,
   `span_count`, `basis`, `error_count`, `http_4xx`/`http_5xx`, `apdex`, the Apdex bands). The Overview
   page could only draw rates and percentiles even though everything else was already on the wire.
   Same class of bug as the overview row's missing `rate`/`error_rate`.
4. **A real grammar bug surfaced and was fixed in `photon-core`.** `field:"value"` kept its quotes in
   the resolved value, so every quoted term parsed cleanly and matched **nothing** — and quoting is
   the only way to express a value containing a space, which operation names (`GET /v1/charges`)
   always do. `classify` now strips one surrounding pair and splits OR-lists on commas outside quotes.
   `photon-api`'s `grammar_term` and the frontend's `grammarTerm` are the shared builders; both return
   nothing for a value containing a `"`, since the grammar has no escape and a term built around one
   would silently match nothing.

### Task 13: Operations list and one operation `[done]`

**Files:** create `frontend/src/views/{ServiceOperationsView,ServiceOperationView}.vue`; create
`frontend/src/components/charts/LatencyHeatmap.vue`; modify `crates/photon-api/src/services.rs`

**Operations list** reuses the index's column vocabulary exactly — no new columns — so one reading skill
covers both tables. A segmented control switches `Entry points` (default) / `All operations`. The
`__other__` row is rendered explicitly with a note naming the cap, and a footer states how many operations
`Entry points` is hiding and why (internal/client/producer spans are a service's own work, not requests it
served).

**One operation** adds: a **latency heatmap over time** (the one shape that genuinely needs a chart engine —
three percentile lines cannot show a distribution splitting in two), with the Apdex `T` and `4T` lines drawn
on the same axes so the heatmap and the Apdex score describe the same thing; a response-code table
(502/504/409/422 separated); and a slowest-traces exemplar list naming each trace's slowest child span.

**Backend:** `GET /api/services/:service/operations` and
`GET /api/services/:service/operations/:operation/{timeseries,histogram,exemplars}`.

- [x] Steps: failing tests (Rust + vitest) → implement → full test sweep → docs.

**Deviations:**

1. **Two new backend routes, not four.** `GET /api/services/:service/operations` (rows + per-row
   sparklines + the rollup read path) and `.../operations/:operation/heatmap`. The plan's
   `timeseries` and `exemplars` need no new route: `/api/services/:service/timeseries` already accepts
   a `q` AND-ed with the service term, and `/api/traces/search?sort=slowest` already is the exemplar
   list. Adding routes that wrap existing ones would be two more places for the same query to drift.
2. **The heatmap's latency ladder is the rollup's `LATENCY_BOUNDS_NS`**, not bins derived from the
   matched set. Data-derived bins re-scale the y-axis when one outlier enters the window, so a cell
   would mean something different at 10:00 than at 10:05 — and the columns of a heatmap have to be
   comparable or the chart says nothing. It also means a wide-window heatmap can later be answered
   from `photon.service.duration` (shipped as `photon.service.latency`).
3. **Spans that never closed are reported apart from every band** (`open_spans`). "Under 1ms" and
   "never finished" are opposite facts; folding the second into the first makes a stalling operation
   look like it got faster.
4. **The `Entry points` filter is derived from `basis`, not a new backend flag.** `red_over` already
   resolves the entry-set cascade per operation group, so `basis == "entry"` is exactly "this
   operation has server or consumer spans of its own".
5. **The operations table has no sparkline column yet in the Overview page's embedded copy** — it
   shares `OperationsTable`, and the sparkline arrives with the dedicated route's `spark` field once
   the table renders it. Everything else in the column vocabulary matches the index exactly.

---

# Phase 4 — Backend Issues

### Task 14: Fingerprint span exceptions at ingest `[done]`

`CONTEXT.md` has defined Backend Issue for a while with no implementation. `photon_core::rum::fingerprint`
(`crates/photon-core/src/rum.rs:126`) is public, pure, and already does exactly the required normalisation
(type + digit-masked message + top frame).

**Files:** `crates/photon-ingest/src/` (span mapping), `crates/photon-core/src/rum.rs` (export a
span-oriented wrapper), `docs/subsystems/traces.md`, `docs/subsystems/services-apm.md`

**Interfaces:** during span mapping, when `status_code == 2` **or** an `exception` event is present, parse
the span's `events` JSON, take the first `exception` event's `exception.type` / `exception.message` /
first stack frame, and write three attributes: `error.type`, `error.message`, `error.fingerprint`. When
there is no exception event, fall back to `status_message` as the message and `span.name` as the frame.
Spans with neither get no attributes at all (not an empty fingerprint).

Cost control: only touch the `events` JSON for spans that already look like errors. A healthy service pays
nothing.

- [x] **Step 1: Failing tests** — 13 in `photon-core/src/span_error.rs` + 8 in
  `photon-ingest/src/trace_mapping.rs`, covering all six named cases.
- [x] **Step 2–3:** Implement, test, clippy, fmt, docs.
- [x] **Step 4:** The UI cut-over notice shipped with Task 15 — `IssueBackfillNote.vue`, rendered on
  both Issues lists.

**Deviations:**

1. **It reads the OTLP events, not the stored JSON.** The plan said "parse the span's `events` JSON";
   both mappers already hold the OTLP `Vec<Event>` and serialize it a few lines later, so parsing our
   own output would pay a serialize + parse round trip on every errored span. `malformed_events_json`
   therefore cannot arise on this path, and the equivalent case — a non-string `AnyValue` for a
   message or a stack trace — is tested instead.
2. **The fingerprint is service-free.** `same_exception_in_two_services_is_two_issues` is a query-time
   property of the `(service, fingerprint)` identity, not of the hash. Baking the service into the
   hash would also destroy the useful signal that one bug is present in three services.
3. **`error.type` is `"ERROR"` on the status-only path**, and `"exception"` for an exception event
   that carries no type. Neither is invented: they are the OTLP status text and the OTel event name.
4. **A producer-set `error.*` attribute loses to the derived one** on both mapping paths. A stale
   `error.type` copied onto every span by a middleware would otherwise silently regroup a whole
   service's Issues.

### Task 15: Issues API and UI `[done]`

**Files:** create `crates/photon-query/src/span_issues.rs`, `crates/photon-api/src/service_errors.rs`;
create `frontend/src/views/{ServiceIssuesView,ServiceIssueDetailView}.vue`

**Interfaces:** mirror the RUM errors surface deliberately, so the two worlds read alike:
- `GET /api/services/:service/errors` (+ optional `q` grammar filter)
- `GET /api/services/:service/errors/facets`
- `GET /api/services/:service/errors/:fingerprint`
- `GET /api/services/errors` — the fleet-wide list behind `/services/errors`

Identity is `(service, fingerprint)`, never the fingerprint alone: the same exception in two services is two
Issues with two owners and two fixes.

- [x] Steps: failing tests → implement → tests → `docs/subsystems/services-apm.md` + `CLAUDE.md` API surface.

**Deviations:**

1. **The fleet list has no facet panel.** `/api/services/:service/errors/facets` scopes to one
   service's errored spans; a fleet-wide facet would be a different query with a different cost, and
   faking it by unioning per-service calls would mean N requests to render a sidebar. The search bar
   still accepts every field a facet would offer, and the omission is stated in the view's own docs.
2. **`traces`, not `sessions`.** The RUM list counts distinct sessions ("how many people hit this").
   The backend has no session; the honest analogue is distinct traces, "how many requests went
   wrong". `count` (errored spans) rides beside it, since a retry loop inside one request is real
   work the fault caused. A third column, `operations`, says how many call sites one fault reaches.
3. **The facets route filters on `error.fingerprint:*` before faceting.** Without it the panel
   reports that 99% of traffic is `kind_text:SERVER` — true, and useless.
4. **The sample stack is parsed from the `events` JSON at read time**, for the ≤20 detail rows only.
   Task 14 deliberately did not copy `exception.stacktrace` onto an attribute; a stack trace is
   kilobytes and every column read would pay for it. A status-only Issue has no stack, and the UI
   says that rather than rendering an empty box.
5. **A new `backendIssue` correlate kind**, rather than reusing `service`. An Issue's jumps are its
   failing trace, the logs around it, its failing traces and its service — not the service's generic
   metrics/RUM/uptime menu.
6. **`ServiceScaffold` grew a `#search` slot** so a sub-route can put a search bar beside the
   sub-nav in the ContextBar's one fixed-height region. Stacking them would overflow the bar — the
   same call `RumErrorsView` already made.

---

# Phase 5 — Topology

### Task 16: `GET /api/services/map` `[done]`

One query-time self-join produces every service-to-service edge in the window; the focused map and the fleet
graph are two readings of the same result, not two features.

**Files:** create `crates/photon-query/src/service_map.rs`, `crates/photon-api/src/service_map.rs`

**Interfaces:** join child `(trace_id, parent_span_id)` to parent `(trace_id, span_id)` and group by
`(parent.service.name, child.service.name)`, returning `count`, `error_count`, `p50/p95/p99`. Optional
`service` param narrows to edges touching one service (both directions).

**Cost, stated plainly:** the probe side must scan every span file in the window on a narrow projection
(`trace_id`, `span_id`, `parent_span_id`, `service.name`, `duration_nanos`, `status_code`) because the
parent's service is unknown up front, so the skip index cannot prune it. Cap the scan with a
`MAX_MAP_SPANS` constant in the style of `MAX_DEP_SPANS` (`service_dependencies.rs:67`), log when it bites,
and never truncate silently. If this proves too slow in practice, the escape hatch is to emit edges from the
Phase 1 rollup — but do not build that speculatively.

The existing `dependencies` endpoint stays: a database is not a Service, and `CONTEXT.md` is explicit.

- [x] **Step 1: Failing tests** — 13 in `photon-query/src/service_map.rs` + 6 in
  `photon-api/src/service_map.rs`, including all five named cases.
- [x] **Step 2–3:** Implement, test, clippy, fmt, docs.

**Deviations:**

1. **The self-join runs in Rust, not in DataFusion.** Same operation; this reads the spans once
   instead of twice, and the `(trace_id, span_id)` index borrows straight out of the Arrow buffers,
   so the join side copies nothing. It is also the shape `service_dependencies` already uses.
2. **The join key is the PAIR**, not `span_id`. Per OTLP `span_id` is unique only *within* a trace,
   so keying on it alone cross-wires two traces that reused an id — pinned by
   `the_same_span_id_in_two_traces_does_not_cross_wires`.
3. **The cap is a response field, not only a log line.** `scan_cap_is_logged_not_silent` asked for a
   log; a log the user never sees is still a silent truncation from where they sit, so `capped` and
   `scanned` ride on the JSON and the UI states it on the page.
4. **A service calling itself is not an edge.** Real, but a self-loop on every service answers
   nothing about who depends on whom.
5. **Edge latency is the CALLEE's span duration.** The plan did not say whose; naming it matters,
   because the caller's wall-clock also carries network and queueing. Stated in the docs and under
   the UI's table.
6. **Nodes carry their own totals**, not edge totals, so node size means "how busy" rather than "how
   connected". `roots` marks where work enters the system, for the layout.

### Task 17: Focused map and fleet graph `[done]`

**Files:** create `frontend/src/views/ServiceMapView.vue`,
`frontend/src/components/services/{FocusedDependencyMap,FleetGraph}.vue`; add a graph layout dependency

- **Focused map** (`/services/:service/dependencies`): three columns — callers, this service, callees —
  drawn with hand-authored SVG curves. Always one hop, so it is always readable regardless of fleet size.
  Service, database, and external dependencies share one picture, because to someone debugging the
  distinction does not matter. Two tables sit below it for the numbers.
- **Fleet graph** (`/services/map`): a node-link graph laid out with `dagre` or `elkjs` (pick one, justify
  it in the PR description, keep it out of the initial bundle with a dynamic import). Node dot = Attention
  Level, edge weight = throughput, edge hue = error rate.

- [x] Steps: failing tests → implement → `bun run test && bun run type-check && bun run build`
  → `docs/frontend.md`.

**Layout library: `@dagrejs/dagre`.** A service map is a layered DAG with occasional cycles, which is
the one thing dagre is for. `elkjs` ships roughly 1.5 MB against dagre's 48.43 kB (16.88 kB gzipped)
and its extra algorithms (force, radial, mrtree) buy nothing here. The dynamic `import()` puts it in
its own chunk — verified in the build output, the initial bundle is unchanged.

**Deviations:**

1. **The focused map lives on the Dependencies tab, not on a new route.** "Who calls me / what do I
   call / what do I wait on" is one question; a separate route would have split it across two pages
   that both need the same two requests.
2. **The fleet graph has its own top-level tab** (`Services · Issues · Map`) rather than sitting
   inside the index. It is not a filter of the services table — it groups call edges, not services —
   and a mode toggle would imply it is.
3. **Node dots read `serviceHealth.ts`**, and `/api/services/map` deliberately carries no level. A
   level depends on Apdex and on the previous window; neither is a topology fact, and computing a
   second one server-side is how the same service ends up amber here and red on the index.
4. **A layout failure renders as a message, not as an empty canvas.** An empty graph and a healthy
   fleet with no services look identical, so the component says which one happened.
5. **The focused map caps each column at 8** and says how many rows it did not draw. The tables below
   carry every one.

---

# Phase 6 — Deployments

### Task 18: Detect deploys at ingest, accept them from CI `[done]`

**Files:** create `crates/photon-api/src/deployments.rs`; modify `crates/photon-ingest/src/`,
`crates/photon-api/src/lib.rs`, `docs/subsystems/services-apm.md`

**Interfaces:**
- New table `deployments(id, service, environment, tenant, version, detected_at_ms, source,
  commit_sha, author, message, url)` — `CREATE TABLE IF NOT EXISTS`, never `ALTER`.
- Auto-detection: an in-memory `HashSet<(service, environment, version)>` seeded from the table at boot.
  On a span carrying an unseen `service.version`, insert one row with `source = 'detected'`. O(1) on the hot
  path, one insert per genuinely new version.
- `POST /api/deployments` (session-authed) upserts the CI-supplied fields onto a matching detected row, or
  creates one with `source = 'ci'`.
- `GET /api/deployments?service&environment&start&end`.

- [x] **Step 1: Failing tests** — `first_sighting_of_a_version_creates_one_row`,
  `repeat_sightings_do_not_insert`, `ci_payload_enriches_a_detected_row`,
  `set_is_seeded_from_sqlite_at_boot`, `unknown_version_attribute_is_ignored`.
- [x] **Step 2–3:** Implement, test, clippy, fmt, docs. 28 tests
  (`photon-ingest/src/deployments.rs` 9, `photon-api/src/deployments.rs` 19).

**Deviations:**
- **Reads the RESOURCE, not the span.** The plan said "on a span carrying an unseen
  `service.version`". `service.version` is a resource attribute per semconv, so
  `resource_versions(&req)` reads the resource group — a 5,000-span batch is four string lookups
  instead of 5,000, for the same fact. A span-level `service.version` would be a non-conforming
  producer anyway.
- **The identity is four parts, not three.** The plan's set key was
  `(service, environment, version)`. `tenant` is in it too: central aggregates many Sites, and the
  same build deployed at two Sites is two deployments that happened at different times.
- **`environment`/`tenant` are `''`, never NULL.** SQLite treats NULL as distinct from NULL, so a
  nullable column would let the same unversioned deployment insert forever past the UNIQUE index.
- **`detected_at_ms` and `source` are write-once**, deliberately absent from the CI upsert's SET
  list. A pipeline that posts an hour after the traffic arrived must not drag the deploy marker off
  the moment the graph actually changed, and `source` records who saw it first — which is why an
  enriched row still reads `auto-detected` in the UI.
- **The `DeploymentWatcher` seam is sync.** The in-memory set answers every batch after the first
  with no I/O; making it `async` would put an `.await` on the trace receiver for work that almost
  never happens.

### Task 19: Deployments tab and chart markers `[done]`

**Files:** create `frontend/src/views/ServiceDeploymentsView.vue`,
`frontend/src/components/services/DeploymentTimeline.vue`; modify the chart components to accept markers

A timeline of releases, each with a before/after RED diff (error rate, p99, Apdex, throughput) computed from
the rollup, tinted by whether the release made things better or worse. Auto-detected rows are labelled
`auto-detected`; CI rows show sha, author, and message. Deploy markers render on the latency, volume, and
error charts across the service pages.

- [x] Steps: failing tests → implement → test, type-check → docs. 15 new frontend tests
  (`DeploymentTimeline.test.ts` 11, `deployMarkers.test.ts` 4) + one in `MetricChart.test.js`.

**Deviations:**
- **The before/after diff is computed frontend-side from `/api/services/:service/timeseries`, not
  from the rollup directly.** Two reasons: the row's numbers then provably agree with the chart
  above them (same buckets, same read), and the comparison window is a UI choice, not a storage one
  — it is a `±5/15/30/60m` control on the page.
- **A comparison is refused unless both sides are covered.** Diffing half a window against a full
  one invents a regression, so the row says "too recent to compare" or "the window before it is
  outside the time range" instead. Moves under 5% are left untinted as noise.
- **Direction is per metric.** Error rate up is bad, Apdex up is good, throughput up is neither.
  Throughput is shown as context precisely because it must not be scored.
- **Markers are capped at the 12 newest** (`lib/services/deployMarkers.ts`). Forty releases in a
  window would turn a chart into a picket fence, hiding what the markers exist to reveal; keeping
  the newest rather than an even sample is right because the release you are chasing when you see a
  step in a chart is almost always a recent one.
- **Found and fixed a live charting bug on the way through.** `metrics/MetricChart.vue` takes
  `points[].t` in NANOSECONDS (it divides by 1e6 — the metrics explorer's contract), but
  `ServiceDetailView.vue` and `ServiceOperationView.vue` both handed it epoch **ms**. Their errors,
  latency and volume charts therefore plotted at epoch 1970 and drew an empty window. Fixed at the
  two call sites (`b.ts * MS_TO_NS`) rather than by changing `MetricChart`'s contract, which the
  metrics explorer depends on. Pre-existing on `main`, not introduced by this plan.
- **`markers` was threaded through four components**, not just the two the task named:
  `MetricChart`, `LineChart` (it lacked the prop `BarChart` already had), `ServiceVolumeChart` and
  `ApdexBandChart`, so a release line looks identical on every chart on the service pages.

---

# Phase 7 — Objectives

### Task 20: SLOs, error budgets, burn rate `[done]`

**Files:** modify `crates/photon-api/src/settings.rs` (new table, same store); create
`crates/photon-api/src/slo.rs`; create `frontend/src/components/services/SloCard.vue`

**Interfaces:**
- `service_slos(id, service, environment, name, kind, target, threshold_ms, window_days, operation)` where
  `kind` is `availability | latency`. Several named SLOs per service; each may be scoped to one operation, so
  a critical endpoint can be held to a tighter line than an admin page in the same service.
- `GET/POST /api/services/:service/slos`, `PATCH/DELETE /api/services/:service/slos/:id`,
  `GET /api/services/:service/slos/:id/budget`.
- Budget and **multi-window burn rate** (fast: 1h vs 5m; slow: 6h vs 30m) are computed from the Phase 1
  rollup. "40% of budget left" conveys no urgency; "burning 14.2×, budget gone in 3h 10m" does.
- Because the rollup is metrics, a burn-rate alert is an ordinary metrics rule in `photon-alerts`. Add no new
  rule kind and no new `ConditionSource`.

- [x] **Step 1: Failing tests** — CRUD + validation, `budget_is_computed_from_rollup_counters`,
  `burn_rate_uses_two_windows`, `operation_scoped_slo_ignores_other_operations`,
  `budget_over_a_window_longer_than_rollup_coverage_is_reported_partial`. All four exist, plus
  `a_burst_the_short_window_has_moved_past_does_not_fire_the_fast_pair`,
  `a_latency_slo_counts_requests_under_the_threshold`,
  `a_latency_threshold_between_ladder_edges_reports_what_it_measured` and
  `no_rollup_coverage_at_all_is_an_empty_partial_budget_not_an_error`.
- [x] **Step 2–3:** Implement, test, clippy, fmt, docs. SLO cards appear on the service Overview
  only — never as an index column. 31 backend tests (`photon-api/src/slo.rs` 24 across three test
  modules, `photon-query/src/metric_dist.rs` 7) + 13 frontend (`SloCard.test.ts`).

**Deviations:**
- **A new table, not `settings.rs`.** The plan said "modify `settings.rs` (new table, same store)".
  `service_slos` got its own module and its own store instead: `SettingsStore` is a key/value seam
  with an `all_apdex_thresholds`-shaped API, and grafting five CRUD methods and a budget computation
  onto it would have made one trait mean two unrelated things. Same file pattern, separate seam.
- **`latency_sli` on `MetricsQueryEngine`, not a new `Agg`.** The latency SLI needs "count at or
  below X", which the metrics engine could not answer. It is a dedicated entry point rather than an
  `Agg` variant because `Agg` is a closed vocabulary shared with the metrics explorer and the alert
  engine, and this one carries a parameter none of the others do. `histogram_series`'s accumulation
  was factored out into `histogram_accumulate` and is now shared, so the quantile reader and the SLI
  reader cannot drift apart on the cumulative-reset rule.
- **A latency threshold snaps DOWN to a ladder edge, and the response says which**
  (`measured_threshold_ms`). Not in the plan, but unavoidable: a histogram knows nothing finer than
  its own `LATENCY_BOUNDS_NS` edges. Snapping down keeps it conservative (an objective reported met
  was met), and reporting the edge is what stops someone believing a 300ms line was tested when
  200ms was.
- **Rollup-only, with `partial` rather than a raw-span fallback.** Every other Backend read falls
  back to raw spans when the rollup cannot answer. An SLO deliberately does not: a 30-day raw-span
  scan behind a page load is worse than an honest "measured over the four hours we have".
- **Burn thresholds are constants** (14.4× / 6×, the SRE-workbook numbers), not per-SLO settings —
  a per-objective burn threshold is a knob nobody has the information to turn.
- **Also delivered here: the error-budget KPI/SLO cards deferred from Task 12**, as `SloPanel` →
  `SloBudgetCard` → `SloCard` on the service Overview, between the KPI row and the charts.

---

# Phase 8 — Charts and usage

### Task 21: Adopt TanStack Charts for new shapes `[done]`

**Files:** `frontend/package.json`; create `frontend/src/components/charts/tanstack/*`; modify the new
chart components from Phases 2–7

TanStack Charts is **pre-alpha (0.14.0)** and its Vue adapter is **SVG only** — the canvas entry point exists
for React and Octane, not Vue. That is acceptable here because Photon's charts default to 48 buckets
(`crates/photon-api/src/histogram.rs:23`, capped at `MAX_BUCKETS = 3000`), so point counts are small.

**Boundary — hold it:** TanStack Charts renders the *new* shapes only (latency heatmap, Apdex band chart,
deploy markers, row sparklines). `SpanVolumeHistogram` and `LatencyHistogram` stay on uPlot because their
drag-to-zoom and drag-to-filter-duration interactions have no documented equivalent (the docs list pointer
focus, selection callbacks, and tooltips — no brush, zoom, or pan). Rewriting brush is not on this plan's
critical path. A full migration is a separate decision, taken later, with this phase as the evidence.

- [x] Steps: add the dependency → build the shared theme adapter mapping `tokens.css` to chart
  channels → port the shapes that qualified → `bun run test && bun run type-check && bun run build`
  → record the bundle delta in `docs/frontend.md`. 8 new tests (`StackedBandChart.test.ts`).

**Deviations:**
- **`@tanstack/charts`, not `@tanstack/vue-charts`.** The Vue package at 0.14.0 is a compatibility
  shim whose own README says new applications import the Vue adapter from `@tanstack/charts/vue`.
  Same version, one fewer package.
- **One shape ported, not four — and the other three are a finding, not an omission.** The plan
  named the latency heatmap, the Apdex band chart, deploy markers and row sparklines. Only the
  Apdex band chart qualified. `LatencyHeatmap` is already a plain CSS grid with a written rationale
  for not using a chart engine, `RowSparkline` is a hand-authored 22px SVG path, and deploy markers
  are an annotation on other charts rather than a shape — porting them means porting the uPlot
  charts underneath, which the plan's own boundary excludes. Porting any of the three would have
  been a regression. The full table is in `docs/frontend.md`.
- **The measured delta made lazy loading mandatory.** TanStack Charts is **96.95 kB raw / 33.85 kB
  gzipped** for one chart, against uPlot's **52.35 kB / 23.30 kB** for every other chart in the
  app — roughly 1.5x uPlot's whole footprint. It is loaded through a dynamic `import()`
  (`components/charts/tanstack/lazy.ts`), like the fleet graph's dagre, so the initial bundle moved
  only 327.98 kB → 328.95 kB gzipped.
- **The lazy wrapper is its own module** so a consumer's test can stub one named export, rather than
  stubbing a module Vue's async-component resolution probes for internal flags (`__isTeleport`).
- **The engine paints from `currentColor` by default**, so `chartTheme.ts` resolves `tokens.css`
  into concrete `hsl()` values and passes them as the spec's `theme` — the same job
  `useChartTheme.js` already does for uPlot, kept separate so neither engine owns the other's
  colour rules.
- **Found and fixed a second unit bug on the way through.** `ServiceVolumeChart` and
  `ApdexBandChart` both divided `bucket.ts` by 1e6, from back when
  `/api/services/:service/timeseries` sent nanosecond strings. It now sends epoch-ms numbers, so
  every bar was plotted near epoch 1970, outside `[startMs, endMs]` — both charts drew empty. With
  the `MetricChart` fix in Task 19 that is **all four** Overview charts repaired. The mock's
  `ServiceTimeseriesPoint` was also missing every counter the volume/errors/Apdex charts read, so
  the mock UI showed three empty panels beside two full ones; the fixture now carries them.

### Task 22: Per-service data usage on `/data` `[done]`

**Files:** `crates/photon-api/src/usage.rs`, `frontend/src/components/data/*`

Byte and row attribution per `(service, environment, tenant, signal)`, from the same rollup pass. It lives on
`/data`, which owns retention and cost and has its own window — not on the health index, where it would put a
cumulative number beside per-window ones.

- [x] Steps: failing tests → implement → tests → `docs/subsystems/data.md`. 12 backend tests
  (`photon-api/src/usage_services.rs`) + 7 frontend (`DataServices.test.ts`).

**Deviations:**
- **Its own module (`usage_services.rs`), not `usage.rs`.** `usage.rs` is the sampler store — a
  15 KB file about SQLite rows and replication accounting. This is a read-path aggregation over the
  query engines and shares nothing with it but a URL prefix.
- **Not "from the same rollup pass".** The rollup rolls up SPANS, so it could only ever answer for
  traces. Attribution instead comes from a `service.name` **facet** over the logs and spans engines
  — the same facet the search UI uses, over already-pruned files.
- **Bytes are an ESTIMATE, stated on the wire and twice in the UI.** Nothing stores bytes per
  service and nothing cheaply can: Parquet is columnar and sorted by `(service.name, timestamp)`, so
  a service's rows compress *together*. The estimate is `service_rows × (store_bytes / store_rows)`,
  the response carries `estimated: true` plus the divisor, and the page prints both the word
  "estimate" and the actual bytes-per-row it used.
- **Metrics are not attributed, and that is a finding rather than a gap.** A metric series is keyed
  by `metric_name`, which leads both the sort key and the pruning bloom; faceting every metric in
  the store would need a metric-name-free scan that defeats the pruning the metrics engine is built
  on. And most metric series belong to no service at all — host and GPU metrics to a host, RUM
  vitals to a browser app, the rollup's output *about* a service rather than from one. The route
  returns `attributed: false` with the reason as prose the UI renders verbatim, and the page shows a
  **"not attributed to a service"** total so nobody reads the difference as waste.
- **Environment and tenant are not in the row key.** The plan said
  `(service, environment, tenant, signal)`. `facet` groups by one field; a three-field group-by is a
  different query shape, and per-`(service, environment)` cost is not a question anyone has asked of
  this page. Rows are keyed by service, and the estimate note says what it measured.

---

## Verification

Run at the end of every phase, not only at the end of the plan:

```bash
cargo test                                   # whole workspace
cargo clippy --all-targets && cargo fmt
cd frontend && bun run test && bun run type-check && bun run build
cd sdk/rum && bun run test && bun run size
```

`frontend/dist` must be rebuilt before `cargo test`, or `photon-api`'s `frontend_bundle_is_embedded` and
`root_serves_index_html` tests fail.

## Docs to keep in sync

`CLAUDE.md` · `CONTEXT.md` · `docs/architecture.md` (crate graph, API surface) ·
`docs/subsystems/services-apm.md` · `docs/subsystems/traces.md` · `docs/subsystems/federation.md` ·
`docs/subsystems/data.md` · `docs/subsystems/rollup.md` (new) · `docs/frontend.md` ·
`docs/conventions.md` · `docs/adr/0002` · `docs/adr/0003` · `docs/adr/0004`

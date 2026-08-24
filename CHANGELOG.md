# Changelog

All notable changes to Photon are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.7.0] - 2026-08-24

A **Backend release**: the Services / APM vertical, the landing page, and the read path underneath
them are rebuilt around **judgment rather than readings**. `/home` becomes an **Attention Board**
where every watched thing is a Subject at one ranked Attention Level; `/services` becomes a fleet
board with per-service Overview / Operations / Issues / Map / Deployments / SLO tabs, each owning its
own queries; a background **rollup** publishes RED as ordinary `photon.service.*` metrics so a wide
window is a metrics read rather than a span scan; and the whole UI moves onto a **flat token
system** where separation comes from a grey canvas instead of shadow. A read-path pass measured
against a copy of a real deployment (9 days, 86.6M spans) puts every Backend query inside a
**2 s / 24 h, 5 s / 7 d** budget — thirteen endpoints were over it and four failed outright, the
worst at 33.3 s. The decisions are recorded as ADRs
[0001](docs/adr/0001-landing-page-is-an-attention-board.md),
[0002](docs/adr/0002-red-rollups-are-published-as-metrics.md),
[0003](docs/adr/0003-a-services-request-rate-is-its-entry-spans.md) and
[0004](docs/adr/0004-the-services-index-is-a-fleet-board.md).

**Upgrading.** No config change is required, no data migration exists, and every existing Parquet
file stays readable. Two things do change under you, both deliberate and both detailed below: every
rate and error rate on the Backend pages is **recomputed over entry spans** and in most cases drops,
which rescales trace alert thresholds written against the old numbers; and the rollup's latency
histogram is renamed `photon.service.duration` → **`photon.service.latency`** (its bucket ladder is
a wire contract), which orphans histograms already on disk until `repair_latency` republishes them.

### Added

- **The Attention Board replaces the landing dashboard.** `/home` showed readings and left the
  judging to the reader — a KPI strip, two charts picked from whichever service happened to be
  busiest, three per-world panels, and no alert incidents at all despite `photon-alerts` being fully
  wired. It now answers one question: **is anything wrong, and where?** Every watched thing becomes
  a **Subject** at one ranked **Attention Level** — Down / Critical / Warning / OK, plus **Unknown**
  for the absence of a judgment. The judging is pure and table-tested (`lib/home/attention.ts`) and
  adds no fourth opinion about what "bad" means: it maps `serviceHealth.ts`, `hostStats.ts`, the
  API's own Web-Vital rating and the tenants API's own up/stale/down onto the shared axis, keeping
  each kind's native word verbatim in the reason line.
  - The **World** roll-up cards double as the page's filter (`?world=`, no tabs), so every world's
    tally stays on screen — that is what makes an empty attention list believable.
  - The board judges over its own bounded **Watch Range** (5m / 15m / 1h / 6h, `?watch=`) rather
    than the app-wide Window, so `AppShell`/`ContextBar` gain a `timePicker` prop and `/home`
    renders its own picker. Drilling out writes the Watch Range into the global range first, so the
    destination shows the same slice of time.
  - A tenant filter is a **lens**, and the board narrows honestly. Services, RUM apps and hosts
    carry the tenant dimension; uptime and the alert engine do not — so under a lens the Ops world
    is dropped rather than mislabelled as the Site's, incidents resolving to another Site's Subject
    stay out of the Unattributed banner, and per-row Site labelling switches off.
  - `TenantCard` moves to `/tenants` as a fleet-health board rather than being orphaned by the
    rewrite; `StatusPill` gains `solid` (spent on **Down** alone); `useUrlState` gains
    `useUrlParam(key, ref)`, the one-key merge-preserving counterpart for views owning a single
    param. See [`docs/adr/0001`](docs/adr/0001-landing-page-is-an-attention-board.md).

- **The Backend (Services / APM) vertical, rebuilt around judgment rather than readings.** The index
  was a RED table sorted by throughput and the detail page was four charts plus an operations list;
  neither answered "is this service healthy", "what is broken in it", "what does it talk to", or
  "did we do this to ourselves". `/services` is now a **fleet board** ranked by Attention Level
  ([ADR-0004](docs/adr/0004-the-services-index-is-a-fleet-board.md)), served by
  `GET /api/services/overview` in one request, and each service gets **sub-routes that own their own
  queries**, so an expensive tab never slows a cheap one:
  - **Overview** (`/services/:service`) — health banner, KPIs, error budgets, four charts, and key
    operations beside a **Logs panel**, which closes a real hole: a service can be on fire in its
    logs while its spans look normal.
  - **Operations** (`/services/:service/operations`) — entry points by default with all operations
    behind a stated toggle, per-row sparklines, and a per-operation **latency heatmap**
    (`GET /api/services/:service/operations/:operation/heatmap`) — the one shape three percentile
    lines cannot draw.
  - **Issues** (`/services/:service/errors`, `…/:fingerprint`, plus the fleet-wide
    `/services/errors`) — errored spans grouped by `error.fingerprint`, fingerprinted at ingest.
    Identity is the **pair** `(service, fingerprint)`, never the fingerprint alone: one bug across
    three services is three Issues. Backed by `GET /api/services/errors`,
    `GET /api/services/:service/errors[/facets|/:fingerprint]`.
  - **Map** (`/services/map`) — the topology from a single scan, self-joining child
    `(trace_id, parent_span_id)` to parent `(trace_id, span_id)`. `service` narrows **after** the
    edges exist, never as a scan filter, because scoping the scan would delete the parent spans the
    join needs; the `MAX_MAP_SPANS` cap is reported as `capped`.
  - **Deployments** (`/services/:service/deployments`, `GET/POST /api/deployments`) — releases
    **detected at ingest** from a resource `service.version` nobody has seen before, each with a
    before/after RED diff, so a 5,000-span batch costs four string lookups. CI may POST commit /
    author / message to enrich a row, but `detected_at_ms` and `source` are **write-once**, so a
    late pipeline post cannot move a deploy marker off the moment the graph changed.
  - **SLOs & error budgets** (`GET/POST /api/services/:service/slos`,
    `PATCH/DELETE …/slos/:id`, `GET …/slos/:id/budget`) — error budget and multi-window burn rate
    computed from the **rollup only**. A window the rollup does not cover comes back `partial`,
    never a part-window number dressed as a full one, and a latency objective's threshold snaps
    **down** to a `LATENCY_BOUNDS_NS` edge with the response saying which.
  - **Per-service footprint** on `/data` (a new Services tab), stated as the estimate it is:
    Parquet sorts by `service.name` and compresses a service's rows together, so there is no
    measured per-service byte figure to read.
  See [`docs/subsystems/services-apm.md`](docs/subsystems/services-apm.md).

- **The service rollup.** A background pass turns spans into per-service `photon.service.*` and
  per-operation `photon.operation.*` metrics, so the Backend pages can read a wide window without
  re-scanning raw spans. It adds no storage engine: the points go through the existing metrics WAL
  and are metrics like any other. 60-second buckets, each published exactly once; latency ships as a
  real explicit-bucket histogram so any window can be re-quantized honestly. A **first run backfills
  `[rollup].backfill_days` (default 30)**, walking one hour-wide slice per pass, so the rollup
  describes the spans already on disk and not only the ones that arrive next — measured on 300M
  spans, `/api/services/overview` took 23.1 s over 7 days scanning raw spans against 63 ms off the
  rollup. Checked against a full raw scan over the same window, request counts agree to **0.00 %**
  on every service at 1h, 6h and 24h, and the rollup answers 24h in 38 ms against the raw scan's
  2,196 ms. Tunable via `[rollup]` (`interval_secs` / `lag_secs` / `top_operations` /
  `backfill_days`) or `PHOTON_ROLLUP_*`. Three things worth knowing before you rely on it: a span
  arriving more than `lag_secs` after its bucket closed never reaches the rollup; per-service
  operation series are capped at `top_operations` with the tail summed into a single `__other__`
  series; and **coverage cannot be extended backwards** — the backfill runs only when no watermark
  exists, so once any bucket is published the floor is fixed, and a window starting before coverage
  falls back to a raw scan (a 53 ms → 24.7 s cliff). Republishing a range without double-counting
  its counters is a design decision rather than a repair, so this one is reported, not fixed. See
  [`docs/subsystems/rollup.md`](docs/subsystems/rollup.md) and
  [ADR-0002](docs/adr/0002-red-rollups-are-published-as-metrics.md).

- **HTTP status classes beside span status.** RED rows and buckets now carry `http_4xx` and
  `http_5xx` alongside `error_count`. They sit *beside* it, never instead of it: `error_count` keeps
  meaning `status_code == 2` exactly, so "error rate" continues to mean one thing, while an SDK that
  never sets span status stops hiding real HTTP failures entirely.

- **`deployment.environment` is a first-class dimension.** Promoted by default, picked in the
  ContextBar next to the time range and the federation Site, and appended to every logs / traces /
  metrics / RUM / services / infra query through one `withContext()` helper. Existing Parquet needs
  no migration — an unpromoted older file simply reads the key from its attributes map.

- **`make replica` runs the backend against a copy of a real deployment** (`photon.prod-replica.toml`
  over a hot tier copied into a gitignored `.photon-prod/`), so the read path can be worked on
  against real telemetry instead of loadgen output. Two things that copy needs, both explained in
  the config file: the replica must use a **fresh** control-plane DB rather than the source's — the
  alert engine and the uptime prober are always on and read their work out of that database, so a
  replica pointed at a production copy would deliver to production's real notification channels and
  probe production's endpoints from a laptop — and a hot tier copied out of a *running* Photon is
  not a clean snapshot, since `merge_once` unlinks files mid-copy, so
  `scripts/prune-replica-manifest.py` drops manifest entries whose Parquet did not survive.

### Changed

- **A service's request rate is now its entry spans, not every span it emits.** RED counts, error
  counts, latency percentiles, and Apdex bands are computed over SERVER and CONSUMER spans, falling
  back to trace roots and then to all spans for services that emit neither (the rule that produced
  the number travels with it as `basis`, and the UI badges the last case). Total span volume is kept
  beside it as `span_count` rather than discarded.

  **Upgrade note:** every rate and error rate on the Backend pages changes, in most cases downward.
  This is a correction — a service that instrumented generously used to report several times the
  throughput of one doing identical work, and a downstream call that failed and was retried
  successfully used to count as that service's error. **Trace alert rules are rescaled by the same
  correction** (`photon-server`'s `EngineConditionSource` reads the same RED rows), so thresholds
  written against the old inflated counts need revisiting. There is no automatic migration for this.

- **One severity vocabulary across the whole UI.** The Backend world's `critical` / `degraded` /
  `healthy` / `idle` scale is retired in favour of the Attention Level the landing page already uses:
  **Down / Critical / Warning / OK**, plus **Unknown** for the absence of a judgment. `idle` was the
  load-bearing bug — a service falling from thousands of requests per second to nothing rendered as
  calm neutral grey. It now splits correctly: reported-then-silent is **Down**, never-reported is
  **Unknown**.

- **A flat token system: separation now comes from the canvas, not from shadow.** `--background`
  drops to a visibly grey sheet with white `--surface-1` paper on top, so a card reads as a card
  with nothing but a hairline border and a 1 px ambient shadow.
  - **Elevation** is cut to three steps — `shadow-1` (hairline ambient, was a 20 px drop plus an
    inset highlight), `shadow-2` (the one real float), `sink` — dropping the `--hi` top-highlight
    token and the unused `shadow-hi` utility. Popover / dropdown / dialog / toast / tooltip move
    onto that ramp instead of stock Tailwind `shadow-md`/`lg`.
  - **One radius family** off `--radius` 0.75rem (was 0.5rem): sm 6 / md 10 / lg 12 / xl 14 /
    2xl 18; pills use `rounded-full` and sit outside the scale.
  - **Primitives**: `badge` becomes real pills with soft tonal `success`/`warning`/`error`/`brand`
    variants (`outline` is now the neutral entity chip); `stat-tile` gains `#icon`, `#action` and a
    hairline-separated `#footer`, and stops lifting on hover; `segmented` / `tabs` / `nav-tabs` /
    `toggle-group` share one switcher look (white pill on a muted track, no cyan underline);
    `switch` swaps its keycap thumb for a plain circle; `input` / `select` / `SearchBar` drop the
    inset shadow for a soft `ring-2` focus; `.pk` loses its hard bottom lip and hover lift and sinks
    1 px on `:active` (`.pk-brand` removed).
  - **`AppShell` gains a `surface` prop.** Dashboards stay on the grey canvas; the dense explorers
    (`/logs`, `/traces`, `/traces/:id`, `/metrics`) pass `surface="sheet"` for one edge-to-edge
    white working surface. A table on a canvas view must own a panel, so `RumBreakdownTable` gets
    one and `RedTable` is wrapped in the service detail view.
  See [`docs/frontend.md`](docs/frontend.md).

- **Trace-search ranking probes the newest slice first, and `matched_count` says when it is an
  estimate.** Ranking grouped every distinct `trace_id` in the window to find the newest 2,000 — one
  hash-table entry per trace id, fine at an hour and ruinous at a week. A 3-day window holds 11.4M
  ids and a 7-day one over 20M (times `target_partitions` partial aggregates), and the 7-day case
  died outright under the shared query pool: `HTTP 500` after 1.9 s, having grouped six days at full
  cost only to throw them away. The scan now starts on the newest 15 minutes and widens ×8 until the
  page fills or it covers the window (15m → 2h → 16h → 5d 8h → all of it); a busy week fills on the
  first probe, a quiet one walks to the full window at ~1.14× a single pass and lands on exactly the
  un-probed answer. Each probe reaches `STEP3_WINDOW_PAD_NANOS` further back than its own start, so
  a trace straddling a boundary cannot rank as newer than it is. `matched_count` stays **exact**
  whenever the page is short of the cap (that page *is* every matching trace — the case for every
  query a human has narrowed down); once it fills, the true total is already past anything the view
  can show, so it becomes a magnitude from a fixed-size HyperLogLog (`approx_distinct`, ~1 % relative
  error — measured 2,007 against an exact 2,001). The response carries **`matched_count_estimated`**
  so the UI renders `~11.3M` instead of passing an approximation off as a count.

- **The query memory pool is sized from the host instead of a hard-coded 512 MiB.** It is now a
  quarter of total RAM clamped to 256 MiB..=4 GiB, overridable with `[query].memory_pool_bytes` /
  `PHOTON_QUERY_MEMORY_POOL_BYTES`. It remains one *process-wide* `FairSpillPool` shared by every
  engine — a ceiling on the sum of all concurrent queries, not a per-query allowance.

### Fixed

- **Parquet filter pushdown silently emptied every fetch plan, and it is now off.** In DataFusion 43,
  `pushdown_filters` over files whose physical schemas differ — the state every
  `[schema].promoted_attributes` change puts a node in, for one whole retention period — makes any
  plan carrying a `LIMIT` return **zero rows**. Not an error, not a partial answer. Measured over
  four surviving metrics files, `count()` reported 29,819 rows while `LIMIT 1` reported none. An
  aggregate or a sort between the filter and the limit hides it, which is why only three call sites
  ever showed the symptom and why it read as "this metric does not exist" rather than as a bug —
  and why `/api/services/:service/dependencies` used to answer in 29 ms: it was returning an empty
  list. `pushdown_filters` and `reorder_filters` are now off with a regression test
  (`a_fetch_plan_over_mixed_schema_files_still_returns_rows`) pinning them there until the upstream
  bug is fixed.
- **Rollup latency percentiles were pinned at exactly 10.000 s.** The bucket ladder topped out
  there, so every slower request fell into the `+Inf` overflow bucket and the quantile read
  interpolated at the top bound — a service whose real p50 was two minutes reported ten seconds,
  with nothing marking the number as a floor rather than a measurement. The ladder now reaches
  **30 minutes across 19 buckets**. Because `explicit_bounds` are a wire contract, renumbering them
  renames the metric (`photon.service.duration` → **`photon.service.latency`**, likewise
  `photon.operation.*`), which orphans every histogram already on disk; `repair_latency` republishes
  the histogram over the covered range — and **only** the histogram, since quantiles are
  scale-invariant and counters are not.
- **That repair could not run on a federation central.** A rollup pass skips any tenant already
  publishing its own rollup, which is what protects counters from being summed twice, but the repair
  publishes no counter and so found nothing to publish. `Publish::skips_covered_tenants` limits the
  exclusion to counter publishes, so central heals its own history instead of waiting on every Site.
- **`merge_once` failed once a minute forever after a `promoted_attributes` change.** Batches
  decoded from files written under different schemas were concatenated as-is. `photon-compact` now
  aligns each decoded batch to the writing schema first (`align.rs`): a column the batch lacks
  becomes a typed null, so consolidation survives the mixed-schema state instead of erroring on
  every pass.
- **Engine failures came back as HTTP 400.** `/api/red` and `/api/metrics/query` answered every
  engine error with a 400, so a server that had exhausted its shared query memory pool under a wide
  window looked exactly like a user typo — a 7-day histogram query returned
  `400 Resources exhausted`, the UI showed the raw message instead of degrading, and nothing in an
  access log distinguished the two. Failures that are **ours** (I/O, storage, pool exhaustion) now
  return **500**; a request the engine legitimately rejected (an unsupported aggregation, `rate` on
  a gauge, an unknown group-by column) keeps its 400.
- **Four charts on the service pages had been drawing nothing.**
  `GET /api/services/:service/timeseries` changed `ts` from nanosecond strings to epoch-ms numbers
  and its consumers were never updated, so two of them multiplied and two divided — every point
  landed near epoch 1970, outside the chart window. Pre-existing on `main`, not introduced by the
  revamp.

### Performance

- **Every Backend read path is inside a 2 s / 24 h, 5 s / 7 d budget.** Stress-tested against a copy
  of a real deployment — 9 days, 86.6M spans, 6.01 GB hot tier — thirteen endpoints were over budget
  and four returned errors outright, the worst at 33.3 s. All of them clear it now; the slowest is
  1,271 ms at 24h and 1,294 ms at 7d, and everything else is under 460 ms.

  | endpoint | 24h | 7d |
  | --- | --- | --- |
  | `/services/:service/operations` | 6,246 ms → **54 ms** | 33,295 ms → **193 ms** |
  | `/services/overview` | 5,069 ms → **42 ms** | 30,301 ms → **147 ms** |
  | `/red?group=service` | 2,133 ms → **36 ms** | 13,371 ms → **131 ms** |
  | `/traces/search` (recent) | 791 ms → **91 ms** | HTTP 500 → **337 ms** |

- **Row sparklines come from one scan instead of one per row.** The Services index and the
  Operations tab each drew two mini-charts per row through their own `red_timeseries` call, capped
  at 40 — so a page load was 1 + N full scans of the same spans. `red_spark` groups by series and
  time bucket at once and returns them all from a single scan.
- **A span query no longer touches the `attributes` map unless a surviving file might hold the key.**
  A span Parquet's `attributes` map is 63 % of its bytes and **96 % of its decoded bytes**, so
  referencing it at all costs about the same however many keys are probed. The manifest already
  records each file's long-tail keys, so `attr_lookup` resolves a key that no surviving file declares
  to a typed NULL literal and the map column never enters the plan (`AttrKeys::Unknown` — at least
  one file did not declare its keys — keeps the probe, conservatively). Measured on 300M spans,
  `/api/red` over 7 days: **12.3 s touching the map against 2.2 s without.**

## [1.6.0] - 2026-08-12

A **federation release**: satellite Photon instances now report into one central Photon. Nothing
about the single-binary model changes — no clustering, no consensus, no new storage engine;
federation is opt-in from the tenant side and rides the OTLP ingest path that was already there.
Summary mode (the default) pushes a periodic health snapshot as OTLP metrics; full mode additionally
mirrors accepted batches — logs, traces, metrics, RUM, or any subset — through a bounded queue that
drops under backpressure rather than ever touching the ack path, so central being down cannot affect
a tenant's own ingest or UI. Central gains a tenant registry with per-tenant ingest tokens and
**server-side tenant stamping** (tenants can't spoof each other), a fleet board on Home, a
`/tenants` view, and a read-only tenant lens over `/rum`. The global context is now **time +
tenant** — the entity-scope dimension, decorative since it shipped, is gone. Also fixed: the Home
and Service-detail charts that had been rendering empty since v1.0.0. Fully backward compatible —
federation stays off unless configured; no migration, no format change.

### Added

- **Multi-tenant federation: satellite Photon instances report into one central Photon.** Opt-in
  via a tenant-side `[federation]` config block (`endpoint`, `token`, `mode`, `interval_secs`,
  `queue_batches`, `signals`; `PHOTON_FEDERATION_*` env overrides; invalid combinations fail
  startup, not silently). Everything rides the existing OTLP ingest path — no clustering, no new
  storage engine. See [`docs/subsystems/federation.md`](docs/subsystems/federation.md).
  - **Summary mode** (default): a background pusher periodically sends a health snapshot as OTLP
    metrics — liveness (`photon.federation.up`), per-signal ingest rates, open-incident count from
    the local alert engine, disk usage. Best-effort by design: central being down never affects the
    tenant's own ingest or UI.
  - **Full mode**: an ingest-side tee mirrors accepted OTLP batches to central through a bounded
    queue + forwarder — drop-oldest under backpressure, never on the ack path. The summary pusher
    runs in this mode too. **`signals`** narrows what full mode mirrors (e.g.
    `signals = ["traces"]` for services/APM at central without shipping logs); the subset is
    encoded in the summary's mode attribute (`full:traces`) so central can show what's mirrored.
  - **Central-side tenant registry** in the control-plane SQLite DB (`GET/POST /api/tenants`,
    `PATCH/DELETE /api/tenants/:id`, token rotation). A registered tenant gets a `tk_tenant_…`
    ingest token; ingest resolves it and **server-side stamps** every row with the tenant id —
    client-supplied tenant attributes are overwritten, so tenants can't spoof each other.
  - **Fleet UI**: a tenant board on Home fed by the curated `GET /api/tenants/summary` (status:
    up ≤ 2 min since the last summary, stale ≤ 10 min, down after; ingest sparkline, incidents,
    disk, mode badge), a `/tenants` registry view, and a manage dialog that emits a copy-paste
    `[federation]` snippet (summary / full / traces-only presets, as TOML or `PHOTON_FEDERATION_*`
    env vars). `GET /api/federation/status`
    exposes pusher/forwarder health in the tenant's own UI.
  - The global context is now **time + federation tenant** — the entity-scope dimension was
    removed as decorative-only. Picking a tenant filters logs/traces/metrics and the curated
    services/RED/infra endpoints via the query grammar (`tenant:<id>`).
  - **RUM federation**: RUM is its own federation signal — `signals` may include `"rum"`, and
    **omitting `signals` in full mode now mirrors all four signals** (logs, traces, metrics, rum;
    previously all three — set an explicit subset to keep the old behavior). The browser beacon
    bypasses the OTLP tee, so the tenant side wraps the local RUM write path (`TeeingRumSink`):
    after each successful local write it synthesizes OTLP protobuf per beacon (no batching) and
    offers it to the existing tee — vitals to `/v1/metrics`, errors to `/v1/logs` — encoded in the
    summary mode attribute (`full:traces,rum`). Central derives tenant apps from the mirrored data
    (`GET /api/rum/apps?tenant=` — distinct `service.name` among `web_vitals.*`; deliberately no
    registry sync), and its `/rum` views become a read-only tenant lens when the tenant context is
    active: the RUM read endpoints gain an optional `q` grammar filter carrying `tenant:<id>`, and
    app management is hidden under the lens. Uptime remains local-only. Mirrored error records
    keep their `trace_id`/`span_id` (log→trace correlation works on central when traces are
    mirrored too), RUM alert rules evaluate over local data only (`-tenant:*`), and
    `?tenant=<name>` 404s a tenant that isn't in the registry.

### Fixed

- **Home "Traffic & errors" / "Latency" charts (and the Service detail charts) rendered empty**
  while their corner sparklines showed data: `GET /api/services/:service/timeseries` emitted each
  bucket's `ts` as a nanosecond *string*, but the UI plots it raw as an epoch-ms number, so every
  point landed far outside the chart's x-window. The endpoint now serves `ts` as an epoch-ms
  number (present since v1.0.0).

## [1.5.0] - 2026-08-04

A **performance release driven by production measurement**, not by benchmarks: every change below
was found on a live 15-day deployment (95M records, 6.1 GB of spans across 7,012 files, on a
rotational disk) and verified against its own data. Small-file merging had silently stalled at a
fixed point, skip-index pruning paid full seek latency one file at a time, an idle server held
~700 MB of freed heap, and compacted Parquet was taking library-default column encodings that were
wrong for its two largest column shapes. Two of the fixes cost nothing (the encoding change and the
prune fan-out are the same or faster), and one long-standing config recommendation turned out to be
backwards. Failed searches also stop disguising themselves as empty results. Infrastructure
monitoring gains a per-host Processes table. Fully backward compatible — no migration, no format
change; existing Parquet files stay readable and are re-encoded opportunistically as merging
rewrites them.

### Added

- **Per-host Processes table on `/infra/:host`, behind `GET /api/infra/hosts/:host/processes`.**
  Answers "which process is heaviest on this node" alongside the host-level vitals. A row is one
  **`service.name` on that host** (not one OS process): `photon-query`'s `infra_host_processes`
  enumerates the distinct `service.name` among `process.*` metrics scoped to the host (mirroring
  `infra_host_detail`'s `MetricRequest.host` + `host.name` row predicate) and window-averages
  cpu%/rss/fds/threads/restarts plus `last_seen` — `restarts` aggregates with **max**, not avg,
  since it's a cumulative counter. Metric names read the OTel process semconv as the primary
  contract (`process.cpu.utilization` ×100 for the percent column, `process.memory.usage`,
  `process.unix.file_descriptor.count`, `process.thread.count`) and fall back to the earlier
  bespoke names (`process.cpu.percent`, `.memory.rss`, `.open_fds`, `.threads`);
  `process.restarts` is a documented Photon extension with no OTel equivalent. The UI is
  `components/infra/HostProcessesTable.vue` on the shared `ui/table` primitives — click-to-sort
  headers, default CPU descending, nulls last as `—`, top-N capped, `EmptyState` when nothing
  reports. Any producer tagging `process.*` with `service.name` + `host.name` lights this up with
  no further config.
- **`GET /api/infra/hosts` rows carry the across-group mean beside the worst group.** Alongside
  `diskUtil`/`gpuUtil` (the worst mountpoint/GPU) each row now sends `diskUtilAvg`/`gpuUtilAvg` and
  `diskGroups`/`gpuGroups` — the mean across mountpoints/devices and how many there were, folded
  from the same single aggregation pass (no extra query). Purely additive; existing consumers of
  `diskUtil`/`gpuUtil` are unaffected.

### Changed

- **Compacted Parquet picks its per-column encodings instead of taking parquet-rs's defaults —
  logs 18% smaller, spans 11.5% smaller, at equal or lower CPU.** parquet-rs defaults every column
  to dictionary-first, which is right for the low-cardinality string and enum columns and wrong for
  the two shapes that dominate these files. On a 15-day production corpus (6.1 GB of spans), the
  `Timestamp`/`Int64` time columns were **30% of a spans file at a compression ratio of 1.2×** —
  every value distinct, so the dictionary spills and the column falls back to PLAIN, a flat 8
  bytes/row zstd barely dents — and the random hex ids (`trace_id`/`span_id`/`parent_span_id`) were
  another **33%**, paying for a dictionary as large as the data it indexes. `writer_properties`
  (`photon-compact/src/stream.rs`) now assigns `DELTA_BINARY_PACKED` / `DELTA_BYTE_ARRAY` with the
  dictionary off for those, deriving the choice from the Arrow type rather than a per-signal column
  list. This is not a size/CPU trade: encoding the same batch measured **the same or faster** than
  before. Three boundaries are deliberate and each is pinned by a test:
  - **`Int32` keeps its dictionary.** It carries only small enums (`severity_number`, `kind`,
    `status_code`, `metric_type`, `temporality`) where a handful of distinct values RLE-compress to
    nearly nothing; restricting delta to `Timestamp`/`Int64` measured 0.4 pp *better* on logs than
    the broader "all integers" rule, so the narrower one costs nothing.
  - **Metrics opts out of the time-column rule** (`TimeEncoding::Dictionary`). Its sort key is
    `(metric_name, service.name, host.name, timestamp)` — timestamp is last of four, so it restarts
    per series rather than climbing, and one scrape stamps many rows with the *identical* instant.
    That is the dictionary's best case: forcing delta there measured **2.4% larger**, so metrics
    output is unchanged.
  - **The hex-id rule stays global**, since it describes the content being random, which no sort
    key changes.
  Write-side only. Parquet records each column's encoding in its own metadata, so files written
  either way read back identically and there is no migration: existing files keep their old
  encoding until retention ages them out, or until a `merge_once` pass rewrites them — merge shares
  the same `compact_segment` path, so consolidation re-encodes as it goes.
- **`zstd_level`'s guidance was wrong and is now corrected.** `photon.example.toml` and
  `docs/architecture.md` both claimed "roughly 10-30% smaller at level 3". Measured on the same
  production corpus, **level 3 is ~3% _larger_ than level 1** — zstd changes strategy in a way that
  hurts these small, already well-encoded pages. The default (1) is unchanged; the docs now say to
  skip to 6 or 9 (~−9 pp on spans for ~2.7× the compaction CPU) and to measure first, and note that
  on a rotational disk the extra rewrite I/O competes with queries even though compaction CPU is
  off the ingest ack path.
- **The traces grain now names what each number counts.** Filtering `kind:server` listed traces
  rooted at a service with no server span. That is correct — a query matches **spans**, and the
  traces grain returns the whole trace around each match (`trace_list` finds trace ids with ≥1
  matching span, then refetches every span of those traces unfiltered for the rollups) — but
  nothing in the UI said so, and three labels implied otherwise. No semantics changed, only the
  labelling: the root-service column is now headed **"Root service"** (it renders `root_service`,
  which may not be what matched, and now agrees with its "Root operation" neighbour); the result
  count reads **"44 traces containing a matching span"** whenever a query narrows the traces grain
  (silent with no query, and in the spans grain where the rows *are* the matches); and the filter
  rail is captioned **"counts are spans"**, since its facets run on the spans engine in both
  grains — which is why a rail total (47) and a list total (44) can legitimately disagree. Each
  carries the full explanation on hover, including a pointer to the Spans grain for anyone who
  wanted the matching spans themselves.
- **The Infrastructure nav-rail world is headed "Infra".** The rail is 74 px wide (~66 px of
  content); at 9 px semibold uppercase with `tracking-wider`, "INFRASTRUCTURE" needs ~84 px, so it
  bled past both edges and was clipped by the rail border, reading as a mis-centred label beside
  FRONTEND/BACKEND. "Infra" fits and matches the `/infra` route; the full word still appears
  wherever it has room (breadcrumbs, docs). Rail headings also truncate now, so a future over-long
  one clips cleanly instead of bleeding across the edge.
- **The frontend dev server's API proxy target is overridable via `PHOTON_API_PROXY`.**
  `bun run dev` hard-coded `:8080`, so a taken port meant editing a tracked file. The default
  still matches what `process-compose` starts.
- **Infra utilization headlines no longer read as whole-host numbers.** Disk and GPU are
  label-split (mountpoints, devices) and their headline is deliberately the **worst** group — a
  full `/` must not be averaged away against an idle `/boot/efi` — but shown alone that max
  misleads the other way: `GPU 76%` above a chart of four GPUs, three of them near zero, looks
  like a busy host when the average is 20%. Every split resource now shows the pair, and names
  the group the max came from:
  - `/infra/:host` tiles read `76% max · 20% avg` over `gpu 2 of 4` (likewise Disk with its
    mountpoint, and GPU temp). Single-mountpoint/single-GPU hosts keep a plain reading — the mean
    would only repeat the max.
  - `/infra` host cards read `85% / 30%` on their DSK/GPU meter rows.
  - Each split chart card states its split in the subtitle (`5 mountpoints`,
    `NVIDIA A2 · 4 devices`), so the tile numbers have something visible to point at.

  Host **status** is deliberately unchanged: the `⚠ DSK` card flag and the fleet
  Warning/Critical counts still follow the worst group, so a host with `/data` at 95% never reads
  as healthy.
- **`photon-server` peak memory during ingest cut sharply** (measured 150–250 MB territory on a
  moderate single node; the dominant terms below scale it down several-fold):
  - Default `wal.segment_max_bytes` **128 MiB → 32 MiB**. The compactor's peak working set is
    ~2× the decoded segment per signal (×3 signals on the same 2 s cadence), so this directly
    shrinks the largest ingest-time term. Smaller segments mean smaller/more Parquet files;
    `merge_once` consolidates them as before.
  - Compacted Parquet **row groups capped at 131,072 rows** (was the parquet-rs default of
    1,048,576) — bounds the `ArrowWriter`'s in-progress buffer and makes row-group pruning
    more granular.
  - Ingest HTTP handlers **release the request body buffer right after protobuf decode**
    instead of holding it (up to 16 MiB per in-flight request) through the WAL fsync ack.
  - Default live-tail `[live].broadcast_capacity` **1024 → 128**. While any SSE client is
    connected, the broadcast ring pins the last N ingested batches (a `tokio::broadcast` slot
    frees only on overwrite) — at 1024 that alone reached hundreds of MB under load. Lagging
    subscribers get `Lagged` and skip the gap, which live tail tolerates by design.
- **DataFusion query memory is now one process-wide budget, fairly shared.** Every query session
  draws on a single 512 MiB `FairSpillPool` (was: a fresh *per-query* 512 MiB greedy pool, so a
  dashboard fanning out N queries could claim N × 512 MiB with no aggregate bound). Spillable
  operators (sorts, aggregations) get an even share of the budget and **spill to the OS temp dir
  instead of failing** when they hit it; only a truly unspillable operator exceeding the remaining
  budget still errors with `ResourcesExhausted` — the deliberate final backstop that replaces
  OOM-killing the process.
- **Metrics series queries bound their row collection.** The pointwise (`rate`/`increase`/`last`)
  and distribution (histogram/exp-histogram/summary) paths now cap collected rows at 200,000
  (`MAX_COLLECT_ROWS`), applied after the sort so truncation keeps a deterministic leading prefix,
  and surface truncation through the existing `capped` response field. Caveat that rides
  `capped=true`: the one series straddling the cap keeps only its earliest rows, so its own value
  can be understated — the server log warning now names which cap actually tripped.
- **`photon-agent` memory footprint cut ~3× — it no longer retains the host's full process
  table.** `sysinfo` is initialized with only the CPU-usage + RAM refresh kinds instead of
  `new_all()`/`refresh_all()`; host identity (`host.id`/`os.type`) is resolved once at
  startup instead of re-reading `/etc/machine-id` + hostname every cycle; NVML GPU names are
  cached per device instead of queried from the driver each sample. Measured on a dev host:
  14 → 5 MB RSS (`--no-gpu`) and 35 → 25 MB with GPU metrics (the remaining ~20 MB is the
  `libnvidia-ml` driver library, outside Photon's control). Busy fleets see a larger drop —
  the removed process-table snapshot scaled with process count.

### Fixed

- **An idle server no longer holds hundreds of MB of freed heap.** jemalloc has been the global
  allocator for a while, but on **stock settings**: `background_thread` is off by default and decay
  is driven by allocation activity *within each arena*, so a process that stops allocating never
  runs the purge that would return its dirty pages to the OS. A production node showed exactly
  that — **734 MB RSS at 2% CPU** after 4 days, 698 MB of it anonymous across 863 mappings, with no
  SSE clients and ~68 spans/s of ingest: peak compaction/ingest buffers, freed but never handed
  back. `photon-server` now exports a `_rjem_malloc_conf` static setting `background_thread:true`
  with 1 s `dirty_decay_ms`/`muzzy_decay_ms`, so a dedicated jemalloc thread purges on a timer and
  RSS tracks the live working set instead of the high-water mark. Two traps worth knowing (both
  written up in `docs/conventions.md`): the symbol **must** be `_rjem_malloc_conf` — `tikv-jemallocator`
  builds jemalloc with the `_rjem_` prefix, so a plain `malloc_conf` is silently ignored and every
  setting is lost, and the matching env var is `_RJEM_MALLOC_CONF` (which makes it the way to try a
  setting against a *running* deployment with no rebuild); and it is `#[cfg(target_os = "linux")]`
  on purpose, since macOS does not support `background_thread` and warns on every run. Note
  `MALLOC_ARENA_MAX` does nothing here — it is a glibc tunable and glibc is not the allocator.
- **Small-file merging no longer stalls, so the Parquet file population stays bounded.**
  `merge_once` partitioned on a 10,000-row `MERGE_ROW_THRESHOLD` used as an *input classifier*,
  with no notion of a target output size — which gave merging a **fixed point barely above the
  threshold**: a pass folded small files together, its own output landed just past 10k rows, and
  every later pass then classified that output as "large" and skipped it forever. Under steady
  ingest every file was born above the threshold, so merging silently became a no-op. Observed on
  a 14.5-day, 85.3M-span deployment: **6,861 files averaging 12,429 rows / 0.83 MiB**, 6,860 of
  them permanently ineligible (98% were themselves frozen merge outputs), row counts pinned just
  past the threshold (p50 12,326, max 19,756), and a 10.5 MB spans manifest. All three compactors
  now merge toward an **output target** (`MERGE_TARGET_ROWS`, 150k rows), taking files oldest-first
  until the output would reach it (min 2, max `MERGE_MAX_FILES_PER_PASS`) — so a file climbs toward
  the target across passes instead of freezing, while peak per-pass memory stays at parity with the
  old `32 × 10k` ceiling. Selection stays oldest-first so merged outputs remain time-adjacent and
  keep pruning well.
- **Skip-index pruning fans out across the blocking pool instead of looping sequentially.** Each
  candidate file's `.idx` sidecar is a separate small random read; issued one at a time, every one
  paid full rotational latency on a spinning disk. Measured on a 3 TB rotational volume:
  **10.9 ms/file**, so a 7-day trace search over 2,707 candidates spent **~16 s in prune alone**
  before DataFusion opened a single Parquet file — while a warm page cache hid it completely
  (0.10 ms/file), which is why the symptom read as erratic rather than slow. All three engines —
  **logs, spans and metrics** — now split candidates into 64 contiguous slices once the list
  exceeds 64 entries, each re-deriving `candidates()` from a shared `Arc<Manifest>` (a pure
  in-memory filter, no `FileEntry` cloning) and awaited in order so the survivor list is
  byte-identical to the sequential prune's. **~4.5× faster on the same disk** (2.43 ms/file at
  64-way; sweep: 1→10.9, 4→4.7, 8→4.4, 16→3.4, 32→2.8, 64→2.4). Neutral-to-positive on flash and
  on a warm cache. The two tuning constants live once in `lib.rs` rather than per signal: unlike
  the query logic they sit in, the disk behaviour they compensate for belongs to the machine, not
  the signal. Each engine keeps its sequential `prune` for synchronous callers
  (`metric_catalog.rs`) and unit tests; only the DataFrame path fans out. `trace_candidates` stays
  sequential on purpose — a `time_hint` narrows it to a ±1 h window, so it sits below the fan-out
  threshold anyway.
- **Search handlers no longer report a failed query as an empty result.** `POST /api/search`,
  `/api/traces/search` and `/api/spans/search` caught every engine error, logged a warning and
  returned an empty-but-`200` page. That was meant to keep a *fresh* server from 500-ing, but the
  engines already handle that on their `Ok` path (nothing survives pruning ⇒ empty result, not an
  `Err`), so the fallback only ever fired on genuine failures — rendering a `ResourcesExhausted`
  from the shared 512 MiB query pool as "your search matched nothing", with the real cause visible
  only in the server log. Hard failures are now a **500** carrying `{"error": ...}`. The frontend
  mirrors it: `api.ts`'s mock fallback now triggers only when there is no HTTP response at all
  (genuine dev-without-backend), never on a 4xx/5xx, so a failing backend can't be silently
  replaced with demo data. 401 still falls through to the router's auth guard rather than
  surfacing as a query error.
- **Span bars, chart swatches, health dots and severity tones rendered with no background.**
  Tailwind's `content` glob covered only `.vue`/`.js`, so every class literal declared in a `.ts`
  module was purged from the bundle — `SERVICE_PALETTE` (`serviceColor.ts`), the chart swatches
  (`seriesColor.ts`), the health dots (`serviceHealth.ts`) and the severity tones (`format.ts`).
  The classes still reached the DOM, so nothing threw and no test failed; the waterfall's span
  bars and the "time by service" band simply had no colour. `ts` added to the glob and guarded by
  `tailwind.config.test.js`.
- **The traces back button no longer loses your filters.** `/traces?…&q=kind%3Aserver` drilled
  into a trace and came back as `/traces?…&sort=recent&mode=traces` — two distinct defects, both
  from raw `history.replaceState(null, …)` calls that merged query keys outside vue-router.
  (1) `replaceState` *replaces* the entry's state, and vue-router keeps its
  `{back, current, forward, position, scroll}` bookkeeping there; passing `null` wiped it, and
  `router.afterEach(syncContextToUrl)` runs on every navigation, so the state was destroyed the
  moment you landed on the detail route — `history.state.back` was gone, so `useBackTo` could never
  recognise the list entry and always took the filterless push fallback. (2) Preserving the state
  alone wasn't enough: vue-router tracks the entry's URL in `state.current` and re-asserts it on
  the way out, so a stale `current` *reverted* the merged URL. Every merge-write now goes through
  `replaceSearch()` (`lib/core/historyUrl.ts`), which preserves the state object and moves
  `current` with the URL, making both impossible to reintroduce per-call-site. Regression test
  drives the real reported chain over a real `createWebHistory` router.
- **Trace waterfall: clipped labels, invisible end-of-trace spans, misaligned axis.** The label
  column was capped at 320 px and truncated service + operation names on deep traces — it's now
  drag/keyboard resizable (160–720 px, persisted to `localStorage`), with axis, gridlines and rows
  all reading one computed split. Bar geometry floored width via `min(width, 100 - left)`, which
  erased spans sitting at the very end of a clock-skewed trace; `left` now slides back instead, so
  the 0.5% floor always holds, and the duration caption flips to the other side of its anchor near
  the right edge rather than being clipped. The axis was inset `mx-3` while rows were flush-left
  with `pr-3` and the gridline overlay had neither, drifting ticks 12 px off the bars.
- **The service "Related ▾" pivots actually filter their destination now.** `relatedFor('service')`
  sent a `svc` query param to `/metrics`, `/rum` and `/uptime`; no destination read it, so all
  three landed unfiltered. Each now sends the key its target view consumes — `/metrics` gets
  `q=service:<svc>`, the same grammar filter `MetricsExplorer` already persists. That also exposed
  an ordering bug: the URL seed ran *after* `refDebounced(filter, 180)` was created, so its initial
  value was the empty pre-seed filter and any `/metrics?q=` deep link fired one unfiltered series
  query before self-correcting 180 ms later. The seed moved up with the builder refs, ahead of
  everything derived from them.
- **`photon-agent` sender-loop hardening.** HTTP posts now carry a 10 s timeout — previously
  a hung/black-holed server could stall sampling forever (and block Ctrl-C, since the signal
  isn't polled mid-send). The Ctrl-C handler is registered once instead of re-created every
  loop iteration, and missed ticks are delayed instead of bursting a backlog of catch-up
  POSTs after host suspend/resume.
- **Agent cumulative sums (`system.network.io`) now carry `start_time_unix_nano`** (the
  process start) per the OTLP data model, so consumers can compute rates and detect counter
  resets; gauges keep no start time.

## [1.4.0] - 2026-07-21

A feature release focused on **infrastructure monitoring UX**: the host detail page becomes
a two-layer monitoring view, the hosts list gains a fleet executive summary, and release
tags now ship a prebuilt `photon-agent` binary. Chart rendering fixes (percent axes, axis
label clipping, legends) ride along. Fully backward compatible.

### Added

- **Prebuilt `photon-agent` binary on GitHub Releases.** Release tags now also build and
  attach a stripped Linux x86_64 agent tarball (+ SHA-256 checksum) with a stable
  `releases/latest/download/photon-agent-linux-x86_64.tar.gz` URL — no more building from
  source and `scp`-ing to each host. `deploy/README.md` was rewritten as a full step-by-step
  guide (server, agent, and app telemetry) for technical and non-technical readers.

- **Infra host detail v2 (`/infra/:host`).** The host page is now a two-layer monitoring
  view: a glance **stat-tile row** (CPU · Memory with absolute GB · worst-mountpoint disk ·
  network rate · GPU util · GPU temp; warn/error tint at the shared 80%/90% thresholds;
  CPU + Memory sparklines) above per-resource **trend sections in ChartPanel cards** — CPU
  with a `total | per-core` toggle and a 1m load-average chart, Memory + Network, Disk
  per-mountpoint meters + trend, and a 4-chart GPU section (utilization, memory,
  temperature, power). Tiles derive from the last point of the same series the charts
  plot — no extra API calls. Backed by four new curated resources on
  `GET /api/infra/hosts/:host/timeseries`: `gpu_memory`, `gpu_temp`, `gpu_power`, `load`.
- **Infra fleet executive summary (`/infra`).** The hosts list is now a **fleet KPI band**
  (hosts · warning · critical · avg CPU · GPU hosts — a degraded host counts in exactly one
  bucket) above a **host-card grid** (CPU/MEM/DSK/GPU meters, degraded border tint +
  worst-resource flag, last-seen) replacing the old table, so fleet health reads at a
  glance without opening each host. `GET /api/infra/hosts` rows gain `diskUtil` (the
  **worst** mountpoint, not an average) and `gpuUtil`.

### Fixed

- **Chart y-axis labels no longer clip** (the "00 By/s" artifact): axes auto-size to the
  widest formatted label, and byte-rate axes use compact units (`2.1 MB/s`).
- **Utilization charts render on a fixed 0–100 % axis** instead of auto-zooming to a
  sliver of raw fractions (e.g. memory at "0.468–0.478").
- **Chart legends never wrap** into a multi-row block — one horizontally scrollable row.

### Changed

- **`MetricChart`'s percent handling is an explicit contract**, not a unit-string side
  effect: `unit` is a pure label; the ×100 fraction transform is an opt-in `percent` prop
  and axis pinning an explicit `yRange` — so third-party OTLP metrics declaring `unit="%"`
  in the metrics explorer are never silently rescaled (regression-tested). The services
  "Error %" chart keeps its auto-ranged axis.
- `StatTile` gains an optional `sub` line and `#spark` slot (additive).

## [1.3.0] - 2026-07-20

A feature release adding **system-wide alerting & notifications** — a cross-signal
webhook alert engine with provider-native channel presets — on top of correctness fixes
to the RUM pages breakdown and the WAL. Fully backward compatible: alerting is always-on
with sensible defaults and no required config, and every change is additive.

### Added

- **System-wide webhook alert engine (`photon-alerts`).** Rules watch **metrics, logs,
  traces, and RUM** and fire a webhook when a condition holds, moving each `(rule, series)`
  through a pure **OK · Pending · Triggered · Resolved** state machine. Incidents,
  notification channels, and per-rule severity / `for`-duration / evaluation interval are
  all UI/SQLite-managed (no config surface); the engine is a read-path consumer of the
  three query engines and is always on (optional `[alerts]` tunes only defaults). Uptime
  up/down transitions **bridge onto the same incident history + channels**, so there is one
  notification system, not two. New `/alerts` UI (rules · incidents · channels) and the
  `/api/alerts/*` surface.
- **Alert rule templates.** A target-first **"Browse templates"** quick-setup on the Rules
  tab: pick a target (Service · RUM app · Host · Global) and a concrete instance from live
  data, then **Apply** or **Customize** from a 23-template catalog — a frontend-only on-ramp
  that flows straight through the existing rule-create path.
- **Provider channel presets — Discord & Telegram.** Notification channels are now typed
  presets: the original **Generic webhook** (+HMAC) plus **Discord** (native embed) and
  **Telegram** (Bot API, HTML), each rendered by a pure `format.rs`. Pick a preset and fill
  in only its fields (Discord webhook URL; Telegram bot token + chat id). Channel input is
  validated (Discord host-locked to Discord's own hosts; Telegram bot-token shape), and a
  channel **Test** now performs one real, awaited delivery and reports the actual outcome —
  including for an **unsaved draft**, straight from the create/edit dialog. Discord
  (host-locked) and Telegram (server-constructed `api.telegram.org` endpoint) are SSRF-free;
  only the Generic webhook can target an arbitrary host.

### Fixed

- **Soft-navigated routes missing from the RUM pages list.** Two compounding drops hid
  clean soft views (no layout shift, no slow interaction) from `/rum/:app/pages`:
  - *SDK*: `beacon.flush()` skipped views whose buffers were empty, so such a view never
    reached the server at all. The first flush of a view is now its **finalizing beacon**,
    sent even with empty buffers so its `view.dur` → `web_vitals.view_duration` pageview
    marker always lands — and `dur` is now emitted **exactly once per view id** (repeat
    flushes, e.g. `visibilitychange` then `pagehide`, previously double-counted
    `view_duration` when new vitals accrued in between).
  - *Query*: the pages breakdown (`rum_breakdown`) counted pageviews only from
    LCP/INP/CLS sample counts — but soft views never emit LCP and emit CLS/INP only when
    nonzero, so a route reached exclusively by clean soft navigations stored
    `route_change`/`view_duration` points the pages list never looked at.
    `web_vitals.view_duration` (one point per finalized view — the true pageview count)
    now joins the merge.
- **Idle WAL segments never became queryable.** Age-based segment rotation ran only
  after a commit, so on a low-traffic instance the data in the active segment stayed
  invisible to the compactor — and to every query — until the *next* write happened to
  arrive, no matter how long you waited. The WAL writer's idle wait now wakes at the
  active segment's age deadline and seals it, so ingested data always becomes queryable
  within ~`segment_max_age_secs` even with zero follow-up traffic. Applies to all three
  WALs (logs, spans, metrics); no config change.

## [1.2.0] - 2026-07-15

A feature release adding first-class Single-Page-App (SPA) support to the RUM SDK,
plus the metrics and attributes to store and query it. Fully backward compatible —
older SDKs and existing data are unaffected (every new beacon field and metric is
additive; unknown fields are ignored).

### Added

- **SPA / soft-navigation RUM tracking.** The `@photon/rum` SDK now models a **view**
  as a logical pageview instead of a document load: `view.id` rotates on every real
  client-side route change (History API — `pushState`/`replaceState`/`popstate`,
  auto-detected, on by default; query/hash-only changes don't rotate, and MPAs are
  unaffected). Each route becomes its own pageview with correctly-attributed Web
  Vitals, JS errors, and — with `tracing: true` — its own backend trace. Attribution
  is by construction (a per-view beacon buffer flushed on each rotation), not
  flush-time timing. Fixes SPA routers (e.g. TanStack Router) reporting no data on
  in-app navigation.
- **Honest per-route Web Vitals.** Soft-navigated routes report per-view **CLS**
  (web-vitals' session-window rule) and **INP**, plus a new **`web_vitals.route_change`**
  metric — a DOM-settle transition-time heuristic (good ≤ 1 s / poor > 3 s). LCP/FCP/
  TTFB stay real web-vitals for the landing load and are **never** synthesized for soft
  navigations. A new **`web_vitals.view_duration`** metric captures time-on-view.
- **New RUM attributes** on every vital point and error log — `nav` (`hard` | `soft`),
  `view.seq` (ordinal within the session), and `view.previous_route` — enabling
  navigation-path and engagement analysis.
- **`trackView(route?)` SDK export** — a manual escape hatch for routers that prefer to
  drive soft-navigation boundaries themselves (e.g. TanStack Router's `router.subscribe`).

## [1.1.0] - 2026-07-15

A hardening release on top of 1.0.0: correctness, durability, and DoS fixes across
the read and write paths, meaningful transfer/allocation wins, and a few new
operator-facing knobs. Fully backward compatible — no config or API breaking changes.

### Added

- **Response compression.** The whole HTTP surface (JSON API + embedded UI) now
  content-negotiates gzip/br per the client's `Accept-Encoding`. A ~115 KB / 500-row
  `/api/search` payload compresses ~15× to ~8 KB. Live-tail SSE (`/api/stream/*`),
  gRPC, images, and sub-32-byte bodies are skipped, so streaming is never buffered.
- **Gzipped OTLP ingest.** Both the HTTP and gRPC receivers now accept gzip-compressed
  OTLP, so a stock OpenTelemetry Collector (which gzips by default) works out of the box.
- **Configurable Parquet compression.** New `[storage].zstd_level` (1–19, default 1)
  tunes the compactor's zstd level. The default is byte-identical to the previous
  hardcoded level.
- **Ingest body-size cap.** `[ingest].max_body_bytes` (16 MiB default, override with
  `PHOTON_INGEST_MAX_BODY_BYTES`) is enforced on the *decompressed* request body across
  all receivers, bounding gzip bombs and pre-allocation blowups.
- **`PHOTON_DISABLE_COMPACTION`** environment variable to gate the three background
  compactors (dev/ops).
- **Retention ceiling.** A `MAX_RETENTION_DAYS` cap is now validated in config, in the
  retention API route, and in the server retention loop.

### Changed

- **The durable replica now honors retention.** Retention deletes replicate to the
  durable (S3-compatible) store through a unified Upload/Delete replication queue
  (NotFound-tolerant), so the replica no longer grows forever.
- **Storage/usage stats come from the manifest.** Each file's on-disk size is captured
  at compaction time (`FileEntry.bytes`); footprint stats are now manifest arithmetic
  with a `stat()` fallback for files written before this release.
- **Deterministic span pagination.** All span sorts gain a stable `(span_id, trace_id)`
  final tiebreaker, so paginating across exact-key ties is deterministic (this changes
  the tie-order of the "Recent" sort).
- **Bounded query memory.** A 512 MiB DataFusion memory pool makes the facet/metrics
  paths fail loud (`ResourcesExhausted`) instead of OOMing the node.

### Fixed

- **Substring search no longer over-prunes.** Bloom pruning could drop files that
  contained partial-word matches (e.g. `tim` inside `timeout`); interior tokens are now
  bloom-tested on both sides, so a matching file is never pruned away.
- **Corrupt skip-index sidecars are tolerated.** A corrupt/unreadable `.idx` now *keeps*
  the file (conservative pruning) instead of aborting the query or panicking.
- **Wide time windows bucket correctly.** Fixed i64 overflow in time-bucket math so
  30–90 day windows bucket consistently.
- **Histogram reset detection.** Classic-histogram resets now compare bucket bounds by
  value, so a redefined histogram no longer corrupts the reset-aware delta.
- **Crash durability on the write path.** On a failed WAL commit, the torn tail is rolled
  back (or the segment rotated) and poisoned/recovered before the next write — restoring
  the "ack ⇒ survives crash" guarantee even under compound disk failure.
- **No overflow panics on untrusted OTLP.** Span timestamps and durations saturate /
  `checked_sub` instead of panicking on debug builds or producing negative durations.
- **Compaction durability & robustness.** The manifest is fsynced before any file is
  unlinked; empty WAL segments no longer emit a zero-row Parquet file; the replicator
  backs off and re-enqueues instead of silently dropping on retry exhaustion; compactor
  tasks are supervised; and stale `*.tmp` files are swept on startup.
- **Frontend request churn (logs view).** The volume histogram and facet requests are now
  debounced (one request per settle instead of one per keystroke), and the field catalog
  is fetched once against a shared key (no more `ColumnPicker` empty-list flash).

### Security

- **Constant-time ingest-token comparison** (`subtle::ConstantTimeEq`).
- **Token is checked before the in-flight permit** is acquired in every receiver, so an
  unauthenticated flood can't starve backpressure permits.
- **Query DoS closed.** Shared bucket/limit clamps (`MAX_BUCKETS=3000`, `MAX_LIMIT=1000`)
  are applied at every API handler and inside every engine method, closing the
  `?buckets=2e9` ~16 GB OOM vector.

### Performance

- **Single-pass search + count.** `/api/search` and `/api/spans/search` no longer prune
  the manifest/skip-indexes and re-open every surviving Parquet file twice — the row
  fetch and the match count share one query. Output-identical envelope.
- **Late materialization for all span sorts.** Slowest/Errors join Recent in decoding the
  wide `attributes` map only for surviving rows.
- **Streaming OTLP mappers.** Traces and metrics now stream straight into their Arrow
  builders, dropping the intermediate `Vec`/`BTreeMap` allocations (~2× transient memory),
  proven byte-identical to the reference path.
- **Bounded, off-runtime compaction.** Merge passes are bounded per pass (with a carry set
  so no entry is ever dropped) and Parquet decode runs on the blocking pool.
- **Narrowed trace-search hydration.** `search_traces` hydrates each kept trace's
  `min(start) ± 1h` and ranks in DataFusion instead of a full-window rescan (also fixes a
  window-straddle undercount).
- **Cached metric probe metadata** per engine (manifest-pointer invalidated), dropping a
  redundant prune + Parquet open per chart panel.

[1.7.0]: https://github.com/nevindra/photon/compare/v1.6.0...v1.7.0
[1.6.0]: https://github.com/nevindra/photon/compare/v1.5.0...v1.6.0
[1.5.0]: https://github.com/nevindra/photon/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/nevindra/photon/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/nevindra/photon/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/nevindra/photon/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/nevindra/photon/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/nevindra/photon/releases/tag/v1.0.0

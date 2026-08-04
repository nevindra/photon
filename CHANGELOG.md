# Changelog

All notable changes to Photon are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[1.5.0]: https://github.com/nevindra/photon/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/nevindra/photon/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/nevindra/photon/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/nevindra/photon/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/nevindra/photon/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/nevindra/photon/releases/tag/v1.0.0

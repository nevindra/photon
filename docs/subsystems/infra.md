# Infrastructure (host/GPU resource monitoring)

First-class host and NVIDIA GPU resource monitoring: a standalone `photon-agent` binary pushes OTLP
`system.*` metrics tagged with a promoted `host.name`, and a curated `/api/infra/*` vertical +
dedicated `/infra` UI page surface them. **No new storage engine** — resource metrics are ordinary
gauge/sum series that ride the existing metrics WAL → `MetricsCompactor` → `MetricsQueryEngine`
(see [`metrics.md`](metrics.md)); this doc covers what's specific to the infra vertical: the agent,
the `host.name` prunable dimension, and the curated query/API/UI on top.

> Shared plumbing and invariants: [`../architecture.md`](../architecture.md).

## The agent (`photon-agent`)

A standalone binary (its own workspace member, not compiled by `cargo build -p photon-server`,
mirroring `photon-loadgen`'s standalone-OTLP-client shape) that samples the local host on a fixed
interval and POSTs OTLP/HTTP protobuf metrics to Photon's `/v1/metrics`:

```bash
cargo run -p photon-agent -- --endpoint http://127.0.0.1:4318/v1/metrics
```

**Files** (`crates/photon-agent/src/`): `config.rs` (CLI/env config), `sample.rs` (signal-agnostic
`MetricSample`/`ResourceSample`/`Sampler` trait), `sysinfo_sampler.rs` (host CPU/RAM/disk/network via
`sysinfo` 0.33), `gpu.rs` (`GpuSampler` trait; `NvmlGpu` behind the default-on `gpu` feature, `NoGpu`
fallback), `otlp.rs` (`ResourceSample` → `ExportMetricsServiceRequest`, resource attrs + per-metric
data points), `send.rs` (the sender loop: sample → POST, bearer auth, `application/x-protobuf`),
`main.rs` (wires `clap`-parsed `AgentConfig` into `send::run`).

**Config** (`crates/photon-agent/src/config.rs`, CLI flags with `env` fallbacks via `clap`):

| Flag | Env var | Default | Purpose |
|---|---|---|---|
| `--endpoint` | `PHOTON_AGENT_ENDPOINT` | `http://127.0.0.1:4318/v1/metrics` | OTLP/HTTP metrics endpoint |
| `--token` | `PHOTON_INGEST_TOKEN` | `dev-ingest-token` | ingest bearer token (must match `[ingest].token`) |
| `--host-name` | `PHOTON_AGENT_HOST` | OS hostname (`sysinfo::System::host_name()`) | reported `host.name` |
| `--interval-secs` | `PHOTON_AGENT_INTERVAL` | `15` | seconds between samples |
| `--no-gpu` | `PHOTON_AGENT_NO_GPU` | `false` | disable GPU sampling even when built with the `gpu` feature |

**GPU sampling** (`gpu.rs`): NVML loads dynamically via `nvml-wrapper` (`libloading`, no link-time
driver dependency), so the agent compiles and runs fine on any host, including one with no NVIDIA
driver (e.g. a macOS dev machine) — `Nvml::init()` simply fails at runtime and the agent falls back
to `NoGpu` (logging once), never refusing to start.

**Every emitted point carries resource attributes `host.name`, `host.id` (the OS hostname), and
`os.type`** (`std::env::consts::OS`), set on the OTLP `Resource`; GPU points additionally carry
`gpu` (device index) and `gpu.name` as **data-point** attributes.

### Metrics emitted (OTel system semantic conventions)

| Metric | Kind | Unit | Data-point attrs | Notes |
|---|---|---|---|---|
| `system.cpu.utilization` | Gauge | `1` | `cpu` = `total` \| core index | one aggregate point + one per logical core |
| `system.cpu.logical.count` | Gauge | `{cpu}` | — | logical core count |
| `system.cpu.load_average.1m` | Gauge | `1` | — | 1-minute load average |
| `system.memory.utilization` | Gauge | `1` | — | used/total |
| `system.memory.usage` | Gauge | `By` | `state` = `used` \| `free` | |
| `system.memory.limit` | Gauge | `By` | — | total RAM |
| `system.filesystem.utilization` | Gauge | `1` | `mountpoint` | per mounted filesystem |
| `system.filesystem.usage` | Gauge | `By` | `mountpoint`, `state=used` | |
| `system.network.io` | Sum (monotonic, cumulative) | `By` | `device`, `direction` = `receive` \| `transmit` | reset-aware rate() applies |
| `system.gpu.utilization` | Gauge | `1` | `gpu`, `gpu.name` | NVML `utilization_rates().gpu` |
| `system.gpu.memory.usage` | Gauge | `By` | `gpu`, `gpu.name` | |
| `system.gpu.memory.utilization` | Gauge | `1` | `gpu`, `gpu.name` | used/total |
| `system.gpu.temperature` | Gauge | `Cel` | `gpu`, `gpu.name` | |
| `system.gpu.power` | Gauge | `W` | `gpu`, `gpu.name` | NVML reports milliwatts; the agent divides by 1000 |

`system.disk.io` (per-device disk read/write bytes, `SUM` monotonic) is in the design's Global
Constants list but is **not yet emitted** by `sysinfo_sampler.rs` — only filesystem usage/
utilization are.

## The host model: `host.name` as a prunable dimension

`host.name` was already a promoted Arrow column (`photon.example.toml`'s
`[schema].promoted_attributes`); this feature makes it **prunable**, the same way `service.name`
already was, without adding a new storage engine:

- **Compactor sort key** (`crates/photon-compact/src/metrics_compactor.rs`, `sort_metrics`) is now
  `(metric_name, service.name, host.name, timestamp)` — `host.name` appended after `service.name`,
  preserving existing ordering for single-host/app-metric data. The compactor's lexsort order *is*
  the query engine's pruning contract (see [`../architecture.md`](../architecture.md)).
- **Metrics skip index** (`crates/photon-index/src/skip_index.rs`) gains a `host_range: Option<(String,
  String)>` field: the inclusive min/max of the promoted `host.name` column per compacted file, built
  by `SkipIndex::build_metrics` and read back via `host_range()`. Logs and spans skip indexes always
  set `host_range: None` (they don't range over host).
- **Binary sidecar format bumps `1 → 2`** (`idx_binary` in `skip_index.rs`) to carry the host block
  after the service block. `decode` stays backward compatible: a v1 sidecar (written before this
  feature) has no host block, so `host_range` defaults to `None` rather than erroring.
- **Pruning** (`crates/photon-query/src/metric_engine.rs`): `MetricRequest.host: Option<String>` flows
  into `keep_candidate`, which drops a candidate file only when the requested host is **provably
  outside** `[lo, hi]`. Consistent with the load-bearing "no inverted index, never false-negative"
  invariant: a missing `.idx` or an unknown host range always **keeps** the file — pruning can only
  add work, never drop a real result.

## Curated query (`photon-query/src/infra.rs`)

`impl MetricsQueryEngine` methods, all built on the metrics engine's existing `survivors_df` +
`metric_base_predicate` pruning/predicate path (no new storage engine, no new schema):

- **`infra_hosts(start_ns, end_ns) -> Vec<HostSummary>`** — distinct hosts + latest headline vitals.
  Hosts are enumerated from `system.cpu.utilization` (every agent reports it); a host with no CPU
  points in the window doesn't appear. `system.memory.utilization` fills `mem_util`; presence of any
  `system.gpu.utilization` row sets `has_gpu`. `HostSummary { host, cpu_util, mem_util, disk_util +
  disk_util_avg + disk_groups, gpu_util + gpu_util_avg + gpu_groups, last_seen_ns, has_gpu }`.

  **Split resources cross as a pair.** Disk and GPU are label-split (mountpoints, devices), so
  `fill_group_gauge` groups by `(host.name, get_field(attributes, <mountpoint|gpu>))`, takes each
  group's window-avg, and folds **both** the MAX and the MEAN of those per-group values into the
  summary, plus the group count. Neither number is sufficient alone: the max is what keeps a full
  `/` from being averaged away against an idle `/boot/efi`, and the mean is what stops that max
  from reading as a saturated host when one group is hot and the rest idle (a 4-GPU node with one
  card at 76% and three near zero is a 20%-average host). The UI renders them side by side —
  see the `HostStatTiles.vue`/`HostCard.vue` notes below.
- **`infra_host_detail(host, start_ns, end_ns) -> HostDetail`** — per-host metadata: latest
  `system.cpu.logical.count` (→ `cores`), `system.memory.limit` (→ `total_ram_bytes`), the latest
  `os.type` long-tail attribute (→ `os`, read via `get_field` since it's not promoted), and the
  distinct `gpu.name` values seen (→ `gpus`). `last_seen_ns` is derived from `system.cpu.utilization`
  (the same canonical always-present metric `infra_hosts` uses for its last-seen), not from the
  core-count/mem-limit metrics, so host-detail and the host list always agree. Every read is
  host-scoped (`col_ref(HOST_ATTR).eq(lit(host))` plus `MetricRequest.host`), so it both prunes files
  via the skip-index host range and filters rows.
- **`infra_host_series(host, resource, start_ns, end_ns, buckets) -> HostSeries`** — one curated
  bucketed timeseries per resource panel, delegating to the general `query_series` with a compiled
  `host.name:<host>` filter (`host_filter`, built through `MetricFieldResolver` so it resolves to the
  same `Attr("host.name")` shape the skip-index host pruning expects). `InfraResource::primary()` maps
  each panel to its headline metric + breakdown attribute:

  | Resource | Metric | Group-by attribute |
  |---|---|---|
  | `cpu` | `system.cpu.utilization` | `cpu` |
  | `memory` | `system.memory.utilization` | `host.name` |
  | `disk` | `system.filesystem.utilization` | `mountpoint` |
  | `network` | `system.network.io` | `direction` |
  | `gpu` | `system.gpu.utilization` | `gpu` |
  | `gpu_memory` | `system.gpu.memory.utilization` | `gpu` |
  | `gpu_temp` | `system.gpu.temperature` | `gpu` |
  | `gpu_power` | `system.gpu.power` | `gpu` |
  | `load` | `system.cpu.load_average.1m` | `host.name` |

  `system.network.io` is a monotonic cumulative Sum, so `query_series` (no `agg` override) picks its
  default aggregation for a monotonic Sum — reset-aware `rate()` — meaning the network panel's series
  are bytes/sec, not a raw cumulative counter; the UI labels it `By/s` accordingly
  (`HostResourcePanels.vue`).

## API

| Route | Purpose |
|---|---|
| `GET /api/infra/hosts?start=<ns>&end=<ns>` | distinct hosts + latest CPU/memory/disk/GPU utilization vitals (`cpuUtil`/`memUtil`/`diskUtil`/`gpuUtil`, nullable fractions) + `hasGpu`. `diskUtil`/`gpuUtil` are the WORST mountpoint/GPU, and they never travel alone: `diskUtilAvg`/`gpuUtilAvg` carry the mean across groups and `diskGroups`/`gpuGroups` the group count, so a client can render "85% / 30%" instead of a bare max |
| `GET /api/infra/hosts/:host?start=<ns>&end=<ns>` | one host's metadata (OS, cores, RAM, GPU names, last-seen) |
| `GET /api/infra/hosts/:host/processes?start=<ns>&end=<ns>` | the supervised processes running on one host + their latest resource usage (`process`, `cpuPct` (a percent, not a fraction), `rssBytes`, `fds`, `threads`, `restarts` — all nullable; `lastSeenNs`). Processes are the distinct `service.name` values among `process.*` metrics scoped to the host, enumerated from the CPU metric, and capped at the top 200 by CPU. Powers the per-host Processes table |
| `GET /api/infra/hosts/:host/timeseries?resource=cpu\|memory\|disk\|network\|gpu\|gpu_memory\|gpu_temp\|gpu_power\|load&start=<ns>&end=<ns>&buckets=<n>` | curated bucketed series for one resource panel (`buckets` optional, default 48, clamped 1–500) |

### Per-host processes (`infra_host_processes`)

A process row is **one `service.name` on the host**: two instances of the same service on a host
collapse into a single row and their gauges average together. (OTel would key finer on
`process.pid` / `process.executable.name`; that finer identity is out of scope for this vertical.)
The query enumerates processes from the CPU metric and caps the result at the top 200 by CPU
(`PROC_ROW_CAP`), matching the repo's bounded-table convention.

Metric names follow the OTel process semantic conventions as the **primary** contract, with the
original bespoke names kept as a **fallback** so both OTel-Collector producers and pre-semconv
producers work. A producer uses one scheme consistently, so the query picks the scheme from whichever
CPU metric enumerates processes and reads every other gauge in that same scheme:

| field | semconv (primary) | bespoke (fallback) | aggregation |
|---|---|---|---|
| cpu | `process.cpu.utilization` (0..1 → ×100 for the percent shown) | `process.cpu.percent` (already 0..100) | avg |
| memory | `process.memory.usage` (bytes) | `process.memory.rss` | avg |
| fds | `process.unix.file_descriptor.count` | `process.open_fds` | avg |
| threads | `process.thread.count` | `process.threads` | avg |
| restarts | *(no OTel semconv — Photon-specific extension)* | `process.restarts` | **max** |

`process.restarts` has **no OTel semconv equivalent** — it is a Photon-specific extension emitted by
supervised-process producers (e.g. a process supervisor like mandor). It is a cumulative counter, so
it is aggregated with `max` (last value), not `avg`: averaging a 0→1→3 series would render a nonsense
"1.3". All other gauges are window-averaged.

Handler: `crates/photon-api/src/infra.rs`, registered in `crates/photon-api/src/lib.rs` alongside
`/api/metrics/*`, behind the same session auth (`require_auth`) as the rest of the authenticated API.
Timestamps cross the wire as decimal-nanosecond strings (JS-safe), mirroring `metrics.rs`'s
`series_json`: `lastSeenNs` and each series point's `t`.

## UI

**Routes:** `/infra` (`InfraHostsView.vue`) and `/infra/:host` (`InfraHostDetailView.vue`), declared
in `router/index.js` with the static `/infra` before the dynamic `/infra/:host` (same ordering
convention as the RUM sub-routes).

- **`InfraHostsView.vue`** — the host list: `useInfraHosts` polled every 15s, rendered as a fleet
  KPI band (`HostFleetKpis.vue`) above a card grid (`HostCard.vue`, one per host); card click
  navigates to `/infra/:host`. Empty state ("Run photon-agent on a host…") when no hosts report.
  - **`HostFleetKpis.vue`** — one `StatTile` row derived client-side from the host list (no extra
    query): Hosts (count), Warning/Critical (counts, tinted `warning`/`error` when >0), Avg CPU
    (mean of non-null `cpuUtil`), GPU hosts (count of `hasGpu`). Per-host status is the worst of
    `cpuUtil`/`memUtil`/`diskUtil`/`gpuUtil` via the new `hostStatus` helper
    (`lib/infra/hostStats.ts`) — a critical host counts only toward Critical, never double-counted
    as Warning.
  - **`HostCard.vue`** — one card per host: name + a small `⚠ <RESOURCE>` flag naming the single
    worst-degraded resource, labeled `Meter` rows for CPU/MEM/DSK (a null value skips its row) plus
    a GPU row when `gpuUtil` is present, and a relative last-seen footer. A warn/error border tint
    mirrors the flag's severity. The **DSK/GPU** rows read `85% / 30%` — worst group over the mean
    across groups, from the API pair above — dropping to a single number when the resource has one
    group (the mean would just repeat the max). CPU/MEM are never split, so they stay single. No GPU device names (not in this API — see `HostStatTiles.vue`
    below for those). `HostTable.vue` is removed (dead code); its coverage moved to
    `HostCard.test.ts` and `hostStats.test.ts`.
- **`InfraHostDetailView.vue`** — host header (OS/cores/RAM/GPU names) + a glance stat-tile row
  (`HostStatTiles.vue`) + `HostResourcePanels.vue`'s per-resource trend sections, both driven off one
  `useHostResourceSeries(host, startNs, endNs, hasGpu)` call hoisted in the view — a bundle of nine
  `useInfraHostSeries` queries (`cpu`, `memory`, `disk`, `network`, `load`, `gpu`, `gpu_memory`,
  `gpu_temp`, `gpu_power`; the four GPU ones additionally gated on `hasGpu` so they never fire for a
  GPU-less host) passed down as one `res` prop to both children, so a tile and its section chart
  always read the same query-cache entry.
  - **`HostStatTiles.vue`** — a responsive tile grid (CPU · Memory with a used/total-bytes sub-label ·
    worst-mountpoint Disk · Network combined rate · GPU util + GPU temp when `hasGpu`), each value
    derived from the **last point** of the shared series via pure helpers in
    `lib/infra/hostStats.ts` (`latestValue`/`latestTotal`/`latestMean`/`worstSeries`/`groupStat`/
    `sparkValues`, table-tested in `HostStatTiles.test.ts`/`hostStats.test.ts`). Utilization tiles
    tint `warning`/`error` at the shared **80%/90%** thresholds (`utilAccent`); the CPU and Memory
    tiles additionally render a `Sparkline` off the same series through `StatTile`'s `#spark` slot.

    The **Disk/GPU/GPU-temp** tiles are the split resources, and they show the same max·avg pair the
    API sends for the host list — derived client-side by `groupStat()` from the very series the
    panels below chart, so a tile and its chart can never disagree:

    ```
    Disk                     GPU                     GPU temp
    85% max · 30% avg        76% max · 20% avg       74°C max · 52°C avg
    /data of 5 mounts        gpu 2 of 4              gpu 2 of 4
    ```

    The big number stays the **worst** group (`StatTile`'s `value`, and what `utilAccent` tints);
    the mean rides beside it in `StatTile`'s `secondary` slot, and the `sub` slot names which group
    the max came from and out of how many. With a single mountpoint or GPU the pair collapses to a
    plain reading — the mean would only repeat the max. CPU/Memory/Network are not split and never
    show a pair.
  - **`HostResourcePanels.vue`** — now purely presentational (no queries of its own; reads `res`);
    each chart lives in a titled `charts/ChartPanel` card: a **CPU** card with a `Segmented`
    total/per-core toggle in its `#summary` slot (`cpuSeriesForMode`, a client-side label filter over
    the same already-fetched series — defaults to `total`) next to its **load-average (1m)** card; a
    2-column **Memory** + **Network I/O** row; a **Disk** card listing per-mountpoint `Meter`s
    worst-first above the trend chart; and, only when `hasGpu`, a 4-card **GPU** section
    (Utilization/Memory/Temperature/Power), the device name(s) carried in the utilization card's
    subtitle. Every split card's subtitle states the split — `5 mountpoints`, `4 devices`
    (`NVIDIA A2 · 4 devices` on the utilization card) — so the max·avg pair on the tiles above has
    something visible to point at instead of leaving the reader to count legend entries.
  Below the trend panels it renders the **`HostProcessesTable`** component — every supervised process
  on the host, one row per `service.name`, driven by `useInfraHostProcesses(host, startNs, endNs)`
  (`GET /api/infra/hosts/:host/processes`, 15s poll). Built on the shared `ui/table` primitives with
  columns Process, CPU %, RSS (human-readable via `formatBytes`), FDs, Threads, Restarts; all header
  cells are click-to-sort (lucide `ArrowUp`/`ArrowDown` indicator), defaulting to CPU descending so
  the heaviest process is on top (nulls sort last, rendered as `—`), with an `EmptyState` when no
  process is reporting. Covered by `InfraHostDetailView.test.ts`.
  On mount, sets the global scope to `{ type: 'host', id: host, label: host }` via `lib/core/context.ts`'s
  `setScope`, so the time range + host scope carry through `AppShell`'s `ContextBar` and the
  "Related ▾" menu (`RelatedMenu`) the same way a service or RUM app scope would.
- **Components** (`frontend/src/components/infra/`): `HostFleetKpis.vue`, `HostCard.vue`,
  `HostStatTiles.vue`, `HostResourcePanels.vue`, `HostProcessesTable.vue` — together reuse existing
  primitives (`ui/card`, `ui/meter`, `ui/segmented`, `ui/sparkline`, `ui/stat-tile`, `ui/table`,
  `ui/empty-state`, `charts/ChartPanel`, `components/metrics/MetricChart.vue`), no bespoke chart code.
- **Queries** (`frontend/src/lib/infra/infraQueries.ts`): `useInfraHosts`, `useInfraHost`,
  `useInfraHostSeries` (one resource at a time) and `useHostResourceSeries` (the nine-query bundle
  above, hoisted once per host-detail view so its children share one cache read) — same TanStack
  Query contract as the other per-signal query modules (reactive inputs normalized with `toValue`
  into a computed `queryKey`, `keepPreviousData`, 15s polling for the two live views).
- **NavRail:** the "Infrastructure" world now has two items — **Hosts** (`/infra`, `Server` icon) and
  **Ops** (`/uptime`, `Activity` icon) — instead of a single landing item into `/uptime`.
  `AppShell`'s `ROUTE_GROUP`/`LANDING` maps route `infra` → nav-group `infra` → landing `/infra`.
- **Correlation:** `lib/core/useCorrelate.ts` adds `'host'` to `EntityKind` and a `case 'host'` in
  `candidates()` — "Related ▾" from a host offers Logs (`host.name:<host>` query), Traces
  (`host.name:<host>`, sorted slowest-first), and Metrics (plain `/metrics`).

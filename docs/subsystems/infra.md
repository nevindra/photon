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
  `system.gpu.utilization` row sets `has_gpu`. `HostSummary { host, cpu_util, mem_util, last_seen_ns,
  has_gpu }`.
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

  `system.network.io` is a monotonic cumulative Sum, so `query_series` (no `agg` override) picks its
  default aggregation for a monotonic Sum — reset-aware `rate()` — meaning the network panel's series
  are bytes/sec, not a raw cumulative counter; the UI labels it `By/s` accordingly
  (`HostResourcePanels.vue`).

## API

| Route | Purpose |
|---|---|
| `GET /api/infra/hosts?start=<ns>&end=<ns>` | distinct hosts + latest CPU/memory/GPU-presence vitals |
| `GET /api/infra/hosts/:host?start=<ns>&end=<ns>` | one host's metadata (OS, cores, RAM, GPU names, last-seen) |
| `GET /api/infra/hosts/:host/timeseries?resource=cpu\|memory\|disk\|network\|gpu&start=<ns>&end=<ns>&buckets=<n>` | curated bucketed series for one resource panel (`buckets` optional, default 48, clamped 1–500) |

Handler: `crates/photon-api/src/infra.rs`, registered in `crates/photon-api/src/lib.rs` alongside
`/api/metrics/*`, behind the same session auth (`require_auth`) as the rest of the authenticated API.
Timestamps cross the wire as decimal-nanosecond strings (JS-safe), mirroring `metrics.rs`'s
`series_json`: `lastSeenNs` and each series point's `t`.

## UI

**Routes:** `/infra` (`InfraHostsView.vue`) and `/infra/:host` (`InfraHostDetailView.vue`), declared
in `router/index.js` with the static `/infra` before the dynamic `/infra/:host` (same ordering
convention as the RUM sub-routes).

- **`InfraHostsView.vue`** — the host list: `useInfraHosts` polled every 15s, rendered via
  `HostTable.vue` (one row per host — meter + percentage for CPU/memory, a GPU yes/blank flag);
  row click navigates to `/infra/:host`. Empty state ("Run photon-agent on a host…") when no hosts
  report.
- **`InfraHostDetailView.vue`** — host header (OS/cores/RAM/GPU names) + `HostResourcePanels.vue`
  (one `MetricChart` per resource — CPU/Memory/Disk/Network always render, GPU only when
  `hasGpu`; each panel is its own `useInfraHostSeries` query so panels load independently). On
  mount, sets the global scope to `{ type: 'host', id: host, label: host }` via `lib/core/context.ts`'s
  `setScope`, so the time range + host scope carry through `AppShell`'s `ContextBar` and the
  "Related ▾" menu (`RelatedMenu`) the same way a service or RUM app scope would.
- **Components** (`frontend/src/components/infra/`): `HostTable.vue`, `HostResourcePanels.vue` —
  both reuse existing primitives (`ui/table`, `ui/meter`, `components/metrics/MetricChart.vue`), no
  bespoke chart code.
- **Queries** (`frontend/src/lib/infra/infraQueries.ts`): `useInfraHosts`, `useInfraHost`,
  `useInfraHostSeries` — same TanStack Query contract as the other per-signal query modules
  (reactive inputs normalized with `toValue` into a computed `queryKey`, `keepPreviousData`,
  15s polling for the two live views).
- **NavRail:** the "Infrastructure" world now has two items — **Hosts** (`/infra`, `Server` icon) and
  **Ops** (`/uptime`, `Activity` icon) — instead of a single landing item into `/uptime`.
  `AppShell`'s `ROUTE_GROUP`/`LANDING` maps route `infra` → nav-group `infra` → landing `/infra`.
- **Correlation:** `lib/core/useCorrelate.ts` adds `'host'` to `EntityKind` and a `case 'host'` in
  `candidates()` — "Related ▾" from a host offers Logs (`host.name:<host>` query), Traces
  (`host.name:<host>`, sorted slowest-first), and Metrics (plain `/metrics`).

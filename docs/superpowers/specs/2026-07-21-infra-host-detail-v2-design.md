# Infra host detail v2 — design

**Date:** 2026-07-21
**Status:** approved (brainstorm w/ visual companion; layout option B chosen)
**Scope:** backend (`photon-query`, `photon-api`) + frontend (`/infra/:host`) + docs

## Problem

The `/infra/:host` page renders raw series with no glance layer and several presentation
defects (screenshot-verified):

- CPU draws 17 lines (16 cores + total) — unreadable spaghetti with a multi-row legend.
- Utilization y-axes auto-scale to the data (memory shown as 0.468–0.478 raw fraction)
  instead of a fixed 0–100 % scale.
- Network y-axis labels render clipped as "00 By/s" (axis formatting/width defect in the
  charts layer).
- No "current value" layer — you can't tell at a glance whether the host is healthy.
- The agent already emits GPU memory/temperature/power and load average, but the curated
  API only exposes 5 resources (cpu/memory/disk/network/gpu-util), so the UI can't show
  them.

## Goals

Two-layer experience: **glance** (current-state tiles) on top, **trend** (full
time-series charts) below. Non-negotiable: every resource keeps a full time-series chart
bound to the global time range — the tiles/meters layer is additive, never a replacement
for trend tracking.

## Non-goals (YAGNI, explicitly cut)

- Per-device network breakdown (direction rx/tx split is the headline).
- A memory used/free **bytes** series (absolute GB on the tile is derived client-side
  from utilization × `totalRamBytes`, already in the host-detail response).
- Core heatmap, multi-tab layout (options considered and rejected in brainstorm).
- New storage engine / schema changes — everything below is read-path only.

## 1. Backend — new curated resources

`InfraResource` (`crates/photon-query/src/infra.rs`) gains 4 variants, wired through the
existing `primary()` metric+group-by mapping:

| Resource param | Metric | Group-by attr | Unit |
|---|---|---|---|
| `gpu_memory` | `system.gpu.memory.utilization` | `gpu` | `1` (fraction) |
| `gpu_temp` | `system.gpu.temperature` | `gpu` | `Cel` |
| `gpu_power` | `system.gpu.power` | `gpu` | `W` |
| `load` | `system.cpu.load_average.1m` | `host.name` | `1` (absolute, NOT a %) |

`GET /api/infra/hosts/:host/timeseries?resource=` (`crates/photon-api/src/infra.rs`)
accepts the new values. Everything rides the existing `infra_host_series` path
(host-scoped filter + skip-index host pruning); no new endpoints.

## 2. Frontend — charts-layer fixes (shared, not per-view)

- **Percent mode**: utilization series (0–1 fractions) render ×100 on a **fixed 0–100 %**
  y-axis. Exposed as a chart-layer option (e.g. `unit="%"` handling in
  `MetricChart.vue` → a fixed-range knob on `LineChart`/BaseChart options builder), so
  memory can no longer auto-zoom to 0.468–0.478.
- **Byte-rate axis**: compact unit-aware axis labels (`2.1 MB/s`, via a
  `formatBytes`-based rate formatter) — and root-cause the "00 By/s" clipping in the axis
  size/formatting code (systematic-debugging during implementation; fix belongs in
  `components/charts/`, not the infra view).
- **Legend cap**: legends must not wrap into a multi-row block (cap/collapse behavior in
  the shared chart legend).

## 3. Host detail page — layout B (tiles + section per resource)

All in `frontend/src/` (view `InfraHostDetailView.vue`, components under
`components/infra/`):

- **`HostStatTiles.vue` (new)** — one row of current-state tiles: CPU %, Memory %
  (+ `14.5/30.3 GB` derived from `totalRamBytes`), Disk (worst mountpoint %), Net ⇅
  (current combined rate), GPU %, GPU temp (GPU tiles only when `hasGpu`). Values come
  from the **last non-null point of the already-fetched series** — zero extra API calls.
  Each tile has a mini sparkline (existing `LineChart` `compact` mode). Threshold tint:
  ≥80 % warn, ≥90 % error (existing `sev-warn`/`sev-error` tones); applies to
  percent-natured tiles only (not net/temp).
- **CPU section** — utilization chart defaulting to `cpu=total` only, with a `Segmented`
  toggle `total | per-core` (client-side filter of the already-grouped series; no new
  query), plus a small load-average (1m) chart beside it. Load average is charted as an
  absolute value (can exceed core count), never as a percent.
- **Memory + Network** — side by side, 2 columns (memory in %, network in rate units).
- **Disk section** — per-mountpoint meters (existing `ui/meter`) for current usage +
  the utilization trend chart.
- **GPU section** (only when `hasGpu`) — heading carries the GPU name(s); 4 charts:
  utilization %, memory %, temperature °C, power W. New `useInfraHostSeries` calls for
  the 3 new resources, gated on `hasGpu` like the existing gpu query.

Every section chart stays bound to the global time range/context (`startNs`/`endNs` from
`lib/core/context.ts`) with the existing 15 s polling — trend tracking over any window is
preserved everywhere.

## 4. Docs

Same-change updates: `docs/subsystems/infra.md` (new resources, new page structure) and
the `/api/infra` route line in `docs/architecture.md` (+ CLAUDE.md route list if
affected).

## Increment 2 (2026-07-21, post-v2): `/infra` hosts list executive summary

User follow-up after v2 shipped: the hosts list is "just a table" — they want to see many
hosts at once without clicking into each. Chosen layout (visual-companion option A): **fleet
KPI band + host cards**.

- **Backend**: `HostSummary` gains `disk_util: Option<f64>` (the WORST mountpoint: max over
  per-mountpoint window-avg of `system.filesystem.utilization` — NOT a plain per-host avg,
  which would dilute a full disk with an empty one) and `gpu_util: Option<f64>` (max over
  per-gpu window-avg of `system.gpu.utilization`). `/api/infra/hosts` JSON adds `diskUtil`,
  `gpuUtil`. No new endpoints.
- **Frontend `/infra`**: fleet KPI band (StatTile row: total hosts, warning count, critical
  count, avg CPU, GPU hosts — derived client-side from the host list; warn/crit = any of
  cpu/mem/disk/gpu util at the shared ≥0.8/≥0.9 thresholds) above a **host card grid
  replacing `HostTable`** (card per host: name, warn/error border tint + worst-resource flag,
  CPU/MEM/DSK meters + GPU meter when present, last-seen; click → `/infra/:host`). `HostTable`
  is removed (dead code) — its test coverage moves to the new components.
- Sparklines on cards explicitly deferred (option C rejected for now — would need a batch
  timeseries endpoint).

## 5. Testing

- **Rust**: unit tests for the 4 new `InfraResource` mappings + resource-param parsing
  (reject unknown values as today).
- **Frontend (vitest)**: tile derivation (last-point extraction, worst-mountpoint pick,
  GB derivation, threshold tint), per-core filter toggle, percent/byte-rate formatters.
- **Gates**: `cargo test -p photon-query -p photon-api`, `bun run test`,
  `bun run type-check`.
- **Visual**: verify against the live local Docker Photon + running `photon-agent`
  (real 16-core + RTX 4070 SUPER host).

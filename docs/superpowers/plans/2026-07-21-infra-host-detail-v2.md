# Infra Host Detail v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild `/infra/:host` as a two-layer monitoring page (glance tiles + per-resource trend sections) with 4 new curated backend resources and shared charts-layer fixes (percent axes, byte-rate labels, legend cap).

**Architecture:** Backend adds 4 variants to the existing `InfraResource` enum (pure mapping change — no new storage, no new endpoints). Frontend fixes land in the shared charts layer (`chartOptions.js`/`MetricChart.vue`/`BaseChart.vue`), then the infra view hoists its series queries into one composable feeding both a new `HostStatTiles` component and a restructured `HostResourcePanels`.

**Tech Stack:** Rust (axum/DataFusion via existing engines), Vue 3 `<script setup>`, TanStack Query, uPlot, existing `ui/` primitives (StatTile, Sparkline, Segmented, Meter).

**Spec:** `docs/superpowers/specs/2026-07-21-infra-host-detail-v2-design.md`

## Global Constraints

- **NO `git commit` anywhere** — user preference overrides the usual commit steps. Stage with `git add` at the end of each task and leave the working tree dirty.
- Package manager is **bun**, never npm. Frontend tests: `cd frontend && bun run test`; types: `bun run type-check`.
- New `.vue`/`.ts` files use `<script setup lang="ts">` (gated by `vue-tsc`).
- Do not bump any co-pinned dependency (`arrow`/`datafusion`/`parquet`/`object_store`, `opentelemetry-proto`/`tonic`/`prost`).
- Charts-layer changes must keep every existing chart consumer working — run the full frontend test suite, not just new tests.
- Load average is charted/labelled as an **absolute value** (can exceed core count), never a percent.
- Utilization fractions are `0–1` on the wire; ×100 happens **only** in the charts layer percent mode.
- Docs (`docs/subsystems/infra.md`, `docs/architecture.md`) update in the same change (Task 5).

---

### Task 1: Backend — 4 new curated resources

**Files:**
- Modify: `crates/photon-query/src/infra.rs` (constants ~line 36; `InfraResource` enum + impl ~lines 84–126; fixture `two_hosts_cpu` ~line 635; tests module)
- Modify: `crates/photon-api/src/infra.rs` (tests only — the handler parses via `InfraResource::from_str`, no handler change)

**Interfaces:**
- Produces: `InfraResource::{GpuMemory, GpuTemp, GpuPower, Load}`; API accepts `resource=gpu_memory|gpu_temp|gpu_power|load` on `GET /api/infra/hosts/:host/timeseries`. Response shape unchanged (`{ resource, series }`). Frontend (Task 3) calls these exact resource strings.

- [ ] **Step 1: Extend the test fixture with the new metrics**

In `crates/photon-query/src/infra.rs`, add constants next to the existing ones (~line 36):

```rust
const GPU_MEM_UTIL: &str = "system.gpu.memory.utilization";
const GPU_TEMP: &str = "system.gpu.temperature";
const GPU_POWER: &str = "system.gpu.power";
const LOAD_1M: &str = "system.cpu.load_average.1m";
```

In `tests_fixture::two_hosts_cpu`, append these points to the existing batch slice (after the `GPU_UTIL` point for `web-1`):

```rust
mp(
    GPU_MEM_UTIL,
    "web-1",
    20,
    0.55,
    &[("gpu", "0"), ("gpu.name", "NVIDIA A100")],
),
mp(
    GPU_TEMP,
    "web-1",
    20,
    61.0,
    &[("gpu", "0"), ("gpu.name", "NVIDIA A100")],
),
mp(
    GPU_POWER,
    "web-1",
    20,
    180.0,
    &[("gpu", "0"), ("gpu.name", "NVIDIA A100")],
),
mp(LOAD_1M, "web-1", 20, 1.25, &[("os.type", "linux")]),
```

- [ ] **Step 2: Write the failing tests**

In the `tests` module of `crates/photon-query/src/infra.rs`:

```rust
#[test]
fn infra_resource_parses_the_new_resources() {
    assert_eq!(
        InfraResource::from_str("gpu_memory"),
        Some(InfraResource::GpuMemory)
    );
    assert_eq!(
        InfraResource::from_str("gpu_temp"),
        Some(InfraResource::GpuTemp)
    );
    assert_eq!(
        InfraResource::from_str("gpu_power"),
        Some(InfraResource::GpuPower)
    );
    assert_eq!(InfraResource::from_str("load"), Some(InfraResource::Load));
    assert_eq!(InfraResource::from_str("nope"), None);
}

#[tokio::test]
async fn infra_host_series_serves_the_new_gpu_and_load_resources() {
    let (_dir, engine) = super::tests_fixture::two_hosts_cpu().await;
    for (resource, name) in [
        (InfraResource::GpuMemory, "gpu_memory"),
        (InfraResource::GpuTemp, "gpu_temp"),
        (InfraResource::GpuPower, "gpu_power"),
        (InfraResource::Load, "load"),
    ] {
        let r = engine
            .infra_host_series("web-1", resource, 0, i64::MAX, 12)
            .await
            .unwrap();
        assert_eq!(r.resource, name);
        assert!(!r.series.is_empty(), "{name} series must not be empty");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p photon-query infra`
Expected: FAIL — `GpuMemory`/`GpuTemp`/`GpuPower`/`Load` variants don't exist (compile error).

- [ ] **Step 4: Implement the enum variants**

Extend `InfraResource` (update its doc comment from "The five curated resource panels" to "The curated resource panels"):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfraResource {
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    GpuMemory,
    GpuTemp,
    GpuPower,
    Load,
}
```

Add the arms (`from_str`, `as_str`, `primary`):

```rust
// from_str, after "gpu":
"gpu_memory" => Some(InfraResource::GpuMemory),
"gpu_temp" => Some(InfraResource::GpuTemp),
"gpu_power" => Some(InfraResource::GpuPower),
"load" => Some(InfraResource::Load),

// as_str:
InfraResource::GpuMemory => "gpu_memory",
InfraResource::GpuTemp => "gpu_temp",
InfraResource::GpuPower => "gpu_power",
InfraResource::Load => "load",

// primary:
InfraResource::GpuMemory => (GPU_MEM_UTIL, "gpu"),
InfraResource::GpuTemp => (GPU_TEMP, "gpu"),
InfraResource::GpuPower => (GPU_POWER, "gpu"),
InfraResource::Load => (LOAD_1M, HOST_ATTR),
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p photon-query infra`
Expected: PASS (all pre-existing infra tests still green).

- [ ] **Step 6: Add the API-level acceptance test**

In `crates/photon-api/src/infra.rs` tests module:

```rust
#[tokio::test]
async fn timeseries_accepts_the_new_resources() {
    use tower::ServiceExt;
    for resource in ["gpu_memory", "gpu_temp", "gpu_power", "load"] {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/api/infra/hosts/web-1/timeseries?resource={resource}&start=0&end=1"
                    ))
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "resource `{resource}`");
    }
}
```

- [ ] **Step 7: Run the API tests**

Run: `cargo test -p photon-api infra`
Expected: PASS.

- [ ] **Step 8: Stage (no commit)**

```bash
git add crates/photon-query/src/infra.rs crates/photon-api/src/infra.rs
```

---

### Task 2: Charts layer — percent mode, byte-rate axis, auto axis width, legend cap

**Files:**
- Modify: `frontend/src/lib/core/format.ts` (add `formatRate`)
- Modify: `frontend/src/components/charts/chartOptions.js` (`themedAxes` ~line 122; `buildLineOptions` ~line 171)
- Modify: `frontend/src/components/charts/LineChart.vue` (new `yRange` prop → builder)
- Modify: `frontend/src/components/metrics/MetricChart.vue` (unit-aware transform + formatters)
- Modify: `frontend/src/components/charts/BaseChart.vue` (legend row ~line 346)
- Test: `frontend/src/lib/core/format.test.ts` (create if absent), `frontend/src/components/charts/chartOptions.test.js`, `frontend/src/components/charts/BaseChart.test.js`

**Interfaces:**
- Consumes: nothing from Task 1 (independent — may run in parallel; disjoint files).
- Produces: `MetricChart` prop contract `unit="%"` = "input series are 0–1 fractions; render ×100 on a fixed 0–100 axis"; `unit="By/s"` = compact byte-rate labels. `LineChart` prop `yRange: [min, max] | null`. `formatRate(n): string` in `lib/core/format.ts`. Task 4 relies on all three.

- [ ] **Step 1: Write failing tests**

`frontend/src/lib/core/format.test.ts` (add to the file if it already exists):

```ts
import { describe, it, expect } from 'vitest'
import { formatRate } from './format'

describe('formatRate', () => {
  it('formats byte rates compactly', () => {
    expect(formatRate(512)).toBe('512 B/s')
    expect(formatRate(2_150_000)).toBe('2.1 MB/s')
  })
  it('dashes null/undefined/NaN', () => {
    expect(formatRate(null)).toBe('—')
    expect(formatRate(undefined)).toBe('—')
    expect(formatRate(Number.NaN)).toBe('—')
  })
})
```

In `frontend/src/components/charts/chartOptions.test.js`, add (mirror the existing tests' fake `uPlot`/theme setup already in that file):

```js
it('yRange pins the y scale to the given bounds', () => {
  const { opts } = buildLineOptions({
    uPlot: fakeUplot,
    series: [{ key: 'a', points: [{ t: 0, v: 0.4 }, { t: 60_000, v: 0.6 }] }],
    startMs: 0,
    endMs: 60_000,
    yRange: [0, 100],
  })
  expect(opts.scales.y.range()).toEqual([0, 100])
})

it('y axis size grows to fit the widest tick label', () => {
  const { opts } = buildLineOptions({
    uPlot: fakeUplot,
    series: [{ key: 'a', points: [{ t: 0, v: 1 }] }],
    startMs: 0,
    endMs: 60_000,
    theme: {},
  })
  const yAxis = opts.axes[1]
  const fakeU = { ctx: { measureText: (s) => ({ width: s.length * 7 }) } }
  // "12,000 By/s" (11 chars * 7px = 77px) must not be clamped to the 50px default.
  expect(yAxis.size(fakeU, ['12,000 By/s'], 1)).toBeGreaterThan(77 / (globalThis.devicePixelRatio || 1))
  expect(yAxis.size(fakeU, null, 1)).toBe(50)
})
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd frontend && bun run test -- format chartOptions`
Expected: FAIL — `formatRate` not exported; `yRange`/`size` undefined.

- [ ] **Step 3: Implement `formatRate`**

In `frontend/src/lib/core/format.ts`, after `formatBytes`:

```ts
// Bytes/second → compact rate label, e.g. 2_150_000 → "2.1 MB/s", 512 → "512 B/s".
export function formatRate(bytesPerSec: number | null | undefined): string {
  const label = formatBytes(bytesPerSec)
  return label === '—' ? label : `${label}/s`
}
```

- [ ] **Step 4: Implement `yRange` + y-axis auto-size in `chartOptions.js`**

`buildLineOptions` signature gains `yRange = null`. After the existing `if (yLog) {...}` block's sibling position (yLog wins if both are set — keep them mutually exclusive by ordering):

```js
// Fixed y bounds (e.g. [0,100] for percent charts). A range FUNCTION pins it, same as x.
// yLog takes precedence — a log scale supplies its own range.
if (yRange && !yLog) {
  opts.scales.y = { range: () => yRange }
}
```

In `themedAxes`, extend the y-axis entry:

```js
{
  stroke: t.axis,
  grid,
  ticks,
  values: (u, splits) => splits.map((v) => formatValue(v)),
  // Auto-size to the widest tick label. uPlot's default 50px axis clips long labels
  // ("12,000 By/s" → "00 By/s"); `values` here are the already-formatted strings and
  // ctx.measureText is in canvas px, so divide by pxRatio for CSS px.
  size: (u, values) => {
    if (!values) return 50
    const ratio = globalThis.devicePixelRatio || 1
    const w = Math.max(0, ...values.map((s) => u.ctx.measureText(String(s)).width / ratio))
    return Math.max(50, Math.ceil(w) + 14)
  },
},
```

- [ ] **Step 5: Thread `yRange` through `LineChart.vue`**

Add to props: `yRange: { type: Array, default: null },` and to `builderArgs`'s returned object: `yRange: props.yRange,`.

- [ ] **Step 6: Unit-aware transform in `MetricChart.vue`**

```js
import { formatNumber, formatRate } from '@/lib/core/format'

const isPercent = computed(() => props.unit === '%')

const lineSeries = computed(() =>
  props.series.map((s) => {
    const key = seriesLabelKey(s.labels)
    return {
      key,
      label: key,
      color: seriesColor(key).stroke,
      points: s.points.map((p) => ({
        t: Number(p.t) / 1e6,
        v: p.v == null ? p.v : isPercent.value ? p.v * 100 : p.v,
      })),
    }
  }),
)

function formatValue(v) {
  if (props.unit === '%') {
    const n = Number(v)
    return `${Math.abs(n) < 10 && n !== 0 ? n.toFixed(1) : Math.round(n)}%`
  }
  if (props.unit === 'By/s') return formatRate(v)
  return formatNumber(v) + (props.unit && props.unit !== '1' ? ' ' + props.unit : '')
}
```

Bind on the `LineChart` usage: `:y-range="isPercent ? [0, 100] : null"`.

- [ ] **Step 7: Legend cap in `BaseChart.vue`**

Replace the legend container's class (line ~349) — single row, horizontally scrollable, no wrapping:

```html
class="absolute inset-x-0 bottom-0 z-10 flex flex-nowrap items-center gap-x-3 overflow-x-auto px-2 [justify-content:safe_center] [scrollbar-width:none]"
```

and add `shrink-0 whitespace-nowrap` to each legend `<button>`'s class list. Add to `BaseChart.test.js`:

```js
it('legend row never wraps (single scrollable row)', () => {
  // mount with 3+ legendItems using the file's existing mount helper
  const el = wrapper.get('[data-testid="chart-legend"]')
  expect(el.classes()).toContain('flex-nowrap')
  expect(el.classes()).toContain('overflow-x-auto')
  expect(el.classes()).not.toContain('flex-wrap')
})
```

- [ ] **Step 8: Run the FULL frontend suite**

Run: `cd frontend && bun run test && bun run type-check`
Expected: PASS — including every pre-existing chart consumer test.

- [ ] **Step 9: Stage (no commit)**

```bash
git add frontend/src/lib/core/format.ts frontend/src/lib/core/format.test.ts \
  frontend/src/components/charts/chartOptions.js frontend/src/components/charts/chartOptions.test.js \
  frontend/src/components/charts/LineChart.vue frontend/src/components/metrics/MetricChart.vue \
  frontend/src/components/charts/BaseChart.vue frontend/src/components/charts/BaseChart.test.js
```

---

### Task 3: Frontend data layer — series bundle composable, tile-derivation helpers, mocks

**Files:**
- Modify: `frontend/src/lib/infra/infraQueries.ts` (add `useHostResourceSeries`)
- Create: `frontend/src/lib/infra/hostStats.ts`
- Create: `frontend/src/lib/infra/hostStats.test.ts`
- Modify: `frontend/src/lib/core/mock.ts` (`infraSeriesGroups` ~line 1667)

**Interfaces:**
- Consumes: Task 1's resource strings (`gpu_memory`, `gpu_temp`, `gpu_power`, `load`). No Task 2 dependency.
- Produces (Task 4 relies on these exact names):
  - `useHostResourceSeries(host, startNs, endNs, hasGpu)` → `{ cpu, memory, disk, network, load, gpu, gpuMemory, gpuTemp, gpuPower }`, each a `useInfraHostSeries` query.
  - `hostStats.ts`: `SeriesLike`; `latestValue(s?: SeriesLike): number | null`; `latestTotal(list?: SeriesLike[]): number | null`; `worstSeries(list: SeriesLike[] | undefined, labelKey: string): { label: string; value: number } | null`; `utilAccent(frac: number | null): 'error' | 'warning' | undefined`; `sparkValues(s?: SeriesLike): number[]`; `cpuSeriesForMode(list: SeriesLike[] | undefined, mode: 'total' | 'per-core'): SeriesLike[]`; `formatPct(frac: number | null): string`.

- [ ] **Step 1: Write the failing tests**

`frontend/src/lib/infra/hostStats.test.ts`:

```ts
import { describe, it, expect } from 'vitest'
import {
  latestValue, latestTotal, worstSeries, utilAccent, sparkValues, cpuSeriesForMode, formatPct,
} from './hostStats'
import type { SeriesLike } from './hostStats'

const s = (labels: Record<string, string>, vs: (number | null)[]): SeriesLike => ({
  labels,
  points: vs.map((v, i) => ({ t: String(i * 1_000_000), v })),
})

describe('hostStats', () => {
  it('latestValue takes the last non-null point', () => {
    expect(latestValue(s({}, [0.1, 0.5, null]))).toBe(0.5)
    expect(latestValue(s({}, [null, null]))).toBeNull()
    expect(latestValue(undefined)).toBeNull()
  })
  it('latestTotal sums latest values across series (net rx+tx)', () => {
    expect(latestTotal([s({ direction: 'receive' }, [100]), s({ direction: 'transmit' }, [40])])).toBe(140)
    expect(latestTotal([])).toBeNull()
  })
  it('worstSeries picks the max latest value and its label', () => {
    const disk = [s({ mountpoint: '/' }, [0.67]), s({ mountpoint: '/boot/efi' }, [0.04])]
    expect(worstSeries(disk, 'mountpoint')).toEqual({ label: '/', value: 0.67 })
    expect(worstSeries([], 'mountpoint')).toBeNull()
  })
  it('utilAccent thresholds at 0.8 warning / 0.9 error', () => {
    expect(utilAccent(0.5)).toBeUndefined()
    expect(utilAccent(0.8)).toBe('warning')
    expect(utilAccent(0.95)).toBe('error')
    expect(utilAccent(null)).toBeUndefined()
  })
  it('sparkValues strips nulls in order', () => {
    expect(sparkValues(s({}, [0.1, null, 0.3]))).toEqual([0.1, 0.3])
  })
  it('cpuSeriesForMode filters on the cpu label', () => {
    const cpu = [s({ cpu: 'total' }, [0.2]), s({ cpu: '0' }, [0.4]), s({ cpu: '1' }, [0.1])]
    expect(cpuSeriesForMode(cpu, 'total')).toHaveLength(1)
    expect(cpuSeriesForMode(cpu, 'per-core')).toHaveLength(2)
    expect(cpuSeriesForMode(undefined, 'total')).toEqual([])
  })
  it('formatPct renders a 0–1 fraction', () => {
    expect(formatPct(0.484)).toBe('48%')
    expect(formatPct(0.031)).toBe('3.1%')
    expect(formatPct(null)).toBe('—')
  })
})
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd frontend && bun run test -- hostStats`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement `hostStats.ts`**

```ts
// Pure tile-derivation helpers for the /infra/:host glance layer: last-point extraction,
// worst-mountpoint pick, and the shared warn/error utilization thresholds. Kept free of Vue so
// the tile math is table-testable.

export interface SeriesLike {
  labels: Record<string, string>
  points: { t: string; v: number | null }[]
}

export function latestValue(s?: SeriesLike): number | null {
  if (!s) return null
  for (let i = s.points.length - 1; i >= 0; i--) {
    const v = s.points[i].v
    if (v != null && Number.isFinite(v)) return v
  }
  return null
}

export function latestTotal(list?: SeriesLike[]): number | null {
  const vals = (list ?? []).map(latestValue).filter((v): v is number => v != null)
  if (!vals.length) return null
  return vals.reduce((a, b) => a + b, 0)
}

export function worstSeries(
  list: SeriesLike[] | undefined,
  labelKey: string,
): { label: string; value: number } | null {
  let best: { label: string; value: number } | null = null
  for (const s of list ?? []) {
    const v = latestValue(s)
    if (v == null) continue
    if (!best || v > best.value) best = { label: s.labels[labelKey] ?? '', value: v }
  }
  return best
}

// Shared glance thresholds: ≥90% error, ≥80% warning, else no accent.
export function utilAccent(frac: number | null): 'error' | 'warning' | undefined {
  if (frac == null) return undefined
  if (frac >= 0.9) return 'error'
  if (frac >= 0.8) return 'warning'
  return undefined
}

export function sparkValues(s?: SeriesLike): number[] {
  return (s?.points ?? []).map((p) => p.v).filter((v): v is number => v != null)
}

export function cpuSeriesForMode(
  list: SeriesLike[] | undefined,
  mode: 'total' | 'per-core',
): SeriesLike[] {
  const all = list ?? []
  return mode === 'total'
    ? all.filter((s) => s.labels.cpu === 'total')
    : all.filter((s) => s.labels.cpu !== 'total')
}

export function formatPct(frac: number | null): string {
  if (frac == null || !Number.isFinite(frac)) return '—'
  const pct = frac * 100
  return `${Math.abs(pct) < 10 && pct !== 0 ? pct.toFixed(1) : Math.round(pct)}%`
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd frontend && bun run test -- hostStats`
Expected: PASS.

- [ ] **Step 5: Add `useHostResourceSeries` to `infraQueries.ts`**

```ts
// The full per-resource series bundle for one host — the view creates it ONCE and passes it to
// both the glance tiles and the trend panels, so a tile and its section chart always read the
// same query cache entry. GPU-dependent resources are gated on `hasGpu` (no polling for hosts
// without a GPU), mirroring the existing gpu query's `enabled` gate.
export function useHostResourceSeries(
  host: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  hasGpu: MaybeRefOrGetter<boolean>,
) {
  return {
    cpu: useInfraHostSeries(host, 'cpu', startNs, endNs),
    memory: useInfraHostSeries(host, 'memory', startNs, endNs),
    disk: useInfraHostSeries(host, 'disk', startNs, endNs),
    network: useInfraHostSeries(host, 'network', startNs, endNs),
    load: useInfraHostSeries(host, 'load', startNs, endNs),
    gpu: useInfraHostSeries(host, 'gpu', startNs, endNs, hasGpu),
    gpuMemory: useInfraHostSeries(host, 'gpu_memory', startNs, endNs, hasGpu),
    gpuTemp: useInfraHostSeries(host, 'gpu_temp', startNs, endNs, hasGpu),
    gpuPower: useInfraHostSeries(host, 'gpu_power', startNs, endNs, hasGpu),
  }
}
export type HostResourceSeries = ReturnType<typeof useHostResourceSeries>
```

- [ ] **Step 6: Extend the mock fixtures**

In `frontend/src/lib/core/mock.ts`'s `infraSeriesGroups`, add cases before `default`:

```ts
case 'load':
  return [{ labels: { 'host.name': h.host }, base: 1.4 }]
case 'gpu_memory':
  return h.hasGpu ? [{ labels: { gpu: '0' }, base: 0.55 }] : []
case 'gpu_temp':
  return h.hasGpu ? [{ labels: { gpu: '0' }, base: 61 }] : []
case 'gpu_power':
  return h.hasGpu ? [{ labels: { gpu: '0' }, base: 180 }] : []
```

- [ ] **Step 7: Full check**

Run: `cd frontend && bun run test && bun run type-check`
Expected: PASS.

- [ ] **Step 8: Stage (no commit)**

```bash
git add frontend/src/lib/infra/hostStats.ts frontend/src/lib/infra/hostStats.test.ts \
  frontend/src/lib/infra/infraQueries.ts frontend/src/lib/core/mock.ts
```

---

### Task 4: UI — StatTile `sub`, HostStatTiles, HostResourcePanels restructure, view wiring

**Files:**
- Modify: `frontend/src/components/ui/stat-tile/StatTile.vue` (optional `sub` prop + `#spark` slot)
- Create: `frontend/src/components/infra/HostStatTiles.vue`
- Create: `frontend/src/components/infra/HostStatTiles.test.ts`
- Modify: `frontend/src/components/infra/HostResourcePanels.vue` (full rewrite — presentational)
- Modify: `frontend/src/views/InfraHostDetailView.vue` (hoist queries, render tiles + panels)

**Interfaces:**
- Consumes: Task 2's `MetricChart` `unit="%"`/`unit="By/s"` contract; Task 3's `useHostResourceSeries` / `HostResourceSeries` / `hostStats` helpers; existing `StatTile`, `Sparkline`, `Segmented`+`SegmentedItem`, `Meter` primitives; `formatBytes`/`formatRate` from `lib/core/format`.
- Produces: `HostStatTiles` props `{ res: HostResourceSeries; totalRamBytes: number | null; hasGpu: boolean }`; `HostResourcePanels` props `{ res: HostResourceSeries; startMs: number; endMs: number; hasGpu: boolean; gpuNames: string[] }` (its old self-fetching props contract is gone — the view is the only caller).

- [ ] **Step 1: Extend `StatTile.vue`** — additive, existing callers unaffected. Add `sub?: string` to props, and under the value row:

```html
<p v-if="props.sub" class="text-xs text-muted-foreground">{{ props.sub }}</p>
```

Wrap the existing content `div` and a new spark area in a flex row so a sparkline can sit at the right edge:

```html
<div class="flex items-end justify-between gap-2">
  <div class="space-y-2"><!-- existing label/value/delta/sub markup --></div>
  <div v-if="$slots.spark" class="pb-0.5 text-primary/70"><slot name="spark" /></div>
</div>
```

- [ ] **Step 2: Write the failing component test**

`frontend/src/components/infra/HostStatTiles.test.ts`:

```ts
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HostStatTiles from './HostStatTiles.vue'

const series = (labels: Record<string, string>, v: number) => ({
  labels,
  points: [{ t: '0', v }],
})
const q = (list: unknown[]) => ({ data: { value: { series: list } } })

const res = {
  cpu: q([series({ cpu: 'total' }, 0.18), series({ cpu: '0' }, 0.5)]),
  memory: q([series({ 'host.name': 'h' }, 0.48)]),
  disk: q([series({ mountpoint: '/' }, 0.67), series({ mountpoint: '/boot/efi' }, 0.04)]),
  network: q([series({ direction: 'receive' }, 1_500_000), series({ direction: 'transmit' }, 600_000)]),
  load: q([]),
  gpu: q([series({ gpu: '0' }, 0.43)]),
  gpuMemory: q([]),
  gpuTemp: q([series({ gpu: '0' }, 61)]),
  gpuPower: q([]),
} as never

describe('HostStatTiles', () => {
  it('derives current values from the last series points', () => {
    const w = mount(HostStatTiles, {
      props: { res, totalRamBytes: 32 * 1024 ** 3, hasGpu: true },
    })
    const text = w.text()
    expect(text).toContain('18%')            // cpu total (not the 50% core)
    expect(text).toContain('48%')            // memory
    expect(text).toContain('67%')            // worst mountpoint
    expect(text).toContain('/')              // its label
    expect(text).toContain('2.0 MB/s')       // rx+tx combined
    expect(text).toContain('43%')            // gpu util
    expect(text).toContain('61°C')           // gpu temp
  })
  it('hides GPU tiles when hasGpu is false', () => {
    const w = mount(HostStatTiles, { props: { res, totalRamBytes: null, hasGpu: false } })
    expect(w.text()).not.toContain('61°C')
  })
})
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd frontend && bun run test -- HostStatTiles`
Expected: FAIL — component doesn't exist.

- [ ] **Step 4: Implement `HostStatTiles.vue`**

```vue
<script setup lang="ts">
// Glance layer for /infra/:host: one current-state tile per resource, derived from the LAST point
// of the SAME series the trend panels below chart (no extra API calls — `res` is the shared
// useHostResourceSeries bundle). Percent tiles tint warn/error at the shared 80%/90% thresholds.
import { computed } from 'vue'
import { StatTile } from '@/components/ui/stat-tile'
import { Sparkline } from '@/components/ui/sparkline'
import { formatBytes, formatRate } from '@/lib/core/format'
import type { HostResourceSeries } from '@/lib/infra/infraQueries'
import {
  cpuSeriesForMode, formatPct, latestTotal, latestValue, sparkValues, utilAccent, worstSeries,
} from '@/lib/infra/hostStats'

const props = defineProps<{
  res: HostResourceSeries
  totalRamBytes: number | null
  hasGpu: boolean
}>()

const cpuTotal = computed(() => cpuSeriesForMode(props.res.cpu.data.value?.series, 'total')[0])
const cpuFrac = computed(() => latestValue(cpuTotal.value))
const memSeries = computed(() => props.res.memory.data.value?.series?.[0])
const memFrac = computed(() => latestValue(memSeries.value))
const memSub = computed(() => {
  if (memFrac.value == null || props.totalRamBytes == null) return undefined
  return `${formatBytes(memFrac.value * props.totalRamBytes)} / ${formatBytes(props.totalRamBytes)}`
})
const worstDisk = computed(() => worstSeries(props.res.disk.data.value?.series, 'mountpoint'))
const netRate = computed(() => latestTotal(props.res.network.data.value?.series))
const gpuFrac = computed(() => worstSeries(props.res.gpu.data.value?.series, 'gpu')?.value ?? null)
const gpuTemp = computed(() => worstSeries(props.res.gpuTemp.data.value?.series, 'gpu')?.value ?? null)
</script>

<template>
  <div class="grid grid-cols-2 gap-3 md:grid-cols-3" :class="hasGpu ? 'xl:grid-cols-6' : 'xl:grid-cols-4'">
    <StatTile label="CPU" :value="formatPct(cpuFrac)" :accent="utilAccent(cpuFrac)">
      <template #spark><Sparkline :points="sparkValues(cpuTotal)" /></template>
    </StatTile>
    <StatTile label="Memory" :value="formatPct(memFrac)" :sub="memSub" :accent="utilAccent(memFrac)">
      <template #spark><Sparkline :points="sparkValues(memSeries)" /></template>
    </StatTile>
    <StatTile
      label="Disk"
      :value="formatPct(worstDisk?.value ?? null)"
      :sub="worstDisk?.label"
      :accent="utilAccent(worstDisk?.value ?? null)"
    />
    <StatTile label="Network ⇅" :value="netRate == null ? '—' : formatRate(netRate)" />
    <template v-if="hasGpu">
      <StatTile label="GPU" :value="formatPct(gpuFrac)" :accent="utilAccent(gpuFrac)" />
      <StatTile label="GPU temp" :value="gpuTemp == null ? '—' : `${Math.round(gpuTemp)}°C`" />
    </template>
  </div>
</template>
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && bun run test -- HostStatTiles`
Expected: PASS. (If the `2.0 MB/s` assertion trips on `formatBytes` rounding, fix the EXPECTATION to the formatter's actual output for 2_100_000 — do not special-case the formatter.)

- [ ] **Step 6: Rewrite `HostResourcePanels.vue`** (now presentational — receives the bundle):

```vue
<script setup lang="ts">
// Trend layer for /infra/:host (layout B): one section per resource, every chart bound to the
// global time range. CPU defaults to the total series with a Segmented per-core toggle (client-
// side filter — per-core data is already in the same query). GPU gets its own 4-chart section.
import { computed, ref } from 'vue'
import MetricChart from '@/components/metrics/MetricChart.vue'
import { Meter } from '@/components/ui/meter'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { formatPct } from '@/lib/infra/hostStats'
import { cpuSeriesForMode, latestValue, utilAccent } from '@/lib/infra/hostStats'
import type { HostResourceSeries } from '@/lib/infra/infraQueries'

const props = defineProps<{
  res: HostResourceSeries
  startMs: number
  endMs: number
  hasGpu: boolean
  gpuNames: string[]
}>()

const cpuMode = ref<'total' | 'per-core'>('total')
// Reka toggle groups emit '' on deselect — swallow it so a mode is always active.
function setCpuMode(v: unknown) {
  if (v === 'total' || v === 'per-core') cpuMode.value = v
}
const cpuSeries = computed(() => cpuSeriesForMode(props.res.cpu.data.value?.series, cpuMode.value))
const diskSeries = computed(() => props.res.disk.data.value?.series ?? [])
const diskMeters = computed(() =>
  diskSeries.value
    .map((s) => ({ mountpoint: s.labels.mountpoint ?? '?', frac: latestValue(s) }))
    .filter((m) => m.frac != null)
    .sort((a, b) => (b.frac ?? 0) - (a.frac ?? 0)),
)
</script>

<template>
  <div class="flex flex-col gap-6">
    <section>
      <div class="mb-2 flex items-center justify-between">
        <h3 class="text-sm font-medium">CPU</h3>
        <Segmented :model-value="cpuMode" @update:model-value="setCpuMode">
          <SegmentedItem value="total">Total</SegmentedItem>
          <SegmentedItem value="per-core">Per-core</SegmentedItem>
        </Segmented>
      </div>
      <div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <div class="lg:col-span-2">
          <MetricChart :series="cpuSeries" unit="%" :start-ms="startMs" :end-ms="endMs" :loading="res.cpu.isLoading.value" viz="line" />
        </div>
        <div>
          <h4 class="mb-2 text-xs text-muted-foreground">Load average (1m)</h4>
          <MetricChart :series="res.load.data.value?.series ?? []" unit="1" :start-ms="startMs" :end-ms="endMs" :loading="res.load.isLoading.value" viz="line" />
        </div>
      </div>
    </section>

    <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <section>
        <h3 class="mb-2 text-sm font-medium">Memory</h3>
        <MetricChart :series="res.memory.data.value?.series ?? []" unit="%" :start-ms="startMs" :end-ms="endMs" :loading="res.memory.isLoading.value" viz="line" />
      </section>
      <section>
        <h3 class="mb-2 text-sm font-medium">Network I/O</h3>
        <MetricChart :series="res.network.data.value?.series ?? []" unit="By/s" :start-ms="startMs" :end-ms="endMs" :loading="res.network.isLoading.value" viz="area" />
      </section>
    </div>

    <section>
      <h3 class="mb-2 text-sm font-medium">Disk</h3>
      <div v-if="diskMeters.length" class="mb-3 flex flex-col gap-2">
        <div v-for="m in diskMeters" :key="m.mountpoint" class="flex items-center gap-3 text-xs">
          <span class="w-32 truncate font-mono text-muted-foreground">{{ m.mountpoint }}</span>
          <Meter :value="m.frac ?? 0" :tone="utilAccent(m.frac) ?? 'info'" class="flex-1" />
          <span class="w-12 text-right tabular-nums">{{ formatPct(m.frac) }}</span>
        </div>
      </div>
      <MetricChart :series="diskSeries" unit="%" :start-ms="startMs" :end-ms="endMs" :loading="res.disk.isLoading.value" viz="line" />
    </section>

    <section v-if="hasGpu">
      <h3 class="mb-2 text-sm font-medium">
        GPU<span v-if="gpuNames.length" class="ml-2 text-xs font-normal text-muted-foreground">{{ gpuNames.join(', ') }}</span>
      </h3>
      <div class="grid grid-cols-1 gap-4 lg:grid-cols-2 xl:grid-cols-4">
        <div><h4 class="mb-2 text-xs text-muted-foreground">Utilization</h4>
          <MetricChart :series="res.gpu.data.value?.series ?? []" unit="%" :start-ms="startMs" :end-ms="endMs" :loading="res.gpu.isLoading.value" viz="line" /></div>
        <div><h4 class="mb-2 text-xs text-muted-foreground">Memory</h4>
          <MetricChart :series="res.gpuMemory.data.value?.series ?? []" unit="%" :start-ms="startMs" :end-ms="endMs" :loading="res.gpuMemory.isLoading.value" viz="line" /></div>
        <div><h4 class="mb-2 text-xs text-muted-foreground">Temperature</h4>
          <MetricChart :series="res.gpuTemp.data.value?.series ?? []" unit="°C" :start-ms="startMs" :end-ms="endMs" :loading="res.gpuTemp.isLoading.value" viz="line" /></div>
        <div><h4 class="mb-2 text-xs text-muted-foreground">Power</h4>
          <MetricChart :series="res.gpuPower.data.value?.series ?? []" unit="W" :start-ms="startMs" :end-ms="endMs" :loading="res.gpuPower.isLoading.value" viz="line" /></div>
      </div>
    </section>
  </div>
</template>
```

- [ ] **Step 7: Wire `InfraHostDetailView.vue`** — replace the `HostResourcePanels` import block usage. Add imports and the bundle:

```ts
import HostStatTiles from '@/components/infra/HostStatTiles.vue'
import { useInfraHost, useHostResourceSeries } from '@/lib/infra/infraQueries'
// after hasGpu:
const res = useHostResourceSeries(host, startNs, endNs, hasGpu)
```

Template — between the `<header>` and the panels:

```html
<HostStatTiles :res="res" :total-ram-bytes="detail?.totalRamBytes ?? null" :has-gpu="hasGpu" />
<HostResourcePanels :res="res" :start-ms="startMs" :end-ms="endMs" :has-gpu="hasGpu" :gpu-names="detail?.gpus ?? []" />
```

- [ ] **Step 8: Full check**

Run: `cd frontend && bun run test && bun run type-check`
Expected: PASS.

- [ ] **Step 9: Stage (no commit)**

```bash
git add frontend/src/components/ui/stat-tile/StatTile.vue \
  frontend/src/components/infra/HostStatTiles.vue frontend/src/components/infra/HostStatTiles.test.ts \
  frontend/src/components/infra/HostResourcePanels.vue frontend/src/views/InfraHostDetailView.vue
```

---

### Task 5: Docs + full verification

**Files:**
- Modify: `docs/subsystems/infra.md` (curated-query table + API row + UI section)
- Modify: `docs/architecture.md` (the `/api/infra` route line: enumerate the new `resource` values)
- Verify only: `CLAUDE.md` (its infra route mention doesn't enumerate resources — update only if it does)

**Interfaces:**
- Consumes: everything shipped in Tasks 1–4 (docs must describe the code as it now is).

- [ ] **Step 1: Update `docs/subsystems/infra.md`**
  - In the "Curated query" resource table, add rows: `gpu_memory` → `system.gpu.memory.utilization` / `gpu`; `gpu_temp` → `system.gpu.temperature` / `gpu`; `gpu_power` → `system.gpu.power` / `gpu`; `load` → `system.cpu.load_average.1m` / `host.name`.
  - In the API table, extend the `timeseries` row's `resource=` list with the 4 new values.
  - Rewrite the `InfraHostDetailView.vue` bullet in the UI section: stat-tile glance row (`HostStatTiles.vue`, last-point derivation, 80/90% tint thresholds), per-resource sections (CPU total/per-core Segmented toggle + load average, memory+network, disk meters + trend, 4-chart GPU section), queries hoisted into `useHostResourceSeries`.
- [ ] **Step 2: Update `docs/architecture.md`** — find the `GET /api/infra/hosts/:host/timeseries` line and extend its documented `resource` values to `cpu|memory|disk|network|gpu|gpu_memory|gpu_temp|gpu_power|load`.
- [ ] **Step 3: Check `CLAUDE.md`** — `rg -n "infra" CLAUDE.md`; it references the three infra routes without enumerating `resource` values, so expect no change. Update only if an enumeration exists.
- [ ] **Step 4: Full gates**

```bash
cargo test -p photon-query -p photon-api
cargo fmt && cargo clippy --all-targets
cd frontend && bun run test && bun run type-check && bun run build
```
Expected: all PASS; `bun run build` regenerates `frontend/dist` so the embed tests keep passing.

- [ ] **Step 5: Live visual verification** — with the local Docker Photon + running `photon-agent`: open `/infra/<host>`, confirm (a) tiles show plausible current values and spark­lines, (b) CPU defaults to a single total line and the per-core toggle works, (c) memory y-axis reads 0–100%, (d) network axis shows readable `KB/s`/`MB/s` labels (no "00 By/s"), (e) disk meters list mountpoints worst-first, (f) GPU section shows 4 charts with the RTX 4070 SUPER name, (g) legend never wraps to a second row.
- [ ] **Step 6: Stage (no commit)**

```bash
git add docs/subsystems/infra.md docs/architecture.md
```

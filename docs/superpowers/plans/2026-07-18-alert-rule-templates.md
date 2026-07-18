# Alert Rule Templates (Quick Setup) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to
> implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a curated, read-only catalog of ready-made alert rules so a user can stand up
standard alerting for a service / app / host in one click ("Apply") or after light editing
("Customize").

**Architecture:** Frontend-only. A static typed catalog seeds the *existing* `AlertRuleDialog` and
posts through the *existing* `POST /api/alerts/rules`. No backend, no new storage, no new endpoint,
no migration. See `docs/superpowers/specs/2026-07-18-alert-rule-templates-design.md`.

**Tech Stack:** Vue 3 `<script setup lang="ts">`, TanStack Query, Reka UI primitives, vitest, bun.

## Global Constraints

- **bun, never npm.** Gate every task with `cd frontend && bun run type-check` (vue-tsc) and, where a
  test file exists, `bun run test`.
- **Do NOT commit.** Stage/edit only; leave the working tree dirty. (Strip any git-commit step.)
- **Terminology:** OK · Pending · Triggered · Resolved — never "firing".
- **No new backend, no new API, no migration.** If a task finds itself editing Rust, stop and escalate.
- **`MetricsCondition.label_filters?: Record<string,string>` already exists** in `lib/core/api.ts`
  (line 618) and is honored by `alerts_source.rs::sample_metrics`. Do not add or change API types.
- **Engine constraints:** never emit metric agg `p95` or trace kind `latency_p95` (the engine rejects
  them). `system.*.utilization` metrics are 0–1 fractions.
- Build proper shared primitives; don't patch around the existing dialog/builder.

## Execution Waves

| Wave | Tasks | Parallel? | Rationale |
|------|-------|-----------|-----------|
| 1 | T1, T2, T3 | Yes | Disjoint files (`alertTemplates.ts` / `AlertRuleDialog.vue` / `ConditionBuilder.vue`); each depends only on existing `api.ts` types |
| 2 | T4 | — | `TemplatePickerDialog.vue` + `TemplateRow.vue` need T1's catalog + `templateToRuleInput` + `summarizeCondition` |
| 3 | T5, T6 | Yes | T5 wires `AlertsView.vue`+`AlertRulesTable.vue` (needs T2 seed + T4 picker); T6 is docs (disjoint) |

**Model tiering:** T3 (ConditionBuilder round-trip + chips) and T4 (picker integration) → Opus. T1,
T2, T5, T6 → Sonnet.

---

## Task 1: The template catalog

**Files:**
- Create: `frontend/src/lib/alertTemplates.ts`
- Test: `frontend/src/lib/alertTemplates.test.ts`

**Interfaces:**
- Consumes: `AlertCondition`, `AlertSeverity`, `AlertRuleInput` from `@/lib/core/api`.
- Produces (used by T4 + T5):
  - `type TemplateTarget = 'service' | 'app' | 'host' | 'global'`
  - `interface AlertTemplate { id; target: TemplateTarget; name; description; severity; for_secs; interval_secs?; build(target: string): AlertCondition }`
  - `const ALERT_TEMPLATES: AlertTemplate[]` (23 entries)
  - `function templatesForTarget(t: TemplateTarget): AlertTemplate[]`
  - `function templateToRuleInput(t: AlertTemplate, target: string, channelIds: string[]): AlertRuleInput`
  - `function summarizeCondition(c: AlertCondition): string`
  - `function fmtSecs(s: number): string`

- [ ] **Step 1: Write the failing test** — `frontend/src/lib/alertTemplates.test.ts`

```ts
import { describe, it, expect } from 'vitest'
import {
  ALERT_TEMPLATES,
  templatesForTarget,
  templateToRuleInput,
  summarizeCondition,
} from './alertTemplates'

describe('alert template catalog', () => {
  it('has 23 templates with unique ids', () => {
    expect(ALERT_TEMPLATES).toHaveLength(23)
    expect(new Set(ALERT_TEMPLATES.map((t) => t.id)).size).toBe(23)
  })

  it('counts per target type', () => {
    expect(templatesForTarget('service')).toHaveLength(7)
    expect(templatesForTarget('app')).toHaveLength(6)
    expect(templatesForTarget('host')).toHaveLength(6)
    expect(templatesForTarget('global')).toHaveLength(4)
  })

  it('never uses the engine-rejected p95 aggregations/kinds', () => {
    for (const t of ALERT_TEMPLATES) {
      const c = t.build('x')
      if (c.signal === 'metrics') expect(c.agg).not.toBe('p95')
      if (c.signal === 'traces') expect(c.kind).not.toBe('latency_p95')
    }
  })

  it('substitutes the target into the right field per target type', () => {
    const svc = templatesForTarget('service').map((t) => t.build('checkout-api'))
    for (const c of svc) {
      if (c.signal === 'traces') expect(c.service).toBe('checkout-api')
      if (c.signal === 'logs') expect(c.query).toContain('service.name:checkout-api')
    }
    for (const t of templatesForTarget('app')) {
      const c = t.build('storefront')
      expect(c.signal).toBe('rum')
      if (c.signal === 'rum') expect(c.app_id).toBe('storefront')
    }
    for (const t of templatesForTarget('host')) {
      const c = t.build('web-01')
      expect(c.signal).toBe('metrics')
      if (c.signal === 'metrics') expect(c.label_filters?.['host.name']).toBe('web-01')
    }
    for (const t of templatesForTarget('global')) {
      const c = t.build('')
      expect(c.signal).toBe('metrics')
      if (c.signal === 'metrics') expect(c.group_by).toContain('host.name')
    }
  })

  it('utilization host/fleet templates threshold within (0, 1]', () => {
    for (const t of [...templatesForTarget('host'), ...templatesForTarget('global')]) {
      const c = t.build('h')
      if (c.signal === 'metrics' && c.metric_name.endsWith('.utilization')) {
        expect(c.threshold).toBeGreaterThan(0)
        expect(c.threshold).toBeLessThanOrEqual(1)
      }
    }
  })

  it('templateToRuleInput composes name and defaults', () => {
    const t = templatesForTarget('service')[0]
    const input = templateToRuleInput(t, 'checkout-api', ['ch1'])
    expect(input.name).toBe(`${t.name} · checkout-api`)
    expect(input.enabled).toBe(true)
    expect(input.interval_secs).toBe(60)
    expect(input.channel_ids).toEqual(['ch1'])
    expect(input.severity).toBe(t.severity)

    const g = templatesForTarget('global')[0]
    expect(templateToRuleInput(g, '', []).name).toBe(g.name) // no ` · ` suffix for global
  })

  it('summarizeCondition renders a readable line', () => {
    const c = templatesForTarget('service')[0].build('checkout-api')
    expect(summarizeCondition(c)).toMatch(/error_rate/)
  })
})
```

- [ ] **Step 2: Run it, expect failure** — `cd frontend && bun run test alertTemplates` → FAIL (module missing).

- [ ] **Step 3: Implement** — `frontend/src/lib/alertTemplates.ts`

```ts
// Static, read-only catalog of ready-made alert rules ("quick setup"). Frontend-only seed data:
// picking a template in TemplatePickerDialog either Applies it (POST /api/alerts/rules straight
// from templateToRuleInput) or Customizes it (opens AlertRuleDialog pre-seeded). `build(target)`
// substitutes the install-specific entity into the right field per target type. See
// docs/superpowers/specs/2026-07-18-alert-rule-templates-design.md.
import type { AlertCondition, AlertRuleInput, AlertSeverity } from '@/lib/core/api'

export type TemplateTarget = 'service' | 'app' | 'host' | 'global'

export interface AlertTemplate {
  id: string
  target: TemplateTarget
  name: string
  description: string
  severity: AlertSeverity
  for_secs: number
  interval_secs?: number
  /** Return a concrete condition with the target substituted in. */
  build: (target: string) => AlertCondition
}

// --- builders (keep DRY; each returns a fully-typed AlertCondition) ---------------------------
const traces = (
  kind: Extract<AlertCondition, { signal: 'traces' }>['kind'],
  window_secs: number,
  cmp: AlertCondition['cmp'],
  threshold: number,
) => (service: string): AlertCondition => ({ signal: 'traces', service, kind, window_secs, cmp, threshold })

const logs = (severity: string, window_secs: number, threshold: number) =>
  (service: string): AlertCondition => ({
    signal: 'logs',
    query: `service.name:${service} severity:${severity}`,
    group_by: null,
    window_secs,
    cmp: 'gt',
    threshold,
  })

const rum = (
  kind: Extract<AlertCondition, { signal: 'rum' }>['kind'],
  threshold: number,
) => (app_id: string): AlertCondition => ({
  signal: 'rum',
  app_id,
  route: null,
  kind,
  window_secs: 900,
  cmp: 'gt',
  threshold,
})

const hostMetric = (
  metric_name: string,
  agg: Extract<AlertCondition, { signal: 'metrics' }>['agg'],
  window_secs: number,
  threshold: number,
) => (host: string): AlertCondition => ({
  signal: 'metrics',
  metric_name,
  label_filters: { 'host.name': host },
  agg,
  window_secs,
  cmp: 'gt',
  threshold,
})

const fleetMetric = (
  metric_name: string,
  agg: Extract<AlertCondition, { signal: 'metrics' }>['agg'],
  window_secs: number,
  threshold: number,
) => (): AlertCondition => ({
  signal: 'metrics',
  metric_name,
  group_by: ['host.name'],
  agg,
  window_secs,
  cmp: 'gt',
  threshold,
})

export const ALERT_TEMPLATES: AlertTemplate[] = [
  // --- Service (traces + logs) ---
  { id: 'svc-high-error-rate', target: 'service', name: 'High error rate', description: 'Error rate above 5% over 5m.', severity: 'critical', for_secs: 300, build: traces('error_rate', 300, 'gt', 5) },
  { id: 'svc-elevated-error-rate', target: 'service', name: 'Elevated error rate', description: 'Error rate above 1% over 5m.', severity: 'warning', for_secs: 600, build: traces('error_rate', 300, 'gt', 1) },
  { id: 'svc-slow-p99', target: 'service', name: 'Slow responses (p99)', description: 'p99 latency above 1000ms over 5m.', severity: 'warning', for_secs: 300, build: traces('latency_p99', 300, 'gt', 1000) },
  { id: 'svc-slow-p90', target: 'service', name: 'Slow responses (p90)', description: 'p90 latency above 500ms over 5m.', severity: 'warning', for_secs: 600, build: traces('latency_p90', 300, 'gt', 500) },
  { id: 'svc-traffic-dropped', target: 'service', name: 'Traffic dropped', description: 'Request rate below 1 req/s over 10m — the service may be down.', severity: 'warning', for_secs: 600, build: traces('request_rate', 600, 'lt', 1) },
  { id: 'svc-error-logs', target: 'service', name: 'Error logs surging', description: 'More than 100 error logs over 10m.', severity: 'warning', for_secs: 300, build: logs('error', 600, 100) },
  { id: 'svc-fatal-logs', target: 'service', name: 'Fatal logs appeared', description: 'Any fatal log over 5m.', severity: 'critical', for_secs: 0, build: logs('fatal', 300, 0) },

  // --- RUM app ---
  { id: 'rum-poor-lcp', target: 'app', name: 'Poor LCP', description: 'LCP p75 above 2500ms over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_lcp_p75', 2500) },
  { id: 'rum-poor-inp', target: 'app', name: 'Poor INP', description: 'INP p75 above 200ms over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_inp_p75', 200) },
  { id: 'rum-poor-cls', target: 'app', name: 'Layout shift (CLS)', description: 'CLS p75 above 0.1 over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_cls_p75', 0.1) },
  { id: 'rum-slow-fcp', target: 'app', name: 'Slow FCP', description: 'FCP p75 above 1800ms over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_fcp_p75', 1800) },
  { id: 'rum-slow-ttfb', target: 'app', name: 'Slow TTFB', description: 'TTFB p75 above 800ms over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_ttfb_p75', 800) },
  { id: 'rum-js-errors', target: 'app', name: 'JS errors surging', description: 'More than 50 JS errors over 15m.', severity: 'warning', for_secs: 0, build: rum('error_count', 50) },

  // --- Host (one host) ---
  { id: 'host-cpu', target: 'host', name: 'CPU saturated', description: 'CPU utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: hostMetric('system.cpu.utilization', 'avg', 300, 0.9) },
  { id: 'host-mem', target: 'host', name: 'Memory pressure', description: 'Memory utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: hostMetric('system.memory.utilization', 'avg', 300, 0.9) },
  { id: 'host-disk', target: 'host', name: 'Disk filling up', description: 'Filesystem utilization above 85% over 10m.', severity: 'warning', for_secs: 600, build: hostMetric('system.filesystem.utilization', 'avg', 600, 0.85) },
  { id: 'host-gpu', target: 'host', name: 'GPU saturated', description: 'GPU utilization above 95% over 5m.', severity: 'warning', for_secs: 300, build: hostMetric('system.gpu.utilization', 'avg', 300, 0.95) },
  { id: 'host-gpu-temp', target: 'host', name: 'GPU overheating', description: 'GPU temperature above 85°C over 5m.', severity: 'critical', for_secs: 300, build: hostMetric('system.gpu.temperature', 'max', 300, 85) },
  { id: 'host-gpu-mem', target: 'host', name: 'GPU memory pressure', description: 'GPU memory utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: hostMetric('system.gpu.memory.utilization', 'avg', 300, 0.9) },

  // --- Global / fleet (one series per host) ---
  { id: 'fleet-cpu', target: 'global', name: 'Any host CPU saturated', description: 'Any host CPU utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: fleetMetric('system.cpu.utilization', 'avg', 300, 0.9) },
  { id: 'fleet-mem', target: 'global', name: 'Any host memory pressure', description: 'Any host memory utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: fleetMetric('system.memory.utilization', 'avg', 300, 0.9) },
  { id: 'fleet-disk', target: 'global', name: 'Any host disk filling', description: 'Any host filesystem utilization above 85% over 10m.', severity: 'warning', for_secs: 600, build: fleetMetric('system.filesystem.utilization', 'avg', 600, 0.85) },
  { id: 'fleet-gpu-temp', target: 'global', name: 'Any GPU overheating', description: 'Any GPU temperature above 85°C over 5m.', severity: 'critical', for_secs: 300, build: fleetMetric('system.gpu.temperature', 'max', 300, 85) },
]

export function templatesForTarget(target: TemplateTarget): AlertTemplate[] {
  return ALERT_TEMPLATES.filter((t) => t.target === target)
}

export function templateToRuleInput(
  t: AlertTemplate,
  target: string,
  channelIds: string[],
): AlertRuleInput {
  const condition = t.build(target)
  const suffix = t.target === 'global' || !target ? '' : ` · ${target}`
  return {
    name: `${t.name}${suffix}`,
    description: t.description,
    enabled: true,
    signal: condition.signal,
    condition,
    for_secs: t.for_secs,
    interval_secs: t.interval_secs ?? 60,
    severity: t.severity,
    channel_ids: channelIds,
  }
}

const CMP_SYM: Record<AlertCondition['cmp'], string> = { gt: '>', gte: '≥', lt: '<', lte: '≤' }

export function fmtSecs(s: number): string {
  if (s === 0) return 'immediately'
  if (s % 3600 === 0) return `${s / 3600}h`
  if (s % 60 === 0) return `${s / 60}m`
  return `${s}s`
}

/** A compact plain-English line for a template row / preview (numbers only; target-agnostic). */
export function summarizeCondition(c: AlertCondition): string {
  const win = `over ${fmtSecs(c.window_secs)}`
  const op = CMP_SYM[c.cmp]
  switch (c.signal) {
    case 'metrics': {
      const by = c.group_by?.length ? ` by ${c.group_by.join(', ')}` : ''
      return `${c.agg}(${c.metric_name})${by} ${op} ${c.threshold} ${win}`
    }
    case 'logs':
      return `count(logs) ${op} ${c.threshold} ${win}`
    case 'traces':
      return `${c.kind} ${op} ${c.threshold} ${win}`
    case 'rum':
      return `${c.kind} ${op} ${c.threshold} ${win}`
  }
}
```

- [ ] **Step 4: Run tests** — `cd frontend && bun run test alertTemplates` → PASS. Then `bun run type-check` → clean.

---

## Task 2: `AlertRuleDialog` — create-pre-seeded mode

**Files:**
- Modify: `frontend/src/components/alerts/AlertRuleDialog.vue`

**Interfaces:**
- Produces: a new optional prop `seed?: AlertRuleInput | null`. When `rule` is null and `open` becomes
  true, the dialog opens in **create** mode pre-filled from `seed` (or blank if no seed). Edit mode
  (`rule` set) is unchanged.
- Consumed by T5 (AlertsView passes `:seed` for the Customize path).

**Context:** The dialog currently seeds from `props.rule` via two watchers and `:key`s
`ConditionBuilder` on `rule?.id ?? 'new'`. `ConditionBuilder` seeds once at setup, so to reseed a
fresh create draft we must (a) set `condition.value`/`form` from the seed and (b) change the key so
the builder remounts. Add a `createNonce` counter for the key.

- [ ] **Step 1: Add the `seed` prop + `applyDraft` helper.** Replace the `defineProps` line and the two
  seeding watchers (`watch(() => props.rule, …)` and `watch(() => props.open, …)`):

```ts
const props = defineProps<{ open: boolean; rule?: AlertRule | null; seed?: AlertRuleInput | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const isEdit = computed(() => !!props.rule)
const createNonce = ref(0)
```

```ts
function applyCreateDraft() {
  // create mode: pre-fill from `seed` if present, else blank
  const s = props.seed
  Object.assign(form, {
    name: s?.name ?? '',
    description: s?.description ?? '',
    severity: s?.severity ?? ('warning' as AlertSeverity),
    for_secs: s?.for_secs ?? 300,
    channel_ids: s?.channel_ids ? [...s.channel_ids] : [],
  })
  condition.value = s?.condition ?? defaultCondition()
}

watch(
  () => props.rule,
  (r) => {
    if (r) {
      Object.assign(form, {
        name: r.name,
        description: r.description ?? '',
        severity: r.severity,
        for_secs: r.for_secs,
        channel_ids: [...r.channel_ids],
      })
      condition.value = r.condition
    } else {
      applyCreateDraft()
    }
  },
  { immediate: true },
)

watch(
  () => props.open,
  (isOpen) => {
    if (isOpen && !props.rule) {
      applyCreateDraft() // re-apply seed each open so a new template draft takes effect
      createNonce.value++ // force ConditionBuilder to remount + reseed from the new condition
    } else if (!isOpen && !props.rule) {
      Object.assign(form, blank())
      condition.value = defaultCondition()
    }
  },
)
```

- [ ] **Step 2: Key the builder on the nonce.** Change the `ConditionBuilder` tag's key:

```html
<ConditionBuilder :key="rule?.id ?? `new-${createNonce}`" ref="conditionBuilderRef" v-model:condition="condition" />
```

- [ ] **Step 3: Verify no other change needed.** `submit()` already reads `condition.value.signal`,
  `form.*`, and calls `createMut` when `!isEdit` — correct for a seeded create. `AlertRuleInput` is
  already imported.

- [ ] **Step 4: Type-check** — `cd frontend && bun run type-check` → clean. Manual smoke deferred to T5.

---

## Task 3: `ConditionBuilder` — round-trip `label_filters` (+ editable chips)

**Files:**
- Modify: `frontend/src/components/alerts/ConditionBuilder.vue`
- Test: `frontend/src/components/alerts/ConditionBuilder.test.ts`

**Why:** Host templates put `label_filters: { 'host.name': <host> }` on a metrics condition. Today
`ConditionBuilder.seed()`/`builtCondition` drop everything but `metric_name`/`agg`/`group_by`, so a
**Customized** Host template would silently lose its host scope. Make label filters round-trip and
editable as key=value chips (mirroring the existing group-by chip UI).

**Interfaces:** unchanged public surface (`v-model:condition`, exposed `isValid`/`previewSeries`).
Internally: `FormState` gains `label_filters: Record<string, string>`.

- [ ] **Step 1: Write the failing test** — `frontend/src/components/alerts/ConditionBuilder.test.ts`

```ts
import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import ConditionBuilder from './ConditionBuilder.vue'

// The builder fires autocomplete queries on mount; a bare QueryClient is enough (they stay pending).
function mountBuilder(condition: unknown) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(ConditionBuilder, {
    props: { condition },
    global: { plugins: [[VueQueryPlugin, { queryClient: qc }]] },
  })
}

describe('ConditionBuilder label_filters round-trip', () => {
  it('re-emits a seeded host.name label filter', async () => {
    const seeded = {
      signal: 'metrics',
      metric_name: 'system.cpu.utilization',
      label_filters: { 'host.name': 'web-01' },
      agg: 'avg',
      window_secs: 300,
      cmp: 'gt',
      threshold: 0.9,
    }
    const wrapper = mountBuilder(seeded)
    const emitted = wrapper.emitted('update:condition') as unknown[][] | undefined
    expect(emitted, 'should emit at least once (immediate)').toBeTruthy()
    const last = emitted!.at(-1)![0] as { label_filters?: Record<string, string> }
    expect(last.label_filters).toEqual({ 'host.name': 'web-01' })
  })
})
```

- [ ] **Step 2: Run it, expect failure** — `cd frontend && bun run test ConditionBuilder` → FAIL
  (`label_filters` is `undefined`, dropped by seed/build).

  > If mounting proves impractical in this harness (Reka UI deps), instead extract the pure
  > `seed()` and `builtCondition` mapping into a tested helper module and assert the round-trip
  > there. Do not weaken the assertion.

- [ ] **Step 3: Thread `label_filters` through state.** In `FormState` add `label_filters: Record<string, string>`;
  in `blankForm()` add `label_filters: {}`; in `seed()`'s `case 'metrics'` add
  `form.label_filters = c.label_filters ? { ...c.label_filters } : {}`; in `builtCondition`'s metrics
  branch add `label_filters: Object.keys(form.label_filters).length ? { ...form.label_filters } : undefined`.

- [ ] **Step 4: Add the chip editor UI** (metrics only), right above the existing group-by `FormField`:

```html
<FormField
  v-if="signal === 'metrics'"
  label="Filter labels"
  :optional="true"
  hint="Restrict to matching label values, e.g. host.name=web-01."
  class="mt-3"
>
  <div class="flex flex-wrap items-center gap-1.5">
    <span
      v-for="(v, k) in form.label_filters"
      :key="k"
      class="inline-flex items-center gap-1 rounded-md border border-border bg-muted px-2 py-1 text-xs font-mono"
    >
      {{ k }}={{ v }}
      <button type="button" class="text-muted-foreground hover:text-foreground" @click="removeLabelFilter(k)">
        <X class="size-3" />
      </button>
    </span>
    <input
      v-model="labelFilterDraft"
      placeholder="host.name=web-01…"
      class="h-7 w-40 rounded-md border border-input bg-background px-2 font-mono text-xs outline-none focus-visible:ring-1 focus-visible:ring-ring"
      @keydown="onLabelFilterKeydown"
      @blur="addLabelFilter"
    >
  </div>
</FormField>
```

- [ ] **Step 5: Add the chip handlers** (near the group-by handlers):

```ts
const labelFilterDraft = ref('')
function addLabelFilter() {
  const raw = labelFilterDraft.value.trim()
  const eq = raw.indexOf('=')
  if (eq > 0) {
    const k = raw.slice(0, eq).trim()
    const v = raw.slice(eq + 1).trim()
    if (k && v) form.label_filters[k] = v
  }
  labelFilterDraft.value = ''
}
function onLabelFilterKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    addLabelFilter()
  }
}
function removeLabelFilter(k: string) {
  delete form.label_filters[k]
}
```

- [ ] **Step 6: Run tests** — `cd frontend && bun run test ConditionBuilder` → PASS; `bun run type-check` → clean.

---

## Task 4: `TemplatePickerDialog` + `TemplateRow`

**Files:**
- Create: `frontend/src/components/alerts/TemplateRow.vue`
- Create: `frontend/src/components/alerts/TemplatePickerDialog.vue`

**Interfaces:**
- `TemplateRow` props `{ template: AlertTemplate; disabled: boolean }`, emits `apply` and `customize`.
- `TemplatePickerDialog` props `{ open: boolean }`, emits `update:open` and
  `customize: [seed: AlertRuleInput]`. Apply is handled internally (`useCreateRule`), then closes.

**Consumes:** `ALERT_TEMPLATES`/`templatesForTarget`/`templateToRuleInput`/`summarizeCondition`/`fmtSecs`
(T1); `useServices` (`@/lib/logs/logsQueries`), `useRumApps` (`@/lib/rum/rumQueries`), `useInfraHosts`
(`@/lib/infra/infraQueries`), `useChannels`/`useCreateRule` (`@/lib/alertsQueries`); `startNs`/`endNs`
(`@/lib/core/context`).

- [ ] **Step 1: `TemplateRow.vue`**

```vue
<script setup lang="ts">
// One template in the picker: plain-English condition + severity, with Apply / Customize.
import { computed } from 'vue'
import { Button } from '@/components/ui/button'
import { StatusPill } from '@/components/ui/status-pill'
import { summarizeCondition, fmtSecs, type AlertTemplate } from '@/lib/alertTemplates'

const props = defineProps<{ template: AlertTemplate; disabled: boolean }>()
defineEmits<{ apply: []; customize: [] }>()

const summary = computed(() => summarizeCondition(props.template.build('…')))
const sevTone = computed(() => (props.template.severity === 'critical' ? 'error' : 'warning'))
</script>

<template>
  <div class="flex items-center justify-between gap-4 rounded-lg border border-border bg-card px-4 py-3">
    <div class="min-w-0">
      <div class="flex items-center gap-2">
        <span class="text-sm font-medium text-foreground">{{ template.name }}</span>
        <StatusPill :tone="sevTone">{{ template.severity }}</StatusPill>
      </div>
      <p class="mt-0.5 truncate font-mono text-xs text-muted-foreground">
        {{ summary }} · for {{ fmtSecs(template.for_secs) }}
      </p>
    </div>
    <div class="flex shrink-0 items-center gap-2">
      <Button size="sm" variant="ghost" :disabled="disabled" @click="$emit('customize')">Customize</Button>
      <Button size="sm" :disabled="disabled" @click="$emit('apply')">Apply</Button>
    </div>
  </div>
</template>
```

- [ ] **Step 2: `TemplatePickerDialog.vue`**

```vue
<script setup lang="ts">
// Quick-setup template picker: pick a target (Service/App/Host/Global) → list of templates for it →
// Apply (POST straight from templateToRuleInput) or Customize (emit a seed → AlertsView opens
// AlertRuleDialog pre-seeded). Frontend-only; see docs/superpowers/specs/2026-07-18-alert-rule-templates-design.md.
import { computed, ref, watch } from 'vue'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { SelectMenu } from '@/components/ui/select-menu'
import { FormField } from '@/components/ui/form-field'
import TemplateRow from './TemplateRow.vue'
import { startNs, endNs } from '@/lib/core/context'
import { useServices } from '@/lib/logs/logsQueries'
import { useRumApps } from '@/lib/rum/rumQueries'
import { useInfraHosts } from '@/lib/infra/infraQueries'
import { useChannels, useCreateRule } from '@/lib/alertsQueries'
import {
  templatesForTarget,
  templateToRuleInput,
  type TemplateTarget,
  type AlertTemplate,
} from '@/lib/alertTemplates'
import type { AlertRuleInput } from '@/lib/core/api'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean]; customize: [seed: AlertRuleInput] }>()

const TARGETS: { value: TemplateTarget; label: string }[] = [
  { value: 'service', label: 'Service' },
  { value: 'app', label: 'RUM app' },
  { value: 'host', label: 'Host' },
  { value: 'global', label: 'Global' },
]

const target = ref<TemplateTarget>('service')
const selected = ref('') // chosen service/app/host name (unused for global)
const channelIds = ref<string[]>([])

function onPickTarget(v: unknown) {
  if (typeof v === 'string' && v && v !== target.value) {
    target.value = v as TemplateTarget
    selected.value = ''
  }
}
// Reset the transient picks each time the dialog reopens.
watch(() => props.open, (o) => { if (o) { target.value = 'service'; selected.value = ''; channelIds.value = [] } })

// --- target selector options ---
const servicesQuery = useServices()
const rumAppsQuery = useRumApps()
const hostsQuery = useInfraHosts(startNs, endNs)
const targetOptions = computed<{ value: string; label: string }[]>(() => {
  const names =
    target.value === 'service'
      ? servicesQuery.data.value ?? []
      : target.value === 'app'
        ? (rumAppsQuery.data.value?.apps ?? []).map((a) => a.name)
        : target.value === 'host'
          ? (hostsQuery.data.value?.hosts ?? []).map((h) => h.host)
          : []
  return names.map((n) => ({ value: n, label: n }))
})
const needsTarget = computed(() => target.value !== 'global')
const rowsDisabled = computed(() => needsTarget.value && !selected.value)

const templates = computed<AlertTemplate[]>(() => templatesForTarget(target.value))

const channelsQuery = useChannels()
const channels = computed(() => channelsQuery.data.value ?? [])
function toggleChannel(id: string) {
  const i = channelIds.value.indexOf(id)
  if (i === -1) channelIds.value.push(id)
  else channelIds.value.splice(i, 1)
}

const createMut = useCreateRule()

function apply(t: AlertTemplate) {
  const input = templateToRuleInput(t, selected.value, [...channelIds.value])
  createMut.mutate(input, {
    onSuccess: (res) => {
      if (res && res.ok === false) return // useCreateRule already toasts the error
      emit('update:open', false)
    },
  })
}
function customize(t: AlertTemplate) {
  emit('customize', templateToRuleInput(t, selected.value, [...channelIds.value]))
  emit('update:open', false)
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-h-[85vh] max-w-2xl overflow-y-auto">
      <DialogHeader>
        <DialogTitle>Browse templates</DialogTitle>
        <DialogDescription>Pick a target, then apply a ready-made alert — customize only if you need to.</DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-5">
        <Segmented :model-value="target" @update:model-value="onPickTarget">
          <SegmentedItem v-for="t in TARGETS" :key="t.value" :value="t.value">{{ t.label }}</SegmentedItem>
        </Segmented>

        <FormField v-if="needsTarget" :label="TARGETS.find((t) => t.value === target)!.label">
          <SelectMenu
            v-if="targetOptions.length"
            v-model="selected"
            :options="targetOptions"
            content-class="w-56"
            :aria-label="`${target} to alert on`"
          />
          <p v-else class="text-xs text-muted-foreground">
            No {{ target }}s discovered yet — send some data first.
          </p>
        </FormField>

        <FormField label="Notify" hint="Channels attached when you Apply. Optional.">
          <div class="flex flex-wrap gap-2">
            <button
              v-for="c in channels"
              :key="c.id"
              type="button"
              :aria-pressed="channelIds.includes(c.id)"
              class="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors"
              :class="channelIds.includes(c.id) ? 'border-brand/40 bg-brand/10 text-brand' : 'border-border bg-muted text-muted-foreground hover:text-foreground'"
              @click="toggleChannel(c.id)"
            >
              {{ c.name }}
            </button>
            <p v-if="!channels.length" class="text-xs text-muted-foreground">
              No channels yet; the rule will be created without notifications — add one on the Channels tab.
            </p>
          </div>
        </FormField>

        <div class="flex flex-col gap-2">
          <p v-if="rowsDisabled" class="text-xs text-muted-foreground">
            Pick a {{ target }} above to apply a template.
          </p>
          <TemplateRow
            v-for="t in templates"
            :key="t.id"
            :template="t"
            :disabled="rowsDisabled"
            @apply="apply(t)"
            @customize="customize(t)"
          />
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
```

- [ ] **Step 3: Verify primitives exist.** `Segmented`/`SegmentedItem`, `SelectMenu`, `FormField`,
  `StatusPill`, `Button`, `Dialog*` are all already used by sibling alert components — import paths as
  shown. If `StatusPill`'s tone prop differs, match `ConditionBuilder`'s usage (`tone="error|success|neutral"`).

- [ ] **Step 4: Type-check** — `cd frontend && bun run type-check` → clean.

---

## Task 5: Wire the entry point into `AlertsView` + `AlertRulesTable`

**Files:**
- Modify: `frontend/src/components/alerts/AlertRulesTable.vue`
- Modify: `frontend/src/views/AlertsView.vue`

**Interfaces:** consumes T2 (`:seed` on `AlertRuleDialog`) and T4 (`TemplatePickerDialog`).

- [ ] **Step 1: `AlertRulesTable` — add a "Browse templates" button + empty-state action.** Add
  `browse-templates` to the emits, a secondary button beside "New alert", and an action button in the
  empty state:

```ts
defineEmits<{ 'open-create': []; 'browse-templates': []; edit: [rule: AlertRule] }>()
```

```html
<div class="mb-3 flex justify-end gap-2">
  <Button size="sm" variant="secondary" data-testid="alert-browse-templates" @click="$emit('browse-templates')">
    <LayoutTemplate class="mr-1.5 size-3.5" />
    Browse templates
  </Button>
  <Button size="sm" data-testid="alert-new-rule" @click="$emit('open-create')">
    <Plus class="mr-1.5 size-3.5" />
    New alert
  </Button>
</div>
```

Add `LayoutTemplate` to the `lucide-vue-next` import. In the `EmptyState`, use its default slot:

```html
<EmptyState
  v-else-if="!rules.length"
  title="No alert rules yet"
  description="Start from a template, or create one from scratch."
>
  <div class="mt-3 flex justify-center gap-2">
    <Button size="sm" @click="$emit('browse-templates')">
      <LayoutTemplate class="mr-1.5 size-3.5" />
      Browse templates
    </Button>
    <Button size="sm" variant="secondary" @click="$emit('open-create')">New alert</Button>
  </div>
</EmptyState>
```

(If `Button`'s `variant="secondary"` isn't a defined variant, use `variant="outline"` — match what
sibling components use.)

- [ ] **Step 2: `AlertsView` — open the picker, and route Customize into the pre-seeded dialog.** Add
  imports, state, handlers, and wire the two components:

```ts
import TemplatePickerDialog from '@/components/alerts/TemplatePickerDialog.vue'
import { api, type AlertRule, type AlertRuleInput } from '@/lib/core/api'
```

```ts
const pickerOpen = ref(false)
const templateSeed = ref<AlertRuleInput | null>(null)

function openCreate() {
  editingRule.value = null
  templateSeed.value = null // ensure a blank draft, not a lingering template
  dialogOpen.value = true
}
function openBrowseTemplates() {
  pickerOpen.value = true
}
function onCustomizeTemplate(seed: AlertRuleInput) {
  editingRule.value = null
  templateSeed.value = seed
  dialogOpen.value = true
}
```

```html
<AlertRulesTable
  v-if="tab === 'rules'"
  @open-create="openCreate"
  @browse-templates="openBrowseTemplates"
  @edit="openEdit"
/>
...
<TemplatePickerDialog
  :open="pickerOpen"
  @update:open="pickerOpen = $event"
  @customize="onCustomizeTemplate"
/>
<AlertRuleDialog
  :open="dialogOpen"
  :rule="editingRule"
  :seed="templateSeed"
  @update:open="dialogOpen = $event"
/>
```

- [ ] **Step 3: Manual smoke** — `cd frontend && bun run dev` against a running backend:
  1. `/alerts` → Rules empty state shows both actions.
  2. **Browse templates** → pick Service → select a service → **Apply** a template → toast "Rule created",
     rule appears with name `<Template> · <service>`.
  3. Browse → Host → pick a host → **Customize** → dialog opens pre-filled; the metrics builder shows a
     `host.name=<host>` filter chip; Save works.
  4. Global → Apply a fleet template (no target needed) → rule created with `group_by host.name`.

- [ ] **Step 4: Gate** — `cd frontend && bun run type-check` → clean; `bun run test` → green; `bun run build` → OK.

---

## Task 6: Docs

**Files:**
- Modify: `docs/subsystems/alerts.md`, `docs/frontend.md`, `CLAUDE.md`

- [ ] **Step 1: `docs/subsystems/alerts.md`** — add a "Templates / quick setup" subsection: the
  target-first flow (Service/App/Host/Global → Apply/Customize), that it's **frontend-only** static
  seed data in `frontend/src/lib/alertTemplates.ts` (23 templates) seeding the existing create path,
  the `host.name` `label_filters` mechanism for Host templates, and the explicit non-goal of
  provider-native channels (booked follow-up).

- [ ] **Step 2: `docs/frontend.md`** — note the new components (`TemplatePickerDialog`, `TemplateRow`,
  `alertTemplates.ts`), the "Browse templates" entry point on `/alerts`, and the `AlertRuleDialog`
  `:seed` create-pre-seeded mode.

- [ ] **Step 3: `CLAUDE.md`** — one line in the alerts description: a template quick-setup on-ramp
  (target-first, Apply/Customize, frontend-only seed catalog).

- [ ] **Step 4: Re-verify internal links** in the edited docs.

---

## Self-Review (done during authoring)

- **Spec coverage:** target picker (T4), single-select Apply/Customize (T4/T5), catalog of 23 (T1),
  reuse of `AlertRuleDialog` via `:seed` (T2/T5), `label_filters` round-trip so Customize keeps Host
  scope (T3), channels-on-Apply with the no-channels hint (T4), name `<Template> · <target>` (T1),
  docs (T6). All covered.
- **Type consistency:** `templateToRuleInput` returns `AlertRuleInput`; `AlertRuleDialog :seed` is
  `AlertRuleInput | null`; `TemplatePickerDialog` emits `customize: [AlertRuleInput]`; all aligned.
- **No placeholders:** every code step is complete.
- **No backend:** confirmed — `label_filters` already exists in the type and is honored server-side.

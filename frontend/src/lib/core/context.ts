// App-wide observability context: the selected time window, the active federation tenant (a
// Site), and the active deployment environment. Three dimensions, all global, all applied to
// every signal. The auth.js / theme.js module-singleton pattern (NOT Pinia). One source of truth
// for the time math that every view used to duplicate. URL sync lives in this file too (Task 2).
import { ref, computed, watch, type Ref, type ComputedRef } from 'vue'
import { replaceSearch } from '@/lib/core/historyUrl'

// Same preset table every view used (LogsView/ServiceDetailView/...). Milliseconds.
export const RANGE_MS: Record<string, number> = {
  '5m': 3e5, '15m': 9e5, '30m': 18e5, '1h': 36e5, '3h': 108e5,
  '6h': 216e5, '12h': 432e5, '24h': 864e5, '7d': 6048e5,
}

export interface CustomRange {
  startMs: number
  endMs: number
}

// --- state (module singletons) ---
export const timeRange: Ref<string> = ref('30m')                    // preset key into RANGE_MS
export const customRange: Ref<CustomRange | null> = ref(null)        // absolute; wins over preset
export const tenant: Ref<string | null> = ref(null)
export const environment: Ref<string | null> = ref(null)             // `deployment.environment`; null = every environment
// Latch for the one-shot `prod` default below. A ref (not a plain `let`) so tests reset it the
// same way they reset every other singleton in this file.
export const environmentDefaulted: Ref<boolean> = ref(false)
export const nowTick: Ref<number> = ref(Date.now())                  // advanced on range change / by the live control

// --- derived window ---
export const endMs: ComputedRef<number> = computed(() =>
  customRange.value ? customRange.value.endMs : nowTick.value,
)
export const startMs: ComputedRef<number> = computed(() =>
  customRange.value ? customRange.value.startMs : endMs.value - (RANGE_MS[timeRange.value] ?? RANGE_MS['30m']),
)
export const windowMs: ComputedRef<number> = computed(() => Math.max(1, endMs.value - startMs.value))
const prevEndMs: ComputedRef<number> = computed(() => startMs.value)
const prevStartMs: ComputedRef<number> = computed(() => startMs.value - windowMs.value)

const toNs = (ms: number): string => (BigInt(Math.round(ms)) * 1_000_000n).toString()
export const startNs: ComputedRef<string> = computed(() => toNs(startMs.value))
export const endNs: ComputedRef<string> = computed(() => toNs(endMs.value))
export const prevStartNs: ComputedRef<string> = computed(() => toNs(prevStartMs.value))
export const prevEndNs: ComputedRef<string> = computed(() => toNs(prevEndMs.value))

// --- actions ---
export function setTimeRange(r: string): void {
  timeRange.value = r
  customRange.value = null       // presets and custom ranges are mutually exclusive
  nowTick.value = Date.now()     // re-anchor "now" so the window is fresh
}
export function setCustomRange(r: CustomRange | null): void {
  customRange.value = r
}
export function setTenant(id: string): void {
  tenant.value = id
}
export function clearTenant(): void {
  tenant.value = null
}
export function setEnvironment(name: string): void {
  environment.value = name
}
export function clearEnvironment(): void {
  environment.value = null
}

// The grammar term the active tenant contributes to backend queries (logs/traces/metrics
// composables append it to the `q`/`filter` string they send). The value arrives from the free
// `?tenant=` URL param — anything outside the server's `[a-z0-9-]{1,64}` tenant-name shape can't
// be a real tenant AND would inject stray grammar terms (`?tenant=a b` → ` b` body-substring),
// so it contributes nothing.
export function tenantQueryTerm(): string | null {
  const t = tenant.value
  return t && /^[a-z0-9-]{1,64}$/.test(t) ? `tenant:${t}` : null
}

// Same contract for the environment: `deployment.environment` is a promoted attribute, so this is
// an ordinary grammar term. The shape guard is the `?env=` counterpart of the tenant one — the
// grammar splits terms on whitespace and commas, so a value carrying either (or a quote) would
// inject a stray body-substring term rather than filter, and contributes nothing instead.
// Deliberately NOT `deployment.environment:*` when nothing is selected: that would drop every
// record whose producer declares no environment, which is the opposite of "all environments".
export function envQueryTerm(): string | null {
  const e = environment.value
  return e && /^[A-Za-z0-9._-]{1,64}$/.test(e) ? `deployment.environment:${e}` : null
}

// Site + Environment as ONE grammar fragment. Composables put this single value in their cache
// key (one slot, not one per dimension) and join it onto the query string they send; `null` when
// neither is set reads as "no context filter" in a key.
export function contextQueryTerm(): string | null {
  const terms = [tenantQueryTerm(), envQueryTerm()].filter(Boolean)
  return terms.length ? terms.join(' ') : null
}

// `q` (a view's own filter) plus the context fragment. The single point where the global
// dimensions reach the wire — every query composable routes through it.
export function withContext(q?: string): string {
  return [q ?? '', contextQueryTerm()].filter(Boolean).join(' ')
}

// One-shot default selection, called by the ContextBar once its environment facet resolves.
// `prod` is what the reader almost always wants: it is the environment worth waking someone for,
// and averaging staging's numbers into it is how a real regression gets hidden. Everything else
// stays one click away rather than selected for them.
//
// Two guards, both deliberate:
//   - fewer than two values is not a decision at all, so it neither selects nor spends the latch.
//     The picker hides at that threshold, and filtering by a dimension the reader cannot see or
//     unset is worse than not filtering at all — but a later window that DOES have a choice still
//     gets its default.
//   - once only. The facet refetches on every window change; without the latch, clearing the
//     picker would snap back to `prod` on the next refetch.
// An explicit `?env=` in the URL is already seeded by then and wins.
export function applyDefaultEnvironment(values: string[]): void {
  if (environmentDefaulted.value || values.length < 2) return
  environmentDefaulted.value = true
  if (environment.value) return
  if (values.includes('prod')) environment.value = 'prod'
}

// --- URL sync ---
// Context owns the `range` / `from` / `to` / `tenant` / `env` URL keys. Reads/writes are
// merge-preserve: only these keys are touched, everything else in location.search (q/svc/sev/...)
// is left alone.
const CONTEXT_KEYS = ['range', 'from', 'to', 'tenant', 'env']

export interface ParsedContext {
  timeRange: string | null
  customRange: CustomRange | null
  tenant: string | null
  environment: string | null
}

export function parseContext(search: string): ParsedContext {
  const p = new URLSearchParams(search ?? '')
  const from = Number(p.get('from'))
  const to = Number(p.get('to'))
  const hasCustom = p.has('from') && p.has('to') && Number.isFinite(from) && Number.isFinite(to)
  return {
    timeRange: p.get('range') || null,
    customRange: hasCustom ? { startMs: from, endMs: to } : null,
    tenant: p.get('tenant') || null,
    environment: p.get('env') || null,
  }
}

export function seedContextFromUrl(): void {
  if (typeof window === 'undefined') return
  const c = parseContext(window.location.search)
  if (c.timeRange) timeRange.value = c.timeRange
  customRange.value = c.customRange
  tenant.value = c.tenant
  environment.value = c.environment
}

// Merge-write ONLY the context keys into the live URL, preserving everything else (q/svc/sev/...).
// Exported so the router can re-run it after a bare navigation (router/index.js afterEach) —
// the watch below only fires on ref changes, so a route push with unchanged range/tenant/env
// needs this called explicitly to carry those keys onto the new path.
export function syncContextToUrl(): void {
  if (typeof window === 'undefined') return
  const p = new URLSearchParams(window.location.search)
  CONTEXT_KEYS.forEach((k) => p.delete(k))
  if (customRange.value) {
    p.set('from', String(customRange.value.startMs))
    p.set('to', String(customRange.value.endMs))
  } else if (timeRange.value) {
    p.set('range', timeRange.value)
  }
  if (tenant.value) p.set('tenant', tenant.value)
  if (environment.value) p.set('env', environment.value)
  replaceSearch(p)
}

let syncStarted = false
export function startContextUrlSync(): void {
  if (syncStarted || typeof window === 'undefined') return
  syncStarted = true
  watch([timeRange, customRange, tenant, environment], syncContextToUrl, { deep: true })
}

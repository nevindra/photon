// App-wide observability context: the selected time window + the active entity scope.
// The auth.js / theme.js module-singleton pattern (NOT Pinia). One source of truth for the
// time math that every view used to duplicate. URL sync lives in this file too (Task 2).
import { ref, computed, watch, type Ref, type ComputedRef } from 'vue'

// Same preset table every view used (LogsView/ServiceDetailView/...). Milliseconds.
export const RANGE_MS: Record<string, number> = {
  '5m': 3e5, '15m': 9e5, '30m': 18e5, '1h': 36e5, '3h': 108e5,
  '6h': 216e5, '12h': 432e5, '24h': 864e5, '7d': 6048e5,
}

export interface CustomRange {
  startMs: number
  endMs: number
}

export type ScopeType = 'service' | 'rumApp' | 'host' | 'monitor'

export interface Scope {
  type: ScopeType
  id: string
  label: string
}

// --- state (module singletons) ---
export const timeRange: Ref<string> = ref('30m')                    // preset key into RANGE_MS
export const customRange: Ref<CustomRange | null> = ref(null)        // absolute; wins over preset
export const scope: Ref<Scope | null> = ref(null)
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
export function setScope(s: Scope): void {
  scope.value = s
}
export function clearScope(): void {
  scope.value = null
}

// --- URL sync ---
// Context owns the `range` / `from` / `to` / `scope` URL keys. Reads/writes are merge-preserve:
// only these keys are touched, everything else in location.search (q/svc/sev/...) is left alone.
const CONTEXT_KEYS = ['range', 'from', 'to', 'scope']

export interface ParsedContext {
  timeRange: string | null
  customRange: CustomRange | null
  scope: Scope | null
}

export function parseContext(search: string): ParsedContext {
  const p = new URLSearchParams(search ?? '')
  const from = Number(p.get('from'))
  const to = Number(p.get('to'))
  const hasCustom = p.has('from') && p.has('to') && Number.isFinite(from) && Number.isFinite(to)
  const rawScope = p.get('scope')
  let parsedScope: Scope | null = null
  if (rawScope) {
    const i = rawScope.indexOf(':')
    if (i > 0) {
      const type = rawScope.slice(0, i) as ScopeType
      const id = rawScope.slice(i + 1)
      parsedScope = { type, id, label: id }
    }
  }
  return {
    timeRange: p.get('range') || null,
    customRange: hasCustom ? { startMs: from, endMs: to } : null,
    scope: parsedScope,
  }
}

export function seedContextFromUrl(): void {
  if (typeof window === 'undefined') return
  const c = parseContext(window.location.search)
  if (c.timeRange) timeRange.value = c.timeRange
  customRange.value = c.customRange
  scope.value = c.scope
}

// Merge-write ONLY the context keys into the live URL, preserving everything else (q/svc/sev/...).
// Exported so the router can re-run it after a bare navigation (router/index.js afterEach) —
// the watch below only fires on ref changes, so a route push with unchanged range/scope needs
// this called explicitly to carry those keys onto the new path.
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
  if (scope.value) p.set('scope', `${scope.value.type}:${scope.value.id}`)
  const qs = p.toString()
  window.history.replaceState(null, '', qs ? `?${qs}` : window.location.pathname)
}

let syncStarted = false
export function startContextUrlSync(): void {
  if (syncStarted || typeof window === 'undefined') return
  syncStarted = true
  watch([timeRange, customRange, scope], syncContextToUrl, { deep: true })
}

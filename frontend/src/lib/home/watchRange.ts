// The Watch Range — the short, deliberately bounded lookback `/home` judges over. It is NOT the
// app-wide Window from `lib/core/context.ts` and does not read it: a judgment of "wrong now"
// averaged over a week is a lie, so the range that produces it is capped by design. See
// docs/adr/0001-landing-page-is-an-attention-board.md.
//
// Drilling out of the board writes the Watch Range into the global time range first
// (`applyWatchRangeToWindow`), so the page you land on shows the same slice of time. The cap
// applies to the judging, not to the exploring.
//
// Same module-singleton shape as `context.ts` (NOT Pinia), including merge-preserving URL sync:
// this module owns exactly one key, `watch`, and never touches range/from/to/tenant.
import { ref, computed, watch as vueWatch, onUnmounted, type Ref, type ComputedRef } from 'vue'
import { replaceSearch } from '@/lib/core/historyUrl'
import { setTimeRange, RANGE_MS } from '@/lib/core/context'

/** Minutes to a few hours, never days. Every key is also a `RANGE_MS` preset, so drilling out
 *  can hand the same string straight to `setTimeRange`. */
export const WATCH_RANGES = ['5m', '15m', '1h', '6h'] as const
export type WatchRangeKey = (typeof WATCH_RANGES)[number]

export const DEFAULT_WATCH_RANGE: WatchRangeKey = '15m'

export function isWatchRange(v: string | null | undefined): v is WatchRangeKey {
  return !!v && (WATCH_RANGES as readonly string[]).includes(v)
}

// --- state -------------------------------------------------------------------------------------

export const watchRange: Ref<WatchRangeKey> = ref(DEFAULT_WATCH_RANGE)

// "Now", snapped to a whole minute. The board's queries poll every 15s on a stable key; snapping
// keeps that key from changing four times a minute and spawning a fresh cache entry each time.
const CLOCK_MS = 60_000
const snap = (ms: number): number => Math.floor(ms / CLOCK_MS) * CLOCK_MS
export const watchNowMs: Ref<number> = ref(snap(Date.now()))

// --- derived window ------------------------------------------------------------------------------

export const watchMs: ComputedRef<number> = computed(() => RANGE_MS[watchRange.value] ?? RANGE_MS[DEFAULT_WATCH_RANGE])
export const watchEndMs: ComputedRef<number> = computed(() => watchNowMs.value)
export const watchStartMs: ComputedRef<number> = computed(() => watchEndMs.value - watchMs.value)
// The equally-long range immediately before it — the only comparison basis the board has, and what
// Silence is detected against.
export const prevWatchEndMs: ComputedRef<number> = computed(() => watchStartMs.value)
export const prevWatchStartMs: ComputedRef<number> = computed(() => watchStartMs.value - watchMs.value)

const toNs = (ms: number): string => (BigInt(Math.round(ms)) * 1_000_000n).toString()
export const watchStartNs: ComputedRef<string> = computed(() => toNs(watchStartMs.value))
export const watchEndNs: ComputedRef<string> = computed(() => toNs(watchEndMs.value))
export const prevWatchStartNs: ComputedRef<string> = computed(() => toNs(prevWatchStartMs.value))
export const prevWatchEndNs: ComputedRef<string> = computed(() => toNs(prevWatchEndMs.value))

/** Human label for the "vs the previous N" note under the vital signs. */
export const watchLabel: ComputedRef<string> = computed(() => watchRange.value)

// --- actions -----------------------------------------------------------------------------------

export function setWatchRange(r: string): void {
  if (!isWatchRange(r)) return
  watchRange.value = r
  watchNowMs.value = snap(Date.now())
}

/** Re-anchor "now" — the clock tick, and the manual refresh. */
export function refreshWatchNow(): void {
  watchNowMs.value = snap(Date.now())
}

/**
 * Hand the Watch Range to the app-wide Window, so a drill-out lands on the same slice of time.
 * Call this immediately BEFORE `correlate()`, which reads the global context.
 */
export function applyWatchRangeToWindow(): void {
  setTimeRange(watchRange.value)
}

// --- URL sync ----------------------------------------------------------------------------------

const OWNED_KEY = 'watch'

export function parseWatchRange(search: string | null | undefined): WatchRangeKey | null {
  const v = new URLSearchParams(search ?? '').get(OWNED_KEY)
  return isWatchRange(v) ? v : null
}

export function seedWatchRangeFromUrl(): void {
  if (typeof window === 'undefined') return
  const v = parseWatchRange(window.location.search)
  if (v) watchRange.value = v
}

// Merge-write ONLY `watch`, leaving range/from/to/tenant (owned by context.ts) and any view keys
// alone. The default is omitted so a clean URL stays clean.
function syncWatchRangeToUrl(): void {
  if (typeof window === 'undefined') return
  const p = new URLSearchParams(window.location.search)
  p.delete(OWNED_KEY)
  if (watchRange.value !== DEFAULT_WATCH_RANGE) p.set(OWNED_KEY, watchRange.value)
  replaceSearch(p)
}

/**
 * Seed from the URL, keep it in sync, and advance "now" once a minute for as long as the board is
 * mounted. Scoped to the component (not a module-level interval) so leaving `/home` stops the clock.
 */
export function useWatchRange(): void {
  seedWatchRangeFromUrl()
  const stop = vueWatch(watchRange, syncWatchRangeToUrl)
  if (typeof window === 'undefined') return
  const timer = window.setInterval(refreshWatchNow, CLOCK_MS)
  onUnmounted(() => {
    window.clearInterval(timer)
    stop()
  })
}

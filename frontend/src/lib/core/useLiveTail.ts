import { ref, watch, onScopeDispose, toValue } from 'vue'
import type { Ref, MaybeRefOrGetter } from 'vue'
import { openLiveStream, type StreamGrain } from '@/lib/core/liveStream'

// Signals this composable knows how to tail. 'metrics' is never streamable (falls back to poll).
export type LiveTailGrain = 'logs' | 'spans' | 'metrics'

export type RefreshMode = 'manual' | '5s' | '30s' | 'live'

export type LiveTailStatus = 'idle' | 'reconnecting' | 'live' | 'lagged'

// A raw row as it arrives off the stream, before the synthetic `id` is stamped on.
export type LiveTailRow = Record<string, unknown>

// A row after stamping: same shape, `id` is now the collision-free `live-*` marker.
export type TaggedLiveTailRow = LiveTailRow & { id: string }

// Any row with an `id` field, streamed or from the frozen search-page baseline — the two id spaces
// (client-unique `live-*` strings vs. positional numbers) never overlap.
export type IdentifiedRow = Record<string, unknown> & { id: string | number }

// `onPoll` is overloaded by convention: false = stop polling, a number = poll interval ms,
// 'once' = do a single manual refetch now (view maps this to `query.refetch()`).
export type OnPoll = (value: number | false | 'once') => void

export interface UseLiveTailOptions {
  grain: LiveTailGrain
  query: MaybeRefOrGetter<string>
  bufferCap?: number
  streamable?: boolean
  onPoll?: OnPoll
}

export interface UseLiveTailReturn {
  rows: Ref<TaggedLiveTailRow[]>
  status: Ref<LiveTailStatus>
  newCount: Ref<number>
  paused: Ref<boolean>
  mode: Ref<RefreshMode>
  rate: Ref<number | null>
  setMode: (next: RefreshMode) => void
  refresh: () => void
  jumpToLatest: () => void
  setPaused: (v: boolean) => void
}

const POLL_MS: Record<'5s' | '30s', number> = { '5s': 5000, '30s': 30000 }

// Resolves a refresh mode (Manual / 5s / 30s / Live) to a data source and manages the
// streamed-row ring buffer with pause-on-scroll. Logs + spans stream via openLiveStream;
// metrics (streamable: false) fall back to fast polling. The three explorer views depend
// on this one composable so their live-tail behavior stays identical.
export function useLiveTail({ grain, query, bufferCap = 1000, streamable = true, onPoll }: UseLiveTailOptions): UseLiveTailReturn {
  const rows = ref<TaggedLiveTailRow[]>([])
  const status = ref<LiveTailStatus>('idle')
  const newCount = ref(0)
  const paused = ref(false)
  const mode = ref<RefreshMode>('manual')
  const rate = ref<number | null>(null)
  let handle: ReturnType<typeof openLiveStream> | null = null
  let pending: TaggedLiveTailRow[] = [] // rows buffered while paused
  // Client-unique, never-reset id counter for streamed rows. Composable-scoped (NOT reset in
  // openStream) so an EventSource auto-reconnect keeps minting fresh ids instead of restarting at
  // live-0 and re-colliding with rows already on screen or with the search page's positional ids.
  let uid = 0

  function closeStream(): void {
    if (handle) { handle.close(); handle = null }
    status.value = 'idle'
  }

  function prepend(incoming: TaggedLiveTailRow[]): void {
    // incoming arrives oldest-first within a flush; reverse so newest ends up on top.
    const next = [...incoming].reverse().concat(rows.value)
    rows.value = next.slice(0, bufferCap)
  }

  function onRows(incoming: LiveTailRow[]): void {
    if (!incoming?.length) return
    // Tag every streamed row with a collision-free `id` up front so BOTH branches (paused buffer
    // and live prepend) store tagged rows. Rewriting the synthetic `id` is safe: selection/drawer/
    // correlation use the row's own fields (trace_id / span_id / body / timestamp), not `id`.
    const tagged = incoming.map((r) => ({ ...r, id: `live-${uid++}` }))
    if (paused.value) { pending.push(...tagged); newCount.value += tagged.length; return }
    prepend(tagged)
  }

  function openStream(): void {
    closeStream()
    rows.value = []
    pending = []
    newCount.value = 0
    status.value = 'reconnecting'
    handle = openLiveStream({
      // openStream() only runs when `streamable` is true, which the caller guarantees is
      // never the case for the 'metrics' grain — so narrowing to StreamGrain is sound.
      grain: grain as StreamGrain, query: toValue(query),
      onRows,
      onLag: () => { status.value = 'lagged' },
      onRate: (r: number) => { rate.value = r },
      onStatus: (s: LiveTailStatus) => { status.value = s },
    })
  }

  function setMode(next: RefreshMode): void {
    mode.value = next
    if (next === 'live' && streamable) { onPoll?.(false); openStream() }
    else if (next === 'live') { closeStream(); onPoll?.(2000) }      // metrics: fast poll
    else if (next === '5s' || next === '30s') { closeStream(); onPoll?.(POLL_MS[next]) }
    else { closeStream(); onPoll?.(false) }                          // manual
  }

  function setPaused(v: boolean): void { paused.value = v }

  function jumpToLatest(): void {
    if (pending.length) { prepend(pending); pending = [] }
    newCount.value = 0
    paused.value = false
  }

  function refresh(): void { onPoll?.('once') } // view maps 'once' to a manual query.refetch()

  // Re-open the stream when the query changes while live.
  watch(() => toValue(query), () => { if (mode.value === 'live' && streamable) openStream() })

  onScopeDispose(closeStream)

  return { rows, status, newCount, paused, mode, rate, setMode, refresh, jumpToLatest, setPaused }
}

// Merge streamed rows (client-unique `live-*` ids) on TOP of the frozen search-page baseline
// (positional numeric ids) so entering Live keeps the already-loaded rows visible and prepends new
// streamed rows as they arrive. Dedup by id defensively; the two id spaces never overlap, so no
// baseline row is wrongly dropped. `cap` bounds the rendered DOM.
export function mergeLiveRows(streamed: IdentifiedRow[], base?: IdentifiedRow[] | null, cap = 1000): IdentifiedRow[] {
  const seen = new Set<string>()
  const out: IdentifiedRow[] = []
  for (const r of streamed) { const k = String(r.id); if (!seen.has(k)) { seen.add(k); out.push(r) } }
  for (const r of (base ?? [])) { const k = String(r.id); if (!seen.has(k)) { seen.add(k); out.push(r) } }
  return out.slice(0, cap)
}

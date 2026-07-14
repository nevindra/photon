import { hydrateStreamRows } from '@/lib/core/api'

// Which per-signal stream is being opened — matches the `:grain` path segment of `/api/stream/:grain`
// and picks the hydration shape (BigInt nanos) in hydrateStreamRows.
export type StreamGrain = 'logs' | 'spans'

// EventSource connection status as surfaced to the UI (see useLiveTail's `status` ref).
export type LiveStreamStatus = 'live' | 'reconnecting'

// A single streamed row after hydration into the SAME UI shape (BigInt nanos) the search path
// produces (see hydrateStreamRows in api.ts). The exact fields depend on the grain (log vs span
// row), so this stays a loose record rather than a full log/span type.
export type StreamRow = Record<string, unknown>

export interface OpenLiveStreamOptions {
  grain: StreamGrain
  query?: string | null
  onRows?: (rows: StreamRow[]) => void
  onLag?: (skipped: number) => void
  onRate?: (matchedPerSec: number) => void
  onStatus?: (status: LiveStreamStatus) => void
}

export interface LiveStreamHandle {
  close: () => void
}

// Thin EventSource wrapper: the "stream source" behind useLiveTail for logs + spans.
// Best-effort; the browser auto-reconnects on error (status → 'reconnecting').
export function openLiveStream({ grain, query, onRows, onLag, onRate, onStatus }: OpenLiveStreamOptions): LiveStreamHandle {
  const url = `/api/stream/${grain}?q=${encodeURIComponent(query ?? '')}`
  const es = new EventSource(url, { withCredentials: true })

  es.addEventListener('open', () => onStatus?.('live'))
  // Hydrate to the SAME BigInt-nanos shape the search path uses, so streamed and searched rows are
  // interchangeable in the merged table (see hydrateStreamRows in api.js).
  es.addEventListener('rows', (e: MessageEvent) => { try { onRows?.(hydrateStreamRows(grain, JSON.parse(e.data))) } catch {} })
  es.addEventListener('lag', (e: MessageEvent) => { try { onLag?.(JSON.parse(e.data).skipped) } catch {} })
  es.addEventListener('rate', (e: MessageEvent) => { try { onRate?.(JSON.parse(e.data).matched_per_sec) } catch {} })
  es.addEventListener('error', () => onStatus?.('reconnecting'))

  return { close() { es.close() } }
}

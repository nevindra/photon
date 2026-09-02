// Turns `/api/data/storage`'s raw merge counters into the one sentence the Storage tab shows.
//
// `merge_once` consolidates small Parquet files into bigger ones. When it stops, nothing breaks
// loudly: ingest still acks, search still answers, the page still renders. The only visible symptom
// is a file count that climbs by ~1,440 per signal per day and queries that get slower every week.
// A production node ran 12 days like that and the only trace of it was a line on stderr.
//
// So the judgment here is deliberately blunt. A merge always re-selects the OLDEST small files, so
// one bad input fails identically on every pass — a non-zero streak is not a blip that will clear
// itself, it is a stall that lasts until someone looks. One failed pass is already worth a warning;
// a few minutes of them is an error.
import type { CompactionHealth } from '@/lib/core/api'

/** Failures in a row past which a stall stops being "just started" (~5 min at one pass per 10 s). */
export const STALLED_FAILURES = 30

export type CompactionTone = 'success' | 'warning' | 'error' | 'neutral'

export interface CompactionVerdict {
  signal: string
  tone: CompactionTone
  /** Pill text: "Merging", "Failing", "Stalled", "Unknown". */
  label: string
  /** One line under it, or `''` when the pill says everything. */
  detail: string
  consecutiveFailures: number
  lastOkMs: number | null
  lastError: string | null
}

/** `true` for the states worth pulling out into a page-level banner. */
export function isDegraded(v: CompactionVerdict): boolean {
  return v.tone === 'warning' || v.tone === 'error'
}

/** Judge one signal. `nowMs` is injectable so the "stalled for" text is testable. */
export function judge(h: CompactionHealth, nowMs: number = Date.now()): CompactionVerdict {
  const base = {
    signal: h.signal,
    consecutiveFailures: h.consecutive_failures,
    lastOkMs: h.last_ok_ms,
    lastError: h.last_error,
  }
  if (h.consecutive_failures === 0) {
    // No pass has ever succeeded AND none has failed: the loop has not run yet. Young node,
    // nothing to report — calling that "healthy" would be a claim we have not earned.
    if (h.last_ok_ms == null) {
      return { ...base, tone: 'neutral', label: 'Unknown', detail: 'No merge pass yet' }
    }
    return { ...base, tone: 'success', label: 'Merging', detail: `Last merged ${ago(h.last_ok_ms, nowMs)}` }
  }
  const stalled = h.consecutive_failures >= STALLED_FAILURES
  const since = h.last_ok_ms == null ? 'never merged since start-up' : `stalled since ${ago(h.last_ok_ms, nowMs)}`
  return {
    ...base,
    tone: stalled ? 'error' : 'warning',
    label: stalled ? 'Stalled' : 'Failing',
    detail: `${h.consecutive_failures} failed ${h.consecutive_failures === 1 ? 'pass' : 'passes'} in a row, ${since}`,
  }
}

/**
 * Judge the whole payload. `undefined` in, `[]` out — a node with no compactor attached says
 * nothing rather than reporting three healthy signals it cannot see.
 */
export function judgeAll(list: CompactionHealth[] | undefined, nowMs: number = Date.now()): CompactionVerdict[] {
  return (list ?? []).map((h) => judge(h, nowMs))
}

// "4m ago" / "3h ago" / "12d ago". Days matter here: the failure mode this exists for is measured
// in days, and an "even 288h ago" would make a reader do arithmetic to feel the size of it.
function ago(ms: number, nowMs: number): string {
  const secs = Math.max(0, Math.round((nowMs - ms) / 1000))
  if (secs < 60) return `${secs}s ago`
  const mins = Math.round(secs / 60)
  if (mins < 60) return `${mins}m ago`
  const hours = Math.round(mins / 60)
  if (hours < 48) return `${hours}h ago`
  return `${Math.round(hours / 24)}d ago`
}

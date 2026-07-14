// Bucket log records into a fixed number of equal-width time buckets across an
// arbitrary [startMs, endMs] window. Each record's `timestamp` is a BigInt of epoch
// nanoseconds (the hydrated UI shape).
//
// Unlike `mock.bucketize` (hard-wired to the mock's fixed 30-minute window anchored at
// module load), this buckets over the caller-supplied window so the histogram tracks the
// selected time range and any drag-to-zoom range. Records outside the window are clamped
// into the edge buckets (in practice the search request already bounds results to the
// window). Output shape matches VolumeHistogram's `buckets` prop.

const MS = 1_000_000n // ns per ms

/** The five recognized log severities that get their own bucket slot. */
export type SeverityKey = 'debug' | 'info' | 'warn' | 'error' | 'fatal'

const SEVERITY_KEYS: string[] = ['debug', 'info', 'warn', 'error', 'fatal']

/** Minimal shape `bucketize` needs from a hydrated log record. */
export interface HistogramRecord {
  /** Epoch nanoseconds. */
  timestamp: bigint
  /** Free-form severity string; anything outside `SeverityKey` is folded into `info`. */
  severity?: string
}

/** One time-bucket's per-severity + total counts. Matches VolumeHistogram's `buckets` prop shape. */
export type HistogramBucket = Record<SeverityKey, number> & { total: number }

function emptyBucket(): HistogramBucket {
  return { debug: 0, info: 0, warn: 0, error: 0, fatal: 0, total: 0 }
}

export function bucketize(
  records: HistogramRecord[] | null | undefined,
  startMs: number,
  endMs: number,
  buckets = 48,
): HistogramBucket[] {
  const n = Math.max(1, buckets)
  const out = Array.from({ length: n }, emptyBucket)
  const span = endMs - startMs
  if (!(span > 0)) return out
  const width = span / n
  for (const r of records ?? []) {
    const ms = Number(r.timestamp / MS)
    let i = Math.floor((ms - startMs) / width)
    if (i < 0) i = 0
    if (i >= n) i = n - 1
    const key = (SEVERITY_KEYS.includes(r.severity ?? '') ? r.severity : 'info') as SeverityKey
    out[i][key]++
    out[i].total++
  }
  return out
}

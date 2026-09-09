// "Did it work?" — the judgment a deploy or a canary gets from the telemetry Photon already has.
// Pure: two sets of RED rows (the window before an apply, the window after it) in, one verdict
// per service out, plus the overall call the gate shows. Thresholds are deliberately blunt: this
// is a prompt to look, not an SLO.
import type { RedRow } from '@/lib/core/api'

export interface ServiceImpact {
  service: string
  /** Requests per second before → after. */
  rateBefore: number
  rateAfter: number
  errorRateBefore: number
  errorRateAfter: number
  /** p99 in nanoseconds. */
  p99Before: number
  p99After: number
  /** `null` = not enough traffic on either side to say anything. */
  verdict: 'better' | 'same' | 'worse' | null
  reasons: string[]
}

export type Verdict = 'safe' | 'look' | 'nosignal'

export interface ImpactSummary {
  services: ServiceImpact[]
  verdict: Verdict
  headline: string
}

// Below this many requests in a window a rate is noise, not a reading.
export const MIN_REQUESTS = 20
// Error rate must rise by this much (absolute) or p99 by this factor before it counts.
export const ERROR_RATE_RISE = 0.01
export const P99_RISE_FACTOR = 1.3
export const P99_FALL_FACTOR = 0.77

function sumRows(rows: RedRow[], service: string): { count: number; rate: number; errorRate: number; p99: number } {
  const mine = rows.filter((r) => r.service === service)
  const count = mine.reduce((n, r) => n + r.count, 0)
  const rate = mine.reduce((n, r) => n + r.rate, 0)
  const errors = mine.reduce((n, r) => n + r.error_count, 0)
  const p99 = mine.reduce((m, r) => Math.max(m, Number(r.p99 ?? 0)), 0)
  return { count, rate, errorRate: count ? errors / count : 0, p99 }
}

export function judgeService(service: string, before: RedRow[], after: RedRow[]): ServiceImpact {
  const b = sumRows(before, service)
  const a = sumRows(after, service)
  const base: ServiceImpact = { service, rateBefore: b.rate, rateAfter: a.rate, errorRateBefore: b.errorRate, errorRateAfter: a.errorRate, p99Before: b.p99, p99After: a.p99, verdict: null, reasons: [] }
  if (a.count < MIN_REQUESTS) return { ...base, reasons: [a.count === 0 ? 'no requests since the apply' : `only ${a.count} requests since the apply`] }
  const reasons: string[] = []
  let worse = false
  let better = false
  if (a.errorRate - b.errorRate >= ERROR_RATE_RISE) {
    worse = true
    reasons.push(`error rate ${(b.errorRate * 100).toFixed(1)}% → ${(a.errorRate * 100).toFixed(1)}%`)
  } else if (b.errorRate - a.errorRate >= ERROR_RATE_RISE && b.count >= MIN_REQUESTS) {
    better = true
    reasons.push(`error rate ${(b.errorRate * 100).toFixed(1)}% → ${(a.errorRate * 100).toFixed(1)}%`)
  }
  if (b.count >= MIN_REQUESTS && b.p99 > 0) {
    if (a.p99 >= b.p99 * P99_RISE_FACTOR) {
      worse = true
      reasons.push(`p99 up ${Math.round((a.p99 / b.p99 - 1) * 100)}%`)
    } else if (a.p99 <= b.p99 * P99_FALL_FACTOR) {
      better = true
      reasons.push(`p99 down ${Math.round((1 - a.p99 / b.p99) * 100)}%`)
    }
  }
  if (b.count >= MIN_REQUESTS && a.rate < b.rate * 0.5) {
    worse = true
    reasons.push(`traffic halved (${b.rate.toFixed(1)}/s → ${a.rate.toFixed(1)}/s)`)
  }
  if (!reasons.length) reasons.push(b.count < MIN_REQUESTS ? 'no baseline before the apply; nothing alarming since' : 'error rate and p99 within range of the baseline')
  return { ...base, verdict: worse ? 'worse' : better ? 'better' : 'same', reasons }
}

export function judgeImpact(services: string[], before: RedRow[], after: RedRow[]): ImpactSummary {
  const rows = services.map((s) => judgeService(s, before, after))
  const judged = rows.filter((r) => r.verdict != null)
  if (!judged.length) return { services: rows, verdict: 'nosignal', headline: services.length ? 'No traffic to judge yet' : 'No APM services to judge' }
  const worse = judged.filter((r) => r.verdict === 'worse')
  if (worse.length) return { services: rows, verdict: 'look', headline: `Look closer: ${worse.map((r) => r.service).join(', ')} got worse` }
  const better = judged.filter((r) => r.verdict === 'better')
  return { services: rows, verdict: 'safe', headline: better.length ? `Looks good — ${better.map((r) => r.service).join(', ')} improved` : 'Looks good — nothing moved outside the baseline' }
}

export const VERDICT_META: Record<Verdict, { label: string; tone: 'success' | 'error' | 'neutral' }> = {
  safe: { label: 'Looks safe', tone: 'success' },
  look: { label: 'Look closer', tone: 'error' },
  nosignal: { label: 'No signal yet', tone: 'neutral' },
}

/** The two windows around an apply: `windowMs` before it, and from it to now (capped at windowMs). */
export function impactWindows(appliedMs: number, nowMs: number, windowMs: number): { before: [number, number]; after: [number, number] } {
  const afterEnd = Math.min(nowMs, appliedMs + windowMs)
  return { before: [appliedMs - windowMs, appliedMs], after: [appliedMs, Math.max(appliedMs + 1000, afterEnd)] }
}

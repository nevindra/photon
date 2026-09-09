import { describe, it, expect } from 'vitest'
import { judgeService, judgeImpact, impactWindows } from './impact'
import type { RedRow } from '@/lib/core/api'

const row = (service: string, count: number, errors: number, p99ms: number, rate = count / 60): RedRow =>
  ({ service, operation: null, count, rate, error_count: errors, error_rate: count ? errors / count : 0, p50: '0', p90: '0', p99: String(p99ms * 1e6) }) as RedRow

describe('deploy impact', () => {
  it('says nothing without traffic after the apply', () => {
    const r = judgeService('api', [row('api', 600, 3, 120)], [row('api', 5, 0, 100)])
    expect(r.verdict).toBeNull()
    expect(r.reasons[0]).toContain('only 5 requests')
  })
  it('flags an error-rate rise and a p99 rise', () => {
    expect(judgeService('api', [row('api', 600, 3, 120)], [row('api', 600, 30, 125)]).verdict).toBe('worse')
    expect(judgeService('api', [row('api', 600, 3, 120)], [row('api', 600, 3, 200)]).verdict).toBe('worse')
    expect(judgeService('api', [row('api', 600, 3, 120)], [row('api', 600, 3, 125)]).verdict).toBe('same')
    expect(judgeService('api', [row('api', 600, 30, 200)], [row('api', 600, 2, 120)]).verdict).toBe('better')
  })
  it('flags traffic falling off a cliff', () => {
    const r = judgeService('api', [row('api', 600, 0, 120)], [row('api', 100, 0, 120)])
    expect(r.verdict).toBe('worse')
    expect(r.reasons[0]).toContain('halved')
  })
  it('rolls services up into one call', () => {
    const before = [row('api', 600, 3, 120), row('web', 900, 0, 50)]
    expect(judgeImpact(['api', 'web'], before, [row('api', 600, 3, 120), row('web', 900, 0, 50)]).verdict).toBe('safe')
    expect(judgeImpact(['api', 'web'], before, [row('api', 600, 60, 120), row('web', 900, 0, 50)]).headline).toContain('api')
    expect(judgeImpact(['api'], before, []).verdict).toBe('nosignal')
    expect(judgeImpact([], before, []).headline).toContain('No APM services')
  })
  it('builds the before/after windows capped at now', () => {
    const w = impactWindows(1_000_000, 1_300_000, 900_000)
    expect(w.before).toEqual([100_000, 1_000_000])
    expect(w.after).toEqual([1_000_000, 1_300_000])
  })
})

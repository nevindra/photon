import { describe, it, expect } from 'vitest'
import { bucketize } from '@/lib/core/histogram'

const MS = 1_000_000n
// Build a hydrated-shape record whose timestamp sits at `ms` (epoch milliseconds).
const rec = (ms, severity = 'info') => ({ timestamp: BigInt(ms) * MS, severity })

describe('bucketize', () => {
  it('returns `buckets` empty buckets with the full shape', () => {
    const out = bucketize([], 0, 1000, 4)
    expect(out).toHaveLength(4)
    expect(out[0]).toEqual({ debug: 0, info: 0, warn: 0, error: 0, fatal: 0, total: 0 })
  })

  it('places records into the correct equal-width bucket', () => {
    // window [0, 1000ms] over 4 buckets => widths [0,250) [250,500) [500,750) [750,1000]
    const out = bucketize([rec(10), rec(300), rec(300), rec(900)], 0, 1000, 4)
    expect(out.map((b) => b.total)).toEqual([1, 2, 0, 1])
  })

  it('tallies per-severity and total', () => {
    const out = bucketize(
      [rec(100, 'info'), rec(150, 'error'), rec(180, 'fatal')],
      0,
      1000,
      1,
    )
    expect(out[0]).toEqual({ debug: 0, info: 1, warn: 0, error: 1, fatal: 1, total: 3 })
  })

  it('clamps records outside the window into the edge buckets', () => {
    const out = bucketize([rec(-500), rec(5000)], 0, 1000, 2)
    expect(out[0].total).toBe(1)
    expect(out[1].total).toBe(1)
  })

  it('is safe for a zero/negative-width window', () => {
    expect(bucketize([rec(100)], 500, 500, 3).every((b) => b.total === 0)).toBe(true)
    expect(bucketize([rec(100)], 500, 100, 3)).toHaveLength(3)
  })

  it('defaults to 48 buckets', () => {
    expect(bucketize([], 0, 1000)).toHaveLength(48)
  })

  it('treats unknown severities as info', () => {
    const out = bucketize([rec(100, 'trace')], 0, 1000, 1)
    expect(out[0].info).toBe(1)
    expect(out[0].total).toBe(1)
  })
})

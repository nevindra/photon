import { describe, it, expect } from 'vitest'
import { curatedQuickStarts, presetsForType } from '@/lib/metrics/quickStarts'

describe('curatedQuickStarts', () => {
  it('surfaces a curated card only when a matching metric exists (dot or underscore spelling)', () => {
    const cards = curatedQuickStarts([{ name: 'http_requests_total' }, { name: 'custom.metric' }])
    expect(cards.some((c) => c.metric === 'http_requests_total' && c.agg === 'rate')).toBe(true)
  })
  it('returns nothing for a catalog with no known metrics', () => {
    expect(curatedQuickStarts([{ name: 'my.custom.gauge' }])).toEqual([])
  })
})

describe('presetsForType', () => {
  it('offers rate/increase for a monotonic sum but not for a non-monotonic one', () => {
    expect(presetsForType('sum', true)).toContain('rate')
    expect(presetsForType('sum', false)[0]).toBe('sum')
  })
  it('offers percentiles for a histogram and nothing for an unknown type', () => {
    expect(presetsForType('histogram', null)).toEqual(['p99', 'p90', 'p50', 'count'])
    expect(presetsForType('mystery', null)).toEqual([])
  })
})

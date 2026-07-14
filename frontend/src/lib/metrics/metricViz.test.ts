import { describe, it, expect } from 'vitest'
import {
  ALL_VIZ, DEFAULT_VIZ, availableViz, parseViz, serializeViz, statSummary, seriesToBuckets,
} from '@/lib/metrics/metricViz'

describe('viz registry', () => {
  it('lists the seven viz ids', () => {
    expect(ALL_VIZ.map((v) => v.id)).toEqual(['line', 'area', 'stacked', 'bar', 'stacked-bar', 'stat', 'table'])
  })
  it('gates stat and bar to a single series; stacked-bar stays available', () => {
    expect(availableViz({ seriesCount: 1 })).toEqual(expect.arrayContaining(['stat', 'bar']))
    const multi = availableViz({ seriesCount: 3 })
    expect(multi).not.toContain('stat')
    expect(multi).not.toContain('bar')
    expect(multi).toContain('stacked-bar')
  })
})

describe('viz url codec', () => {
  it('parses a valid id and defaults unknown/empty to line', () => {
    expect(parseViz('bar')).toBe('bar')
    expect(parseViz('nonsense')).toBe(DEFAULT_VIZ)
    expect(parseViz(null)).toBe(DEFAULT_VIZ)
  })
  it('serializes to "" for the default (so it is omitted from the URL)', () => {
    expect(serializeViz('line')).toBe('')
    expect(serializeViz('bar')).toBe('bar')
  })
})

describe('statSummary', () => {
  it('reports the latest value and its delta vs the window mean', () => {
    const s = [{ labels: {}, points: [{ t: 1, v: 10 }, { t: 2, v: 20 }, { t: 3, v: 30 }] }]
    const r = statSummary(s)
    expect(r.hero).toBe(30)
    expect(r.dir).toBe('up') // 30 vs mean 20
  })
  it('is null-safe for an empty series', () => {
    expect(statSummary([]).hero).toBeNull()
  })
})

describe('seriesToBuckets', () => {
  it('maps ns-string timestamps to one ms bucket per t with a segment per series', () => {
    const buckets = seriesToBuckets([
      { labels: { svc: 'a' }, points: [{ t: '1000000', v: 1 }] }, // 1e6 ns → 1 ms
      { labels: { svc: 'b' }, points: [{ t: '1000000', v: 2 }] },
    ])
    expect(buckets).toHaveLength(1)
    expect(buckets[0].t).toBe(1)
    expect(buckets[0].segments.map((s) => s.value)).toEqual([1, 2])
  })
})

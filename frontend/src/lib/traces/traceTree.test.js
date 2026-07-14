import { describe, it, expect } from 'vitest'
import { buildTrace, isSpanError, mergeIntervals, pct, getTraceTree } from '@/lib/traces/traceTree'

// Minimal span factory. start/end are BigInt nanos, duration a Number.
function span(id, parent, start, end, extra = {}) {
  return {
    span_id: id,
    parent_span_id: parent,
    start_time_nanos: BigInt(start),
    end_time_nanos: end == null ? null : BigInt(end),
    duration_nanos: end == null ? null : end - start,
    name: `op-${id}`,
    service: extra.service ?? 'api',
    status_code: extra.status_code ?? 0,
    ...extra,
  }
}

describe('pct', () => {
  it('is a clamped percentage', () => {
    expect(pct(50n, 100n)).toBe(50)
    expect(pct(0n, 100n)).toBe(0)
    expect(pct(200n, 100n)).toBe(100)
  })
  it('returns 0 when the whole is zero', () => {
    expect(pct(10n, 0n)).toBe(0)
  })
})

describe('isSpanError', () => {
  it('is true only for OTLP status code 2', () => {
    expect(isSpanError({ status_code: 2 })).toBe(true)
    expect(isSpanError({ status_code: 1 })).toBe(false)
    expect(isSpanError({ status_code: 0 })).toBe(false)
    expect(isSpanError(null)).toBe(false)
  })
})

describe('mergeIntervals', () => {
  it('returns [] for empty input', () => {
    expect(mergeIntervals([])).toEqual([])
  })

  it('passes a single interval through unchanged', () => {
    expect(mergeIntervals([[10n, 20n]])).toEqual([[10n, 20n]])
  })

  it('merges overlapping intervals', () => {
    expect(
      mergeIntervals([
        [0n, 10n],
        [5n, 15n],
      ]),
    ).toEqual([[0n, 15n]])
  })

  it('merges adjacent/touching intervals (prev end === next start)', () => {
    expect(
      mergeIntervals([
        [0n, 10n],
        [10n, 20n],
      ]),
    ).toEqual([[0n, 20n]])
  })

  it('merges a nested interval into its outer interval', () => {
    expect(
      mergeIntervals([
        [0n, 100n],
        [10n, 20n],
      ]),
    ).toEqual([[0n, 100n]])
  })

  it('sorts unsorted input before merging', () => {
    expect(
      mergeIntervals([
        [50n, 60n],
        [0n, 10n],
        [20n, 30n],
      ]),
    ).toEqual([
      [0n, 10n],
      [20n, 30n],
      [50n, 60n],
    ])
  })

  it('drops zero-width and inverted intervals', () => {
    expect(
      mergeIntervals([
        [10n, 10n], // zero-width
        [5n, 3n], // inverted (start >= end)
        [0n, 5n],
      ]),
    ).toEqual([[0n, 5n]])
  })
})

describe('buildTrace', () => {
  it('builds a tree with depth, offsets, and trace bounds', () => {
    const t = buildTrace([
      span('root', null, 1000, 3000),
      span('child', 'root', 1500, 2500),
      span('grand', 'child', 1800, 2000),
    ])
    expect(t.roots.map((n) => n.id)).toEqual(['root'])
    expect(t.startNs).toBe(1000n)
    expect(t.endNs).toBe(3000n)
    expect(t.durationNs).toBe(2000n)
    const child = t.nodes.get('child')
    expect(child.depth).toBe(1)
    expect(child.offsetNs).toBe(500n) // 1500 - 1000
    expect(t.nodes.get('grand').depth).toBe(2)
    expect(t.spanCount).toBe(3)
  })

  it('treats an orphan (missing parent) as a root', () => {
    const t = buildTrace([
      span('root', null, 1000, 2000),
      span('orphan', 'ghost', 1200, 1800), // parent not present
    ])
    expect(t.roots.map((n) => n.id).sort()).toEqual(['orphan', 'root'])
  })

  it('does not infinite-loop on a cycle', () => {
    const t = buildTrace([
      span('a', 'b', 1000, 2000),
      span('b', 'a', 1000, 2000),
    ])
    // Both spans are represented exactly once in the flat list.
    expect(t.flat.length).toBe(2)
    expect(new Set(t.flat.map((n) => n.id))).toEqual(new Set(['a', 'b']))
  })

  it('marks the critical path (root then last-ending child chain)', () => {
    const t = buildTrace([
      span('root', null, 0, 1000),
      span('fast', 'root', 100, 300),
      span('slow', 'root', 100, 900), // ends last → on critical path
    ])
    expect(t.criticalPath.has('root')).toBe(true)
    expect(t.criticalPath.has('slow')).toBe(true)
    expect(t.criticalPath.has('fast')).toBe(false)
    expect(t.nodes.get('slow').onCriticalPath).toBe(true)
  })

  it('flags clock skew when a child starts before its parent', () => {
    const t = buildTrace([
      span('root', null, 1000, 2000),
      span('skewed', 'root', 900, 1500), // starts before parent
    ])
    expect(t.nodes.get('skewed').hasClockSkew).toBe(true)
    // Offset never goes negative (clamped to trace start).
    expect(t.nodes.get('skewed').offsetNs).toBe(0n)
  })

  it('propagates subtree errors and counts them', () => {
    const t = buildTrace([
      span('root', null, 0, 1000),
      span('bad', 'root', 100, 400, { status_code: 2 }),
    ])
    expect(t.errorCount).toBe(1)
    expect(t.nodes.get('root').subtreeHasError).toBe(true)
    expect(t.nodes.get('bad').isError).toBe(true)
  })

  it('derives end from duration when end_time is null', () => {
    const t = buildTrace([
      { span_id: 'x', parent_span_id: null, start_time_nanos: 100n, end_time_nanos: null, duration_nanos: 400, name: 'x', service: 'api', status_code: 0 },
    ])
    expect(t.nodes.get('x').durationNs).toBe(400n)
    expect(t.endNs).toBe(500n)
  })

  describe('selfTimeNs / childCovered', () => {
    it('gives a leaf node selfTimeNs equal to its durationNs', () => {
      const t = buildTrace([span('root', null, 0, 1000)])
      const root = t.nodes.get('root')
      expect(root.selfTimeNs).toBe(1000n)
      expect(root.childCovered).toEqual([])
    })

    it('subtracts a single child interval from the parent self-time', () => {
      const t = buildTrace([
        span('root', null, 0, 1000),
        span('child', 'root', 200, 500),
      ])
      const root = t.nodes.get('root')
      expect(root.childCovered).toEqual([[200n, 500n]])
      expect(root.selfTimeNs).toBe(700n) // 1000 - (500-200)
      expect(t.nodes.get('child').selfTimeNs).toBe(300n) // leaf
    })

    it('subtracts the union of two overlapping children only once', () => {
      const t = buildTrace([
        span('root', null, 0, 1000),
        span('a', 'root', 100, 400),
        span('b', 'root', 300, 600), // overlaps a from 300-400
      ])
      const root = t.nodes.get('root')
      // union covered = [100, 600) => 500 total, NOT (300 + 300) = 600
      expect(root.childCovered).toEqual([[100n, 600n]])
      expect(root.selfTimeNs).toBe(500n)
    })

    it('clamps a clock-skew child (starts before / ends after the parent) so self-time never goes negative', () => {
      const t = buildTrace([
        span('root', null, 1000, 2000),
        span('skewed', 'root', 900, 2500), // starts before, ends after parent
      ])
      const root = t.nodes.get('root')
      expect(root.childCovered).toEqual([[1000n, 2000n]]) // clamped to parent bounds
      expect(root.selfTimeNs).toBe(0n)
      expect(root.selfTimeNs >= 0n).toBe(true)
    })
  })

  describe('serviceSelfTime', () => {
    it('partitions self-time by service, sorted descending by selfNs, ties broken by service name', () => {
      const t = buildTrace([
        span('root', null, 0, 1000, { service: 'gateway' }),
        span('a', 'root', 0, 300, { service: 'auth' }),
        span('b', 'root', 300, 900, { service: 'billing' }),
      ])
      // root self = 1000 - (900-0) = 100; auth self = 300; billing self = 600
      expect(t.serviceSelfTime).toEqual([
        { service: 'billing', selfNs: 600n },
        { service: 'auth', selfNs: 300n },
        { service: 'gateway', selfNs: 100n },
      ])
    })

    it('breaks ties in selfNs by ascending service name', () => {
      const t = buildTrace([
        span('root', null, 0, 1000, { service: 'zeta' }),
        span('a', 'root', 0, 200, { service: 'alpha' }),
        span('b', 'root', 800, 1000, { service: 'beta' }),
      ])
      // root self = 1000 - 400 = 600; alpha self = 200; beta self = 200 (tie, alpha < beta)
      expect(t.serviceSelfTime).toEqual([
        { service: 'zeta', selfNs: 600n },
        { service: 'alpha', selfNs: 200n },
        { service: 'beta', selfNs: 200n },
      ])
    })

    it('sums self-time across multiple spans of the same service without double-counting nested same-service spans', () => {
      const t = buildTrace([
        span('root', null, 0, 1000, { service: 'api' }),
        span('mid', 'root', 100, 900, { service: 'api' }), // same service as root, nested
        span('leaf', 'mid', 200, 400, { service: 'db' }),
      ])
      // root self = 1000 - (900-100) = 200; mid self = 800 - (400-200) = 600; leaf self = 200
      // api total = 200 + 600 = 800 (each node's self-time already excludes covered child time)
      const api = t.serviceSelfTime.find((s) => s.service === 'api')
      const db = t.serviceSelfTime.find((s) => s.service === 'db')
      expect(api.selfNs).toBe(800n)
      expect(db.selfNs).toBe(200n)
    })

    it('includes every service that appears even when its total self-time is 0n', () => {
      const t = buildTrace([
        span('root', null, 0, 1000, { service: 'gateway' }),
        span('wrapper', 'root', 0, 1000, { service: 'wrapper-svc' }), // exactly covers root
        span('leaf', 'wrapper', 0, 1000, { service: 'db' }),
      ])
      const wrapper = t.serviceSelfTime.find((s) => s.service === 'wrapper-svc')
      expect(wrapper).toBeDefined()
      expect(wrapper.selfNs).toBe(0n)
    })
  })
})

describe('getTraceTree (memo)', () => {
  const spans = [
    { span_id: '1', parent_span_id: '', service: 's', name: 'root', start_time_nanos: '0', end_time_nanos: '100', status_code: 0 },
  ]
  it('returns an identical object for the same array reference', () => {
    const a = getTraceTree(spans)
    const b = getTraceTree(spans)
    expect(a).toBe(b) // same reference — no rebuild
  })
  it('rebuilds for a different array reference', () => {
    const a = getTraceTree(spans)
    const c = getTraceTree([...spans]) // new array, same content
    expect(c).not.toBe(a)
  })
  it('produces the same shape as buildTrace', () => {
    const t = getTraceTree(spans)
    expect(t.rootName).toBe('root')
    expect(t.spanCount).toBe(1)
  })
})

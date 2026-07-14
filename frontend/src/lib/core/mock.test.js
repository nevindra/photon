import { describe, it, expect } from 'vitest'
import {
  SERVICES,
  SEVERITY_KEYS,
  queryMock,
  mockFields,
  mockFacet,
  mockHistogram,
  mockSearchTraces,
  mockSearchSpans,
  mockTracesFields,
  mockTracesFacet,
  mockTracesHistogram,
  mockTracesLatency,
} from '@/lib/core/mock'

const WIDE = { start: '0', end: '9223372036854775807' }

describe('mock aggregation shapes', () => {
  it('queryMock returns the search envelope', () => {
    const res = queryMock({ query: '', ...WIDE, limit: 500 })
    expect(Array.isArray(res.rows)).toBe(true)
    expect(typeof res.matched_count).toBe('number')
    expect(typeof res.elapsed_ms).toBe('number')
    expect(res.matched_count).toBeGreaterThanOrEqual(res.rows.length)
  })

  it('mockFields returns typed field entries', () => {
    const fields = mockFields()
    expect(fields.some((f) => f.name === 'body' && f.kind === 'fixed')).toBe(true)
    expect(fields.some((f) => f.kind === 'attribute')).toBe(true)
  })

  it('mockFacet groups by a field, sorted by count desc', () => {
    const res = mockFacet('service', '', WIDE.start, WIDE.end, 50)
    expect(Array.isArray(res.values)).toBe(true)
    expect(typeof res.capped).toBe('boolean')
    for (let i = 1; i < res.values.length; i++) {
      expect(res.values[i - 1].count).toBeGreaterThanOrEqual(res.values[i].count)
    }
  })

  it('mockFacet counts the real pinned columns (service.name / severity_text), not just `service`', () => {
    // The Logs rail facets `service.name` and `severity_text`; both must return non-zero counts
    // (regression: they previously fell through to `attributes[field]` and yielded all-zero rows).
    const svc = mockFacet('service.name', '', WIDE.start, WIDE.end, 50)
    expect(svc.values.length).toBeGreaterThan(0)
    expect(svc.values.reduce((a, v) => a + v.count, 0)).toBeGreaterThan(0)
    expect(svc.values.every((v) => SERVICES.includes(v.value))).toBe(true)

    const sev = mockFacet('severity_text', '', WIDE.start, WIDE.end, 50)
    expect(sev.values.length).toBeGreaterThan(0)
    expect(sev.values.every((v) => SEVERITY_KEYS.includes(String(v.value).toLowerCase()))).toBe(true)
  })

  it('mockHistogram returns the requested number of severity-stacked buckets', () => {
    const buckets = mockHistogram('', WIDE.start, WIDE.end, 12)
    expect(buckets).toHaveLength(12)
    expect(buckets[0]).toHaveProperty('total')
    expect(buckets[0]).toHaveProperty('error')
    expect(typeof buckets[0].t).toBe('string') // nanos as string
  })
})

describe('spans/traces mock aggregation shapes', () => {
  it('mockSearchTraces returns rolled-up trace summaries', () => {
    const res = mockSearchTraces({ query: '', sort: 'recent', limit: 100 })
    expect(Array.isArray(res.traces)).toBe(true)
    expect(typeof res.matched_count).toBe('number')
    expect(typeof res.elapsed_ms).toBe('number')
    expect(res.matched_count).toBeGreaterThan(0)
    expect(res.traces.length).toBeGreaterThan(0)
    for (const t of res.traces) {
      expect(typeof t.trace_id).toBe('string')
      expect(typeof t.start_ts).toBe('string') // nanos as string, pre-hydration
      expect(typeof t.span_count).toBe('number')
      expect(t.span_count).toBeGreaterThan(0)
      expect(typeof t.error_count).toBe('number')
      expect(Array.isArray(t.services)).toBe(true)
      expect(t.services.length).toBeGreaterThan(0)
    }
  })

  it('mockSearchTraces filters by service and rolls up only matching traces', () => {
    const all = mockSearchTraces({ query: '' })
    const filtered = mockSearchTraces({ query: 'service:api' })
    expect(filtered.matched_count).toBeLessThanOrEqual(all.matched_count)
    for (const t of filtered.traces) {
      expect(t.services).toContain('api')
    }
  })

  it('mockSearchSpans returns the search envelope with hydratable span rows', () => {
    const res = mockSearchSpans({ query: '', sort: 'recent', limit: 500 })
    expect(Array.isArray(res.rows)).toBe(true)
    expect(res.matched_count).toBeGreaterThanOrEqual(res.rows.length)
    expect(typeof res.rows[0].start_time_nanos).toBe('string')
    expect(typeof res.rows[0].trace_id).toBe('string')
  })

  it('mockTracesFields returns typed field entries', () => {
    const fields = mockTracesFields()
    expect(fields.some((f) => f.name === 'trace_id' && f.kind === 'fixed')).toBe(true)
    expect(fields.some((f) => f.name === 'service.name' && f.kind === 'promoted')).toBe(true)
  })

  it('mockTracesFacet groups by a field, sorted by count desc', () => {
    const res = mockTracesFacet('service', '', WIDE.start, WIDE.end, 50)
    expect(Array.isArray(res.values)).toBe(true)
    for (let i = 1; i < res.values.length; i++) {
      expect(res.values[i - 1].count).toBeGreaterThanOrEqual(res.values[i].count)
    }
  })

  it('mockTracesHistogram returns the requested number of status-stacked buckets', () => {
    const buckets = mockTracesHistogram('', WIDE.start, WIDE.end, 12)
    expect(buckets).toHaveLength(12)
    expect(buckets[0]).toHaveProperty('total')
    expect(buckets[0]).toHaveProperty('error')
    expect(typeof buckets[0].t).toBe('string')
  })

  it('mockTracesLatency returns monotone percentiles', () => {
    const res = mockTracesLatency('', WIDE.start, WIDE.end, 12)
    expect(res.buckets).toHaveLength(12)
    const p50 = BigInt(res.p50)
    const p90 = BigInt(res.p90)
    const p99 = BigInt(res.p99)
    expect(p50 <= p90).toBe(true)
    expect(p90 <= p99).toBe(true)
    const totalBucketCount = res.buckets.reduce((sum, b) => sum + b.count, 0)
    expect(totalBucketCount).toBeGreaterThan(0)
  })
})

import {
  mockMetricCatalog, mockMetricMetadata, mockMetricLabels, mockMetricQuery,
} from '@/lib/core/mock'

describe('mock metrics', () => {
  const START = '0'
  const END = '3600000000000' // 1h in ns

  it('catalog returns entries with the shipped shape', () => {
    const cat = mockMetricCatalog(START, END)
    expect(cat.length).toBeGreaterThan(0)
    const e = cat.find((m) => m.type === 'sum')
    expect(e).toMatchObject({
      name: expect.any(String), type: expect.any(String),
      series_count: expect.any(Number), last_seen: expect.any(String),
    })
    expect(typeof e.last_seen).toBe('string') // ns as string
  })

  it('catalog honors the search substring and type filter', () => {
    const all = mockMetricCatalog(START, END)
    const gauges = mockMetricCatalog(START, END, { type: 'gauge' })
    expect(gauges.every((m) => m.type === 'gauge')).toBe(true)
    expect(gauges.length).toBeLessThanOrEqual(all.length)
  })

  it('metadata returns attribute_keys including service, null for unknown', () => {
    const name = mockMetricCatalog(START, END)[0].name
    const md = mockMetricMetadata(name, START, END)
    expect(md.attribute_keys).toContain('service')
    expect(mockMetricMetadata('nope.not.real', START, END)).toBeNull()
  })

  it('labels returns keys, or values when a key is given', () => {
    const name = mockMetricCatalog(START, END)[0].name
    expect(mockMetricLabels(name, null, START, END)).toHaveProperty('keys')
    const vals = mockMetricLabels(name, 'service', START, END)
    expect(vals).toHaveProperty('values')
    expect(vals.capped).toBe(false)
  })

  it('query returns one result with grouped series of {t,v} points', () => {
    const name = mockMetricCatalog(START, END).find((m) => m.type === 'sum').name
    const res = mockMetricQuery({
      queries: [{ id: 'a', metric: name, group_by: ['service'], filter: '' }],
      start: START, end: END,
    })
    expect(res.results).toHaveLength(1)
    const r = res.results[0]
    expect(r.id).toBe('a')
    expect(r.series.length).toBeGreaterThan(0)
    expect(typeof r.series[0].points[0].t).toBe('string')
    expect(r.series[0].exemplars).toEqual([])
    expect(r.default_agg).toBeTruthy()
    expect(res.capped).toBe(false)
    expect(res.step).toEqual(expect.any(String))
  })
})

import { mockStorage, mockUsageSeries } from '@/lib/core/mock'

describe('mock data & usage', () => {
  it('mockStorage has the {signals, durable} shape with per-signal durable_bytes', () => {
    expect(mockStorage.signals.logs).toMatchObject({ bytes: expect.any(Number), durable_bytes: expect.any(Number) })
    expect(mockStorage.durable).toMatchObject({ configured: expect.any(Boolean), pending: expect.any(Number) })
    expect('last_replicated_ms' in mockStorage.durable).toBe(true)
  })
  it('mockUsageSeries returns per-signal point arrays with the documented fields', () => {
    const r = mockUsageSeries('24h')
    expect(r.window).toBe('24h')
    expect(typeof r.bucket_ms).toBe('number')
    for (const sig of ['logs', 'traces', 'metrics']) {
      expect(Array.isArray(r.series[sig])).toBe(true)
      expect(r.series[sig].length).toBeGreaterThan(0)
      expect(r.series[sig][0]).toMatchObject({ ts: expect.any(Number), hot_bytes: expect.any(Number) })
    }
  })
})

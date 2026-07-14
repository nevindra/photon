import { describe, it, expect, beforeEach } from 'vitest'
import { timeRange, customRange, scope } from '@/lib/core/context'
import { correlate, relatedFor } from '@/lib/core/useCorrelate'

beforeEach(() => { timeRange.value = '15m'; customRange.value = null; scope.value = null })

describe('correlate() never drops context', () => {
  it('injects the active preset range', () => {
    expect(correlate({ path: '/logs', query: { q: 'trace_id:abc' } }))
      .toBe('/logs?q=trace_id%3Aabc&range=15m')
  })
  it('injects a custom from/to window instead of range', () => {
    customRange.value = { startMs: 500, endMs: 800 }
    expect(correlate({ path: '/traces' })).toBe('/traces?from=500&to=800')
  })
  it('injects the active scope on every hop', () => {
    scope.value = { type: 'service', id: 'checkout', label: 'checkout' }
    expect(correlate({ path: '/logs', query: { q: 'error:true' } }))
      .toBe('/logs?q=error%3Atrue&range=15m&scope=service%3Acheckout')
  })
})

describe('relatedFor() builds the correct destination graph', () => {
  it('a span links to logs (span+trace), its service, and similar traces', () => {
    const d = relatedFor({ kind: 'span', fields: { traceId: 't1', spanId: 's1', service: 'checkout', operation: 'POST /charge' } })
    const ids = d.map((x) => x.id)
    expect(ids).toContain('logs-span')
    expect(ids).toContain('service-health')
    expect(ids).toContain('similar-traces')
    const logsSpan = d.find((x) => x.id === 'logs-span')!
    expect(correlate(logsSpan.dest)).toContain('trace_id%3At1')
    expect(correlate(logsSpan.dest)).toContain('span_id%3As1')
  })
  it('a RUM error without a trace_id omits the Trace destination', () => {
    const withT = relatedFor({ kind: 'rumError', fields: { traceId: 't9', service: 'web' } }).map((x) => x.id)
    const without = relatedFor({ kind: 'rumError', fields: { service: 'web' } }).map((x) => x.id)
    expect(withT).toContain('trace')
    expect(without).not.toContain('trace')
    expect(without).toContain('logs')
  })
  it('a service links out to logs, metrics, RUM, and uptime (the filled gaps)', () => {
    const ids = relatedFor({ kind: 'service', fields: { service: 'checkout' } }).map((x) => x.id)
    expect(ids).toEqual(expect.arrayContaining(['traces', 'logs', 'metrics', 'rum-app', 'uptime']))
  })
})

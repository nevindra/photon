import { describe, it, expect, beforeEach } from 'vitest'
import { timeRange, customRange } from '@/lib/core/context'
import { correlate, relatedFor } from '@/lib/core/useCorrelate'

beforeEach(() => { timeRange.value = '15m'; customRange.value = null })

describe('correlate() never drops context', () => {
  it('injects the active preset range', () => {
    expect(correlate({ path: '/logs', query: { q: 'trace_id:abc' } }))
      .toBe('/logs?q=trace_id%3Aabc&range=15m')
  })
  it('injects a custom from/to window instead of range', () => {
    customRange.value = { startMs: 500, endMs: 800 }
    expect(correlate({ path: '/traces' })).toBe('/traces?from=500&to=800')
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

  // These three used to carry a `svc` param no destination read, so each pivot landed unfiltered.
  // Every one must now speak the key its own view actually consumes.
  it('the service pivots carry a key their destination consumes, not a dead `svc`', () => {
    const d = relatedFor({ kind: 'service', fields: { service: 'checkout' } })
    const dest = (id: string) => d.find((x) => x.dest.path.length && x.id === id)!.dest

    // /metrics: the grammar filter MetricsExplorer persists as `q` (`service` resolves to the
    // promoted service.name column server-side).
    expect(dest('metrics').query).toEqual({ q: 'service:checkout' })
    // /rum: RumAppsView matches this against its registered apps by name.
    expect(dest('rum-app').query).toEqual({ app: 'checkout' })
    // /uptime: UptimeDashboard's name/target text filter.
    expect(dest('uptime').query).toEqual({ q: 'checkout' })

    for (const id of ['metrics', 'rum-app', 'uptime']) {
      expect(Object.keys(dest(id).query ?? {})).not.toContain('svc')
      expect(correlate(dest(id))).not.toContain('svc=')
    }
  })

  it('a service name the grammar cannot express drops the metrics term rather than emitting junk', () => {
    // `term()` refuses whitespace/commas (the grammar splits on them).
    const d = relatedFor({ kind: 'service', fields: { service: 'my service' } })
    expect(d.find((x) => x.id === 'metrics')!.dest.query).toEqual({ q: '' })
    // ...and correlate() drops the empty value entirely instead of writing `q=`.
    expect(correlate(d.find((x) => x.id === 'metrics')!.dest)).not.toContain('q=')
  })
})

// The offline fixture behind `/services[/:service]/errors`. It exists so the UI is usable with no
// backend, so what matters is that it teaches the SAME model the real API does — above all the
// identity rule, which a lazy fixture would quietly violate by listing one fingerprint once.
import { describe, it, expect } from 'vitest'
import { mockBackendIssues, mockBackendIssueFacets, mockBackendIssueDetail } from '@/lib/core/mock'

const START = '1700000000000000000'
const END = '1700003600000000000'

describe('mockBackendIssues', () => {
  it('names no service on the fleet list — each row carries its own', () => {
    const r = mockBackendIssues(null, '', START, END) as { service?: string; errors: { service: string }[] }
    expect(r.service).toBeUndefined()
    expect(new Set(r.errors.map((e) => e.service)).size).toBeGreaterThan(1)
  })

  it('scopes to one service and names it', () => {
    const r = mockBackendIssues('api', '', START, END) as { service?: string; errors: { service: string }[] }
    expect(r.service).toBe('api')
    expect(r.errors.every((e) => e.service === 'api')).toBe(true)
  })

  it('lists one fingerprint present in two services as TWO issues', () => {
    // The whole identity rule: same bug, two owners, two fixes.
    const r = mockBackendIssues(null, '', START, END) as { errors: { service: string; fingerprint: string }[] }
    const shared = r.errors.filter((e) => e.fingerprint === 'd93a71e5')
    expect(shared).toHaveLength(2)
    expect(new Set(shared.map((e) => e.service))).toEqual(new Set(['api', 'worker']))
  })

  it('orders by occurrences, descending', () => {
    const r = mockBackendIssues(null, '', START, END) as { errors: { count: number }[] }
    const counts = r.errors.map((e) => e.count)
    expect([...counts].sort((a, b) => b - a)).toEqual(counts)
  })

  it('narrows on a search needle', () => {
    const r = mockBackendIssues('api', 'timeout', START, END) as { errors: { error_type: string }[] }
    expect(r.errors).toHaveLength(1)
    expect(r.errors[0].error_type).toBe('SQLTimeoutError')
  })

  it('returns nanosecond STRINGS inside the requested window', () => {
    const r = mockBackendIssues('api', '', START, END) as { errors: { first_seen: string; last_seen: string }[] }
    for (const e of r.errors) {
      expect(typeof e.first_seen).toBe('string')
      expect(BigInt(e.first_seen)).toBeGreaterThanOrEqual(BigInt(START))
      expect(BigInt(e.last_seen)).toBeLessThanOrEqual(BigInt(END))
    }
  })
})

describe('mockBackendIssueFacets', () => {
  it('answers for all six fields', () => {
    const r = mockBackendIssueFacets('api')
    expect(Object.keys(r.facets).sort()).toEqual(
      ['deployment.environment', 'error.type', 'host.name', 'http.status_code', 'kind_text', 'name'].sort(),
    )
  })

  it('leaves every field present but empty for a service with no issues', () => {
    const r = mockBackendIssueFacets('nothing-here')
    expect(Object.values(r.facets).every((f) => f.values.length === 0)).toBe(true)
  })
})

describe('mockBackendIssueDetail', () => {
  it('carries the pair it was asked for', () => {
    const d = mockBackendIssueDetail('worker', 'd93a71e5', START, END)
    expect(d.service).toBe('worker')
    expect(d.fingerprint).toBe('d93a71e5')
    // Same fingerprint, other service — a different Issue with a different count.
    expect(d.occurrences).not.toBe(mockBackendIssueDetail('api', 'd93a71e5', START, END).occurrences)
  })

  it('has no stack for a status-only issue, rather than inventing one', () => {
    expect(mockBackendIssueDetail('worker', '7a2d55f9', START, END).sample_stack).toBeNull()
    expect(mockBackendIssueDetail('api', 'd93a71e5', START, END).sample_stack).toContain('at handler')
  })

  it('returns the series as [epoch ms, count] pairs', () => {
    const d = mockBackendIssueDetail('api', 'b1f0c2a4', START, END)
    expect(d.series).toHaveLength(48)
    const [t, count] = d.series[0]
    expect(t).toBeGreaterThanOrEqual(Number(BigInt(START) / 1_000_000n))
    expect(count).toBeGreaterThanOrEqual(0)
  })

  it('caps sample events at 20 and stamps each with its own span id', () => {
    const d = mockBackendIssueDetail('api', 'b1f0c2a4', START, END)
    expect(d.events).toHaveLength(20)
    expect(new Set(d.events.map((e) => e.span_id)).size).toBe(20)
  })

  it('is deterministic — no Date.now/Math.random in the fixture', () => {
    const a = mockBackendIssueDetail('api', 'b1f0c2a4', START, END)
    const b = mockBackendIssueDetail('api', 'b1f0c2a4', START, END)
    expect(a.series).toEqual(b.series)
  })
})

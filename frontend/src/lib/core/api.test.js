import { describe, it, expect, vi, afterEach } from 'vitest'
import { watchEffect, nextTick } from 'vue'
import { api } from '@/lib/core/api'

afterEach(() => {
  api.mock = false
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('api.mock reactivity', () => {
  // `api.mock` is a getter/setter over a Vue ref, so the "demo mode" badge (`:mock="api.mock"`)
  // updates live in BOTH directions — including when the `afterResponse` hook clears it after the
  // backend recovers. Previously it was a plain boolean the badge only saw on an incidental
  // re-render, and nothing ever set it back to `false` (a one-way latch). This guards the flag
  // staying reactive; the actual clear-on-2xx recovery only fires against a real server (in jsdom
  // ky rejects at Request construction before fetch, so it can't be unit-exercised here).
  it('is reactive so the badge tracks it in both directions', async () => {
    api.mock = false
    const seen = []
    const stop = watchEffect(() => seen.push(api.mock))
    expect(seen).toEqual([false]) // watchEffect runs once synchronously

    api.mock = true
    await nextTick()
    api.mock = false
    await nextTick()

    expect(seen).toEqual([false, true, false])
    stop()
  })
})

describe('api.red', () => {
  it('falls back to mockRed rows when the server is unreachable', async () => {
    // No fetch server in jsdom → the request rejects with no response → mock fallback.
    const rows = await api.red('', '0', '60000000000', 'operation')
    expect(api.mock).toBe(true)
    expect(Array.isArray(rows)).toBe(true)
    expect(rows.length).toBeGreaterThan(0)
    const r = rows[0]
    expect(r).toHaveProperty('service')
    expect(r).toHaveProperty('rate')
    expect(r).toHaveProperty('error_rate')
    expect(typeof r.p99).toBe('string')
  })

  it('mock group=service rolls operations up and nulls operation', async () => {
    const rows = await api.red('', '0', '60000000000', 'service')
    expect(rows.every((r) => r.operation === null)).toBe(true)
    // one row per distinct service in the mock corpus
    const services = new Set(rows.map((r) => r.service))
    expect(services.size).toBe(rows.length)
  })

  it('mock rate/error_rate are derived from count/error_count, not hardcoded', async () => {
    const start = '0'
    const end = '60000000000'
    const windowSecs = Number(BigInt(end) - BigInt(start)) / 1e9
    const rows = await api.red('', start, end, 'operation')
    expect(rows.length).toBeGreaterThan(0)
    for (const r of rows) {
      expect(r.rate).toBeCloseTo(r.count / windowSecs, 10)
      expect(r.error_rate).toBe(r.count ? r.error_count / r.count : 0)
    }
  })

  it('mock filters by a service:<X> query, returning only rows for that service', async () => {
    // 'api' is one of the fixed SERVICES the trace-span corpus is generated from (mock.js) —
    // with 40 generated traces it's virtually certain to appear, mirroring the existing
    // `service:api` usage in mock.test.js's `mockSearchTraces` filter test.
    const rows = await api.red('service:api', '0', '60000000000', 'operation')
    expect(rows.length).toBeGreaterThan(0)
    expect(rows.every((r) => r.service === 'api')).toBe(true)
  })
})

describe('api metrics (mock fallback)', () => {
  const START = '0', END = '3600000000000'
  it('metricCatalog falls back to the mock corpus and flips api.mock', async () => {
    const cat = await api.metricCatalog(START, END)
    expect(Array.isArray(cat)).toBe(true)
    expect(cat.length).toBeGreaterThan(0)
    expect(api.mock).toBe(true)
  })
  it('metricQuery returns the mock envelope', async () => {
    const name = (await api.metricCatalog(START, END)).find((m) => m.type === 'sum').name
    const res = await api.metricQuery({
      queries: [{ id: 'a', metric: name, group_by: ['service'], filter: '' }],
      start: START, end: END,
    })
    expect(res.results[0].series.length).toBeGreaterThan(0)
  })
  it('metricMetadata returns attribute_keys', async () => {
    const name = (await api.metricCatalog(START, END))[0].name
    const md = await api.metricMetadata(name, START, END)
    expect(md.attribute_keys).toContain('service')
  })
})

describe('api auth/session methods (mock fallback)', () => {
  it('session() returns an onboarded, authenticated shape when no backend is reachable', async () => {
    const s = await api.session()
    expect(s).toHaveProperty('authenticated')
    expect(s).toHaveProperty('needs_setup')
    expect(s.needs_setup).toBe(false)
  })

  it('listUsers() falls back to a mock users array', async () => {
    const res = await api.listUsers()
    expect(Array.isArray(res.users)).toBe(true)
  })
})

describe('api data & retention (mock fallback)', () => {
  it('getStorage() falls back to per-signal storage stats', async () => {
    const storage = await api.getStorage()
    expect(storage.signals).toHaveProperty('logs')
    expect(storage.signals).toHaveProperty('traces')
    expect(storage.signals).toHaveProperty('metrics')
    expect(storage).toHaveProperty('durable')
  })

  it('getRetention() falls back to per-signal retention days', async () => {
    const retention = await api.getRetention()
    expect(retention).toEqual(expect.objectContaining({ logs: expect.any(Number), traces: expect.any(Number), metrics: expect.any(Number) }))
  })

  it('setRetention() mocks a successful update when the server is unreachable', async () => {
    const res = await api.setRetention({ logs: 15 })
    expect(res).toEqual(expect.objectContaining({ ok: true }))
  })

  it('purgeData() mocks a successful purge when the server is unreachable', async () => {
    const res = await api.purgeData({ signal: 'logs', mode: 'all' })
    expect(res).toEqual(expect.objectContaining({ ok: true }))
  })
})

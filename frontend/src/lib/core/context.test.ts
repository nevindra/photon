import { describe, it, expect, beforeEach } from 'vitest'
import {
  timeRange, customRange, nowTick,
  startNs, endNs, prevStartNs, prevEndNs,
  setTimeRange, setCustomRange, RANGE_MS,
  tenant, setTenant, clearTenant, tenantQueryTerm,
  parseContext, seedContextFromUrl, syncContextToUrl,
} from '@/lib/core/context'

beforeEach(() => {
  // reset module singletons between tests
  customRange.value = null
  tenant.value = null
  nowTick.value = 1_000_000 // ms; deterministic "now"
  timeRange.value = '30m'
})

describe('context time math', () => {
  it('derives a preset window in ns relative to nowTick', () => {
    setTimeRange('15m')
    nowTick.value = 1_000_000
    // end = nowTick ms → ns; start = end - 15m
    expect(endNs.value).toBe((1_000_000n * 1_000_000n).toString())
    expect(startNs.value).toBe(((1_000_000 - RANGE_MS['15m']) * 1_000_000).toString().replace('.0', ''))
  })

  it('previous window is the same length, immediately before', () => {
    setTimeRange('15m'); nowTick.value = 1_000_000
    expect(prevEndNs.value).toBe(startNs.value)
    const win = RANGE_MS['15m']
    expect(prevStartNs.value).toBe(((1_000_000 - 2 * win) * 1_000_000).toString())
  })

  it('a custom range overrides the preset and is deterministic', () => {
    setCustomRange({ startMs: 500, endMs: 800 })
    expect(startNs.value).toBe((500n * 1_000_000n).toString())
    expect(endNs.value).toBe((800n * 1_000_000n).toString())
  })

  it('setTimeRange clears any custom range', () => {
    setCustomRange({ startMs: 500, endMs: 800 })
    setTimeRange('1h')
    expect(customRange.value).toBeNull()
    expect(timeRange.value).toBe('1h')
  })

  it('tenant set/clear drives tenantQueryTerm', () => {
    expect(tenantQueryTerm()).toBeNull()
    setTenant('divtik')
    expect(tenantQueryTerm()).toBe('tenant:divtik')
    clearTenant()
    expect(tenantQueryTerm()).toBeNull()
  })

  it('tenantQueryTerm ignores values outside the tenant-name shape (URL-injected)', () => {
    // `?tenant=a b` would otherwise leak ` b` into the query as a body-substring term.
    setTenant('a b')
    expect(tenantQueryTerm()).toBeNull()
    setTenant('a"quoted')
    expect(tenantQueryTerm()).toBeNull()
    clearTenant()
  })
})

describe('context URL', () => {
  beforeEach(() => { window.history.replaceState(null, '', '/') })

  it('parseContext reads range + tenant', () => {
    const c = parseContext('?range=15m&tenant=divtik&q=foo')
    expect(c.timeRange).toBe('15m')
    expect(c.tenant).toBe('divtik')
  })

  it('parseContext reads a custom from/to window', () => {
    const c = parseContext('?from=500&to=800')
    expect(c.customRange).toEqual({ startMs: 500, endMs: 800 })
  })

  it('seedContextFromUrl hydrates the module refs', () => {
    window.history.replaceState(null, '', '/logs?range=1h&tenant=divtik')
    seedContextFromUrl()
    expect(timeRange.value).toBe('1h')
    expect(tenant.value).toBe('divtik')
  })

  // Regression for the router.afterEach wiring in router/index.js: a bare `router.push(...)`
  // (NavRail world switch, list drill-ins, back buttons) navigates to a URL with no query at
  // all — the watch() in startContextUrlSync only fires on ref changes, so nothing re-syncs
  // range/tenant onto the new path unless something explicitly calls syncContextToUrl() after
  // the navigation. This proves that call carries the active context onto a bare URL. It only
  // passes because syncContextToUrl is exported (Fix A) — before that it wasn't importable and
  // the call below throws "syncContextToUrl is not a function".
  it('syncContextToUrl carries the active range + tenant onto a bare navigation with no query', () => {
    setTimeRange('1h')
    setTenant('divtik')
    // Simulate what vue-router did: navigated to a new path, dropping the old query entirely.
    window.history.replaceState(null, '', '/services')
    expect(window.location.search).toBe('')

    syncContextToUrl()

    const params = new URLSearchParams(window.location.search)
    expect(params.get('range')).toBe('1h')
    expect(params.get('tenant')).toBe('divtik')
  })

  // Regression: syncContextToUrl runs from router.afterEach on EVERY navigation, i.e. immediately
  // after landing on a detail route. It used to pass `null` as the state, which wipes the
  // `{ back, current, forward, position, scroll }` object vue-router keeps on the entry — so
  // `history.state.back` was gone and lib/core/useBackTo.ts could never recognise the entry it
  // came from (it fell back to a filterless push). Same reason vue-router's own scroll
  // restoration depends on that object surviving. A merge-write of the URL must never touch state.
  it('preserves the history state vue-router keeps on the entry', () => {
    const routerState = {
      back: '/traces?range=30m&q=kind%3Aserver&sort=recent&mode=traces',
      current: '/traces/abc?t=123',
      forward: null,
      position: 4,
      replaced: false,
      scroll: { left: 0, top: 0 },
    }
    window.history.replaceState(routerState, '', '/traces/abc?t=123')
    setTimeRange('1h')

    syncContextToUrl()

    // `back` (what useBackTo reads) and the rest of vue-router's bookkeeping survive; only
    // `current` follows the URL — see lib/core/historyUrl.ts for why it must.
    expect(window.history.state).toMatchObject({
      back: routerState.back,
      forward: null,
      position: 4,
      scroll: { left: 0, top: 0 },
    })
    expect(new URLSearchParams(window.location.search).get('range')).toBe('1h')
  })
})

import { describe, it, expect, beforeEach } from 'vitest'
import {
  timeRange, customRange, scope, nowTick,
  startNs, endNs, prevStartNs, prevEndNs,
  setTimeRange, setCustomRange, setScope, clearScope, RANGE_MS,
  parseContext, seedContextFromUrl, syncContextToUrl,
} from '@/lib/core/context'

beforeEach(() => {
  // reset module singletons between tests
  customRange.value = null
  scope.value = null
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

  it('scope set/clear', () => {
    setScope({ type: 'service', id: 'checkout', label: 'checkout' })
    expect(scope.value?.id).toBe('checkout')
    clearScope()
    expect(scope.value).toBeNull()
  })
})

describe('context URL', () => {
  beforeEach(() => { window.history.replaceState(null, '', '/') })

  it('parseContext reads range + scope', () => {
    const c = parseContext('?range=15m&scope=service:checkout&q=foo')
    expect(c.timeRange).toBe('15m')
    expect(c.scope).toEqual({ type: 'service', id: 'checkout', label: 'checkout' })
  })

  it('parseContext reads a custom from/to window', () => {
    const c = parseContext('?from=500&to=800')
    expect(c.customRange).toEqual({ startMs: 500, endMs: 800 })
  })

  it('seedContextFromUrl hydrates the module refs', () => {
    window.history.replaceState(null, '', '/logs?range=1h&scope=rumApp:web')
    seedContextFromUrl()
    expect(timeRange.value).toBe('1h')
    expect(scope.value).toEqual({ type: 'rumApp', id: 'web', label: 'web' })
  })

  // Regression for the router.afterEach wiring in router/index.js: a bare `router.push(...)`
  // (NavRail world switch, list drill-ins, back buttons) navigates to a URL with no query at
  // all — the watch() in startContextUrlSync only fires on ref changes, so nothing re-syncs
  // range/scope onto the new path unless something explicitly calls syncContextToUrl() after
  // the navigation. This proves that call carries the active context onto a bare URL. It only
  // passes because syncContextToUrl is exported (Fix A) — before that it wasn't importable and
  // the call below throws "syncContextToUrl is not a function".
  it('syncContextToUrl carries the active range + scope onto a bare navigation with no query', () => {
    setTimeRange('1h')
    setScope({ type: 'service', id: 'checkout', label: 'checkout' })
    // Simulate what vue-router did: navigated to a new path, dropping the old query entirely.
    window.history.replaceState(null, '', '/services')
    expect(window.location.search).toBe('')

    syncContextToUrl()

    const params = new URLSearchParams(window.location.search)
    expect(params.get('range')).toBe('1h')
    expect(params.get('scope')).toBe('service:checkout')
  })
})

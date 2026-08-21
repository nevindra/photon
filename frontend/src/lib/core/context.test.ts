import { describe, it, expect, beforeEach } from 'vitest'
import { nextTick, ref } from 'vue'
import {
  timeRange, customRange, nowTick,
  startNs, endNs, prevStartNs, prevEndNs,
  setTimeRange, setCustomRange, RANGE_MS,
  tenant, setTenant, clearTenant, tenantQueryTerm,
  environment, setEnvironment, clearEnvironment, envQueryTerm,
  environmentDefaulted, applyDefaultEnvironment,
  contextQueryTerm, withContext,
  parseContext, seedContextFromUrl, syncContextToUrl,
} from '@/lib/core/context'
import { useUrlState } from '@/lib/core/useUrlState'

beforeEach(() => {
  // reset module singletons between tests
  customRange.value = null
  tenant.value = null
  environment.value = null
  environmentDefaulted.value = false
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

// Environment is the third global dimension, alongside the time window and the Site (`tenant`).
// It mirrors the tenant dimension exactly — same grammar-term contribution, same URL key
// ownership, same shape guard on URL-supplied values.
describe('context environment', () => {
  it('env_term_is_appended_to_the_query', () => {
    setEnvironment('prod')
    expect(envQueryTerm()).toBe('deployment.environment:prod')
    expect(withContext('level:error')).toBe('level:error deployment.environment:prod')
    // Site + Environment ride together in one fragment, tenant first.
    setTenant('divtik')
    expect(contextQueryTerm()).toBe('tenant:divtik deployment.environment:prod')
    expect(withContext('level:error')).toBe('level:error tenant:divtik deployment.environment:prod')
  })

  it('clearing_the_environment_removes_the_term_entirely', () => {
    setEnvironment('prod')
    clearEnvironment()
    // Not `deployment.environment:*` (which would filter to records that DECLARE one) — no term
    // at all, so records without an environment stay in the result set.
    expect(envQueryTerm()).toBeNull()
    expect(contextQueryTerm()).toBeNull()
    expect(withContext('level:error')).toBe('level:error')
  })

  it('envQueryTerm ignores values outside the environment-name shape (URL-injected)', () => {
    setEnvironment('a b')
    expect(envQueryTerm()).toBeNull()
    setEnvironment('a"quoted')
    expect(envQueryTerm()).toBeNull()
  })

  it('prod_is_selected_when_present', () => {
    applyDefaultEnvironment(['staging', 'prod', 'dev'])
    expect(environment.value).toBe('prod')
  })

  it('nothing_is_selected_when_prod_is_absent', () => {
    applyDefaultEnvironment(['staging', 'dev'])
    expect(environment.value).toBeNull()
  })

  it('applies the default once, so clearing the picker sticks', () => {
    applyDefaultEnvironment(['staging', 'prod'])
    clearEnvironment()
    applyDefaultEnvironment(['staging', 'prod'])
    expect(environment.value).toBeNull()
  })

  it('an explicit ?env= wins over the prod default', () => {
    setEnvironment('staging')
    applyDefaultEnvironment(['staging', 'prod'])
    expect(environment.value).toBe('staging')
  })

  it('selects nothing when the picker would be hidden (fewer than two values)', () => {
    applyDefaultEnvironment(['prod'])
    expect(environment.value).toBeNull()
  })
})

describe('context URL', () => {
  beforeEach(() => { window.history.replaceState(null, '', '/') })

  it('parseContext reads env', () => {
    expect(parseContext('?range=15m&env=prod').environment).toBe('prod')
  })

  it('seedContextFromUrl hydrates the environment', () => {
    window.history.replaceState(null, '', '/logs?env=staging')
    seedContextFromUrl()
    expect(environment.value).toBe('staging')
  })

  // The drill-in → back round trip: `env` is a context key, so it has to survive both halves.
  // Going in, vue-router pushes a detail path with no query at all and router.afterEach's
  // syncContextToUrl() has to re-stamp it. Coming back, the list view rewrites its OWN keys
  // (svc/sev/q) through useUrlState, a merge-write that must leave `env` alone.
  it('env_survives_a_drill_in_and_back', async () => {
    setEnvironment('prod')
    window.history.replaceState(null, '', '/traces/abc')
    syncContextToUrl()
    expect(new URLSearchParams(window.location.search).get('env')).toBe('prod')

    const text = ref('')
    useUrlState({ text })
    text.value = 'db timeout'
    await nextTick()

    const p = new URLSearchParams(window.location.search)
    expect(p.get('q')).toBe('db timeout')
    expect(p.get('env')).toBe('prod')
  })

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

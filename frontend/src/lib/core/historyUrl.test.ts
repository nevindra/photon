import { describe, it, expect, beforeEach } from 'vitest'
import { createRouter, createWebHistory } from 'vue-router'
import { replaceSearch } from './historyUrl'
import { syncContextToUrl, setTimeRange, customRange } from './context'
import { isListEntry } from './useBackTo'

describe('replaceSearch', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
  })

  it('merges a query string onto the current path without navigating', () => {
    window.history.replaceState(null, '', '/traces')
    replaceSearch(new URLSearchParams({ range: '30m', q: 'kind:server' }))
    expect(window.location.pathname).toBe('/traces')
    expect(new URLSearchParams(window.location.search).get('q')).toBe('kind:server')
  })

  it('drops the "?" entirely when there is nothing left', () => {
    window.history.replaceState(null, '', '/traces?q=x')
    replaceSearch('')
    expect(window.location.search).toBe('')
    expect(window.location.pathname).toBe('/traces')
  })

  it('accepts a raw string, with or without the leading "?"', () => {
    replaceSearch('?a=1')
    expect(window.location.search).toBe('?a=1')
    replaceSearch('b=2')
    expect(window.location.search).toBe('?b=2')
  })

  it('preserves the hash', () => {
    window.history.replaceState(null, '', '/traces#row-4')
    replaceSearch('q=x')
    expect(window.location.hash).toBe('#row-4')
    expect(window.location.search).toBe('?q=x')
  })

  // The whole reason this helper exists.
  it('never clobbers the entry state (vue-router keeps its bookkeeping there)', () => {
    const state = {
      back: '/traces?q=kind%3Aserver',
      current: '/traces/abc',
      forward: null,
      position: 3,
      scroll: { left: 0, top: 0 },
    }
    window.history.replaceState(state, '', '/traces/abc')
    replaceSearch('range=30m')
    // Everything vue-router relies on survives...
    expect(window.history.state).toMatchObject({
      back: state.back,
      forward: null,
      position: 3,
      scroll: { left: 0, top: 0 },
    })
    // ...and `current` follows the URL, or vue-router's next push() would re-assert the OLD one
    // and silently revert the query we just merged in.
    expect(window.history.state.current).toBe('/traces/abc?range=30m')
  })

  it('leaves a non-vue-router state object alone', () => {
    window.history.replaceState({ mine: 1 }, '', '/traces')
    replaceSearch('q=x')
    expect(window.history.state).toEqual({ mine: 1 })
  })

  it('keeps `current` base-relative rather than copying location.pathname', () => {
    // Under a non-'/' router base, state.current omits the base prefix — deriving it from
    // location.pathname would corrupt it.
    window.history.replaceState({ current: '/traces' }, '', '/app/traces')
    replaceSearch('q=x')
    expect(window.history.state.current).toBe('/traces?q=x')
    expect(window.location.pathname).toBe('/app/traces')
  })
})

// End-to-end reproduction of the reported bug: filters survive a drill-in → back round trip only
// if the detail entry still knows which entry it came from. `router.afterEach(syncContextToUrl)`
// runs on arrival at the detail route, and used to `replaceState(null, …)` — wiping the
// `{ back, … }` object vue-router had just written. `useBackTo` then saw no `back`, fell back to a
// bare push('/traces'), and the `q` filter was gone.
describe('drill-in → back preserves the list entry (regression)', () => {
  it('keeps history.state.back — including the filters — across the afterEach URL sync', async () => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    setTimeRange('30m')

    const router = createRouter({
      history: createWebHistory(),
      routes: [
        { path: '/traces', component: { template: '<div />' } },
        { path: '/traces/:traceId', component: { template: '<div />' } },
      ],
    })
    // Exactly the wiring in router/index.js.
    router.afterEach(() => syncContextToUrl())

    await router.push('/traces')
    await router.isReady()
    // The explorer layers its own filters on via replaceSearch (q + sort + mode).
    const p = new URLSearchParams(window.location.search)
    p.set('q', 'kind:server')
    p.set('sort', 'recent')
    p.set('mode', 'traces')
    replaceSearch(p)
    expect(window.location.search).toContain('q=kind%3Aserver')

    // Drill in, exactly like the peek drawer's "Open full view".
    await router.push('/traces/81c1?t=1785137173304692266')

    // The detail entry must still know WHICH ROUTE it came from. Note `back` records the URL as of
    // the last router navigation, so it does NOT carry the filters that `replaceSearch` layered on
    // afterwards — it doesn't need to. All `useBackTo` asks of it is "was the previous entry the
    // list?", and the filters live on the browser's own entry, which `replaceState` updated in
    // place. Before the fix this was `undefined` and the answer was always "no".
    const back = window.history.state?.back
    expect(isListEntry(back, '/traces')).toBe(true)

    // And popping really does restore the filters — the entry, not `state.back`, is the source.
    // jsdom queues session-history traversal as a task, so give it a few macrotask turns.
    router.back()
    for (let i = 0; i < 10 && window.location.pathname !== '/traces'; i++) {
      await new Promise((r) => setTimeout(r, 0))
    }
    expect(window.location.pathname).toBe('/traces')
    expect(new URLSearchParams(window.location.search).get('q')).toBe('kind:server')
    expect(router.currentRoute.value.query.q).toBe('kind:server')
  })
})

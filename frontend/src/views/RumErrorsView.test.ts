// Smoke test for the Task-9 time migration of RumErrorsView (`/rum/:appId/errors`). Mirrors
// RumVitalsView.test.js. Proves the migrated view mounts, keeps the app-level crumb, and queries
// the JS-error issue list with the global context window.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { startNs, endNs, customRange, clearScope, setTimeRange } from '@/lib/core/context'
import RumErrorsView from './RumErrorsView.vue'

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    rumErrors: vi.fn().mockResolvedValue({ app: 'web-storefront', errors: [] }),
    rumErrorFacets: vi.fn().mockResolvedValue({ app: 'web-storefront', facets: {} }),
  },
}))

const routes = [
  { path: '/rum', component: { template: '<div />' } },
  { path: '/rum/:appId', component: { template: '<div />' } },
  { path: '/rum/:appId/pages', component: { template: '<div />' } },
  { path: '/rum/:appId/errors', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } },
  })
  return [VueQueryPlugin, { queryClient }]
}

// `path` may carry a query string (e.g. `?q=...`) — vue-router parses it into `route.query`,
// exercising the same route.query.q seed path LogsView's pivot-seed test proves (see
// LogsView.test.js's "route q param seeds the search" describe block).
async function mountErrors(path = '/rum/web-storefront/errors') {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(path)
  await router.isReady()
  const wrapper = mount(
    {
      components: { TooltipProvider, RumErrorsView },
      template: '<TooltipProvider><RumErrorsView /></TooltipProvider>',
    },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  return { wrapper, router }
}

describe('RumErrorsView (integration)', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    clearScope()
    setTimeRange('30m')
  })

  it('mounts and keeps the app-level crumb', async () => {
    const { wrapper } = await mountErrors()
    await flushPromises()
    expect(wrapper.text()).toContain('Frontend › web-storefront')
    wrapper.unmount()
  })

  it('queries errors with the global context window', async () => {
    const { api } = await import('@/lib/core/api')
    setTimeRange('15m')
    vi.mocked(api.rumErrors).mockClear() // call history accumulates across this file's tests — isolate ours
    const { wrapper } = await mountErrors()
    await flushPromises()
    // useRumErrors(app, startNs, endNs) → api.rumErrors(app, startNs, endNs, { signal }).
    const call = vi.mocked(api.rumErrors).mock.calls[0]
    expect(call[0]).toBe('web-storefront')
    expect(call[1]).toBe(startNs.value)
    expect(call[2]).toBe(endNs.value)
    wrapper.unmount()
  })

  it('seeds text from the route q param and passes the debounced q into useRumErrors, updating the url', async () => {
    const { api } = await import('@/lib/core/api')
    vi.mocked(api.rumErrors).mockClear()
    const { wrapper } = await mountErrors('/rum/web-storefront/errors?q=exception.type%3ATypeError')
    await flushPromises()
    // useRumErrors(app, startNs, endNs, q) → api.rumErrors(app, startNs, endNs, { signal }, q):
    // `q` is the 5th positional arg (index 4).
    await new Promise((r) => setTimeout(r, 200)) // allow the 180ms debounce (refDebounced) to settle
    await flushPromises()
    const call = vi.mocked(api.rumErrors).mock.calls.at(-1)
    expect(call?.[4]).toContain('exception.type:TypeError')
    // useUrlState persists `text` back into the URL (`?q=...`) — proves the round trip, not just the read.
    expect(window.location.search).toContain('q=exception.type')
    wrapper.unmount()
  })
})

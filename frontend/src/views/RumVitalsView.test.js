// Mirrors LogsView.test.js: mounts the RUM Web Vitals hero under a memory router + a fresh
// QueryClient + a <TooltipProvider> (AppShell/NavRail render Reka Tooltips, which throw without a
// provider ancestor). The api layer is fully mocked (no network), so the view renders deterministic
// scorecards + breakdown rows. Also proves the route-dimension row-click drills into page detail.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { startNs, endNs, scope, customRange, setTimeRange, clearScope } from '@/lib/core/context'
import RumVitalsView from './RumVitalsView.vue'
import WebVitalScorecard from '@/components/rum/WebVitalScorecard.vue'

// No network. `rumVitals` returns three CWV entries (LCP needs / INP good / CLS good); `rumBreakdown`
// returns one route row so the (route-dimension) table renders a clickable row. `mock:false` mirrors
// the real api's plain-boolean field so AppShell's :mock binding renders.
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    rumVitals: vi.fn().mockResolvedValue({
      app: 'web-storefront',
      vitals: [
        { metric: 'web_vitals.lcp', p75: 2800, rating: 'needs', good_max: 2500, poor_min: 4000, dist: { good: 58, needs: 31, poor: 11, total: 100 } },
        { metric: 'web_vitals.inp', p75: 184, rating: 'good', good_max: 200, poor_min: 500, dist: { good: 84, needs: 12, poor: 4, total: 100 } },
        { metric: 'web_vitals.cls', p75: 0.06, rating: 'good', good_max: 0.1, poor_min: 0.25, dist: { good: 88, needs: 9, poor: 3, total: 100 } },
      ],
    }),
    rumBreakdown: vi.fn().mockResolvedValue({
      app: 'web-storefront',
      dimension: 'browser.route',
      rows: [{ key: '/checkout', pageviews: 142000, lcp_p75: 4300, inp_p75: 210, cls_p75: 0.09 }],
    }),
  },
}))

const routes = [
  { path: '/rum', component: { template: '<div />' } },
  { path: '/rum/:appId', component: { template: '<div />' } },
  { path: '/rum/:appId/pages', component: { template: '<div />' } },
  { path: '/rum/:appId/pages/:route', component: { template: '<div />' } },
  { path: '/rum/:appId/errors', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

function queryPlugin() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } },
  })
  return [VueQueryPlugin, { queryClient }]
}

async function makeRouter(initial = '/rum/web-storefront') {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(initial)
  await router.isReady()
  return router
}

async function mountVitals(initial = '/rum/web-storefront') {
  const router = await makeRouter(initial)
  const wrapper = mount(
    {
      components: { TooltipProvider, RumVitalsView },
      template: '<TooltipProvider><RumVitalsView /></TooltipProvider>',
    },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  return { wrapper, router }
}

describe('RumVitalsView (integration)', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    // Time + scope are global now (lib/context) — reset the module singletons between tests.
    customRange.value = null
    clearScope()
    setTimeRange('30m')
  })

  it('mounts inside a TooltipProvider without throwing', async () => {
    const { wrapper } = await mountVitals()
    await flushPromises()
    expect(wrapper.exists()).toBe(true)
    wrapper.unmount()
  })

  it('renders one WebVitalScorecard per returned vital', async () => {
    const { wrapper } = await mountVitals()
    await flushPromises()
    const cards = wrapper.findAllComponents(WebVitalScorecard)
    expect(cards).toHaveLength(3)
    // Human labels are mapped from the `web_vitals.*` metric ids.
    const text = wrapper.text()
    expect(text).toContain('LCP')
    expect(text).toContain('INP')
    expect(text).toContain('CLS')
    wrapper.unmount()
  })

  it('calls the (mocked) rum api on mount', async () => {
    const { api } = await import('@/lib/core/api')
    const { wrapper } = await mountVitals()
    await flushPromises()
    expect(api.rumVitals).toHaveBeenCalled()
    expect(api.rumBreakdown).toHaveBeenCalled()
    // Default breakdown dimension is the route dimension.
    expect(api.rumBreakdown.mock.calls[0][1]).toBe('browser.route')
    wrapper.unmount()
  })

  it('queries vitals with the global context window and scopes to the app', async () => {
    const { api } = await import('@/lib/core/api')
    setTimeRange('15m')
    api.rumVitals.mockClear() // call history accumulates across this file's tests — isolate ours
    const { wrapper } = await mountVitals()
    await flushPromises()
    // useRumVitals(app, startNs, endNs) → api.rumVitals(app, startNs, endNs, { signal }).
    const call = api.rumVitals.mock.calls[0]
    expect(call[1]).toBe(startNs.value)
    expect(call[2]).toBe(endNs.value)
    // Mounting the vitals hero sets the active entity scope to this RUM app.
    expect(scope.value).toEqual({ type: 'rumApp', id: 'web-storefront', label: 'web-storefront' })
    wrapper.unmount()
  })

  it('drills into page detail when a route-dimension breakdown row is clicked', async () => {
    const { wrapper, router } = await mountVitals()
    await flushPromises()
    const push = vi.spyOn(router, 'push')

    const row = wrapper.find('[data-testid="rum-breakdown-row"]')
    expect(row.exists()).toBe(true)
    await row.trigger('click')

    expect(push).toHaveBeenCalledWith('/rum/web-storefront/pages/%2Fcheckout')
    wrapper.unmount()
  })
})

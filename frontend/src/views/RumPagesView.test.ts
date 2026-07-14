// Smoke test for the Task-9 time migration of RumPagesView (`/rum/:appId/pages`). Mirrors
// RumVitalsView.test.js. Proves the migrated view mounts, keeps the app-level crumb, and queries
// the per-route rollup with the global context window.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { startNs, endNs, customRange, clearScope, setTimeRange } from '@/lib/core/context'
import RumPagesView from './RumPagesView.vue'

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    rumPages: vi.fn().mockResolvedValue({
      app: 'web-storefront',
      pages: [{ route: '/checkout', pageviews: 142000, lcp_p75: 2400, inp_p75: 180, cls_p75: 0.05 }],
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

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } },
  })
  return [VueQueryPlugin, { queryClient }]
}

async function mountPages() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/rum/web-storefront/pages')
  await router.isReady()
  const wrapper = mount(
    {
      components: { TooltipProvider, RumPagesView },
      template: '<TooltipProvider><RumPagesView /></TooltipProvider>',
    },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  return { wrapper, router }
}

describe('RumPagesView (integration)', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    clearScope()
    setTimeRange('30m')
  })

  it('mounts and keeps the app-level crumb', async () => {
    const { wrapper } = await mountPages()
    await flushPromises()
    expect(wrapper.text()).toContain('Frontend › web-storefront')
    wrapper.unmount()
  })

  it('queries pages with the global context window', async () => {
    const { api } = await import('@/lib/core/api')
    setTimeRange('15m')
    vi.mocked(api.rumPages).mockClear() // call history accumulates across this file's tests — isolate ours
    const { wrapper } = await mountPages()
    await flushPromises()
    // useRumPages(app, startNs, endNs) → api.rumPages(app, startNs, endNs, { signal }).
    const call = vi.mocked(api.rumPages).mock.calls[0]
    expect(call[0]).toBe('web-storefront')
    expect(call[1]).toBe(startNs.value)
    expect(call[2]).toBe(endNs.value)
    wrapper.unmount()
  })
})

// Task 8: ServiceDetailView migrated off LOCAL time onto the app-wide lib/context.ts window, and
// now sets the app-wide SCOPE to the routed service on mount. No test previously existed for this
// view, so this is a new minimal one (kept in TS per the branch's "new test files are .ts"
// convention). Mirrors RumVitalsView.test.js's full-mount shape: a real router + TooltipProvider
// ancestor (AppShell renders NavRail, which renders Reka Tooltips) + a fresh QueryClient, with
// `api.js` fully mocked so nothing hits the network.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ServiceDetailView from './ServiceDetailView.vue'
import AppShell from '@/components/common/AppShell.vue'
import { timeRange, customRange, scope, startNs, endNs, setTimeRange } from '@/lib/core/context'

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    serviceTimeseries: vi.fn().mockResolvedValue([
      { ts: '1000', count: 10, error_count: 1, rate: 1, error_rate: 0.1, p50: '1', p90: '2', p99: '3', apdex: 0.9, satisfied: 8, tolerating: 1, frustrated: 1 },
    ]),
    serviceDependencies: vi.fn().mockResolvedValue({ database: [], external: [] }),
    red: vi.fn().mockResolvedValue([]),
    serviceSettings: vi.fn().mockResolvedValue({ apdex_threshold_ms: 500, is_default: true }),
  },
}))

import { api } from '@/lib/core/api'

const routes = [
  { path: '/services', component: { template: '<div />' } },
  { path: '/services/:service', component: ServiceDetailView },
  { path: '/traces', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

async function mountView(initial = '/services/checkout') {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(initial)
  await router.isReady()
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const wrapper = mount(
    { components: { TooltipProvider, ServiceDetailView }, template: '<TooltipProvider><ServiceDetailView /></TooltipProvider>' },
    { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
  )
  return { wrapper, router }
}

describe('ServiceDetailView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    timeRange.value = '30m'
    scope.value = null
    vi.clearAllMocks()
  })

  it('sets the "Backend › <service>" breadcrumb on AppShell', async () => {
    const { wrapper } = await mountView('/services/checkout')
    await flushPromises()
    expect(wrapper.findComponent(AppShell).props('crumb')).toBe('Backend › checkout')
    wrapper.unmount()
  })

  it('scopes the app-wide context to the routed service on mount', async () => {
    const { wrapper } = await mountView('/services/checkout')
    await flushPromises()
    expect(scope.value).toEqual({ type: 'service', id: 'checkout', label: 'checkout' })
    wrapper.unmount()
  })

  it('re-scopes when the :service route param changes under the same instance', async () => {
    const { wrapper, router } = await mountView('/services/checkout')
    await flushPromises()
    await router.push('/services/web')
    await flushPromises()
    expect(scope.value).toEqual({ type: 'service', id: 'web', label: 'web' })
    wrapper.unmount()
  })

  it('drives the service timeseries query off the global context window, not local state', async () => {
    setTimeRange('15m')
    const { wrapper } = await mountView('/services/checkout')
    await flushPromises()
    expect(api.serviceTimeseries).toHaveBeenCalledWith(
      'checkout',
      { start: startNs.value, end: endNs.value, buckets: 48 },
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    )
    wrapper.unmount()
  })
})

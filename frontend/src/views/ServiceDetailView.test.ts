// The service Overview tab. What these tests hold down:
//
//   - It is a TAB now: the sub-nav is present, and dependencies are NOT on this page (their own
//     route, their own queries, so Overview paints without waiting on a dependency scan).
//   - The Logs panel is here. A service can be on fire in its logs while its spans look normal, and
//     a health page with four latency charts and no log volume can be green during an outage.
//   - Errors are charted BY CLASS. One error line for 5xx and 4xx together is why "our error rate
//     doubled" turns into an hour of work to find out somebody shipped a broken client.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ServiceDetailView from './ServiceDetailView.vue'
import ServiceLogPanel from '@/components/services/ServiceLogPanel.vue'
import ServiceSubNav from '@/components/services/ServiceSubNav.vue'
import AppShell from '@/components/common/AppShell.vue'
import { timeRange, customRange, tenant, environment, startNs, endNs, setTimeRange } from '@/lib/core/context'

const bucket = (over = {}) => ({
  ts: 1000, source: 'raw', rate: 1, error_rate: 0.1, count: 10, span_count: 14, basis: 'entry',
  error_count: 1, http_4xx: 3, http_5xx: 1, p50: '1000000', p90: '2000000', p99: '3000000',
  apdex: 0.9, satisfied: 8, tolerating: 1, frustrated: 1, ...over,
})

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    serviceTimeseries: vi.fn(() => Promise.resolve([{
      ts: 1000, source: 'raw', rate: 1, error_rate: 0.1, count: 10, span_count: 14, basis: 'entry',
      error_count: 1, http_4xx: 3, http_5xx: 1, p50: '1000000', p90: '2000000', p99: '3000000',
      apdex: 0.9, satisfied: 8, tolerating: 1, frustrated: 1,
    }])),
    serviceDependencies: vi.fn().mockResolvedValue({ database: [], external: [] }),
    red: vi.fn().mockResolvedValue([]),
    histogram: vi.fn().mockResolvedValue([]),
    facet: vi.fn().mockResolvedValue({ values: [], capped: false }),
    serviceSettings: vi.fn().mockResolvedValue({ apdex_threshold_ms: 500, is_default: true }),
  },
}))

import { api } from '@/lib/core/api'

const routes = [
  { path: '/services', component: { template: '<div />' } },
  { path: '/services/:service', component: ServiceDetailView },
  { path: '/services/:service/operations', component: { template: '<div />' } },
  { path: '/services/:service/dependencies', component: { template: '<div />' } },
  { path: '/traces', component: { template: '<div />' } },
  { path: '/logs', component: { template: '<div />' } },
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
  await flushPromises()
  return { wrapper, router }
}

describe('ServiceDetailView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    timeRange.value = '30m'
    tenant.value = null
    environment.value = null
    vi.clearAllMocks()
  })

  it('sets the "Backend › <service>" breadcrumb on AppShell', async () => {
    const { wrapper } = await mountView('/services/checkout')
    expect(wrapper.findComponent(AppShell).props('crumb')).toBe('Backend › checkout')
    wrapper.unmount()
  })

  it('drives the service timeseries query off the global context window, not local state', async () => {
    setTimeRange('15m')
    const { wrapper } = await mountView('/services/checkout')
    expect(api.serviceTimeseries).toHaveBeenCalledWith(
      'checkout',
      expect.objectContaining({ start: startNs.value, end: endNs.value, buckets: 48 }),
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    )
    wrapper.unmount()
  })

  it('renders the sub-nav — Overview is a tab, not the whole detail page', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.findComponent(ServiceSubNav).exists()).toBe(true)
    wrapper.unmount()
  })

  it('does NOT fetch dependencies — they have their own route now', async () => {
    const { wrapper } = await mountView()
    expect(api.serviceDependencies).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('renders the logs panel, so an outage that lives only in logs is visible here', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.findComponent(ServiceLogPanel).exists()).toBe(true)
    expect(api.histogram).toHaveBeenCalled()
    wrapper.unmount()
  })

  it('charts errors by class rather than as one line', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.text()).toContain('Errors by class')
    wrapper.unmount()
  })

  it('shows the request count and the span count as two separate numbers', async () => {
    // ADR-0003: requests served and spans recorded are different facts about a service.
    const { wrapper } = await mountView()
    expect(wrapper.text()).toContain('14 spans')
    wrapper.unmount()
  })

  it('badges a service whose count did not come from entry spans', async () => {
    ;(api.serviceTimeseries as ReturnType<typeof vi.fn>).mockResolvedValue([bucket({ basis: 'all_spans' })])
    const { wrapper } = await mountView()
    expect(wrapper.find('[data-testid="basis-badge"]').exists()).toBe(true)
    wrapper.unmount()
  })
})

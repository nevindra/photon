// One operation. The three things here that exist nowhere else in the app: the latency heatmap with
// the Apdex thresholds drawn on its own axes, a response-code table (error_rate collapses 502/504/
// 409/422 into one number, and those are four different problems), and slowest-trace exemplars so
// the next click is a waterfall rather than another aggregate.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ServiceOperationView from './ServiceOperationView.vue'
import LatencyHeatmap from '@/components/charts/LatencyHeatmap.vue'
import { timeRange, customRange, tenant, environment } from '@/lib/core/context'

let codes = { values: [] as { value: string; count: number }[], capped: false }

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    serviceTimeseries: vi.fn(() => Promise.resolve([{
      ts: 1000, rate: 1, error_rate: 0.1, count: 10, span_count: 12, basis: 'entry',
      error_count: 1, http_4xx: 3, http_5xx: 1, p50: '1000000', p90: '2000000', p99: '3000000',
      apdex: 0.9, satisfied: 8, tolerating: 1, frustrated: 1,
    }])),
    serviceOperationHeatmap: vi.fn().mockResolvedValue({ columns: [1000], bounds: ['1000000'], cells: [[0, 0, 5]], open_spans: 0 }),
    serviceSettings: vi.fn().mockResolvedValue({ apdex_threshold_ms: 500, is_default: false }),
    tracesFacet: vi.fn(() => Promise.resolve(codes)),
    searchTraces: vi.fn().mockResolvedValue({ traces: [{ trace_id: 'abcdef0123456789aa', root_service: 'checkout', root_name: 'GET /a', start_ts: 1n, duration_ns: 5_000_000n, span_count: 4, error_count: 0, services: [] }], matched_count: 1, elapsed_ms: 1 }),
  },
}))

import { api } from '@/lib/core/api'

const routes = [
  { path: '/services', component: { template: '<div />' } },
  { path: '/services/:service', component: { template: '<div />' } },
  { path: '/services/:service/operations', component: { template: '<div />' } },
  { path: '/services/:service/operations/:operation', component: ServiceOperationView },
  { path: '/traces', component: { template: '<div />' } },
  { path: '/traces/:traceId', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

async function mountView(operation = 'GET %2Fcharges') {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(`/services/checkout/operations/${operation}`)
  await router.isReady()
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const wrapper = mount(
    { components: { TooltipProvider, ServiceOperationView }, template: '<TooltipProvider><ServiceOperationView /></TooltipProvider>' },
    { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
  )
  await flushPromises()
  return { wrapper, router }
}

describe('ServiceOperationView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    timeRange.value = '30m'
    tenant.value = null
    environment.value = null
    codes = { values: [{ value: '200', count: 900 }, { value: '502', count: 30 }, { value: '404', count: 70 }], capped: false }
    vi.clearAllMocks()
  })

  it('renders the heatmap with this service’s own Apdex threshold on its axes', async () => {
    const { wrapper } = await mountView()
    const heat = wrapper.findComponent(LatencyHeatmap)
    expect(heat.exists()).toBe(true)
    expect(heat.props('thresholdMs')).toBe(500)
    wrapper.unmount()
  })

  it('scopes every read to this one operation, quoting the name it needs to', async () => {
    const { wrapper } = await mountView()
    // `GET /charges` has a space: unquoted it would parse as a name plus a stray free-text word.
    const q = (api.tracesFacet as ReturnType<typeof vi.fn>).mock.calls[0][1]
    expect(q).toContain('name:"GET /charges"')
    expect(q).toContain('service.name:checkout')
    wrapper.unmount()
  })

  it('orders response codes worst class first — a 5xx is ours, a 4xx is the caller’s', async () => {
    const { wrapper } = await mountView()
    const rows = wrapper.findAll('[data-testid="code-row"]').map((r) => r.attributes('data-code'))
    expect(rows).toEqual(['502', '404', '200'])
    wrapper.unmount()
  })

  it('says so when the operation records no HTTP status at all', async () => {
    codes = { values: [], capped: false }
    const { wrapper } = await mountView()
    expect(wrapper.text()).toContain('No HTTP status codes')
    wrapper.unmount()
  })

  it('lists slowest traces as exemplars, sorted slowest first by the backend', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.findAll('[data-testid="exemplar-row"]')).toHaveLength(1)
    expect((api.searchTraces as ReturnType<typeof vi.fn>).mock.calls[0][0].sort).toBe('slowest')
    wrapper.unmount()
  })

  it('opens the waterfall for an exemplar rather than another aggregate', async () => {
    const { wrapper, router } = await mountView()
    await wrapper.find('[data-testid="exemplar-row"]').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/traces/abcdef0123456789aa')
    wrapper.unmount()
  })
})

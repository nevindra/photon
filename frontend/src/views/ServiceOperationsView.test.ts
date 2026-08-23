// The operations tab. The load-bearing behaviour is the Entry points / All operations split: an
// operation with no server or consumer spans is work the service DID, not requests it served
// (ADR-0003), and listing the two together without saying so is how `SELECT users` ends up looking
// like an endpoint serving thousands of requests.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ServiceOperationsView from './ServiceOperationsView.vue'
import { timeRange, customRange, tenant, environment } from '@/lib/core/context'

const op = (over = {}) => ({
  service: 'checkout', tenant: null, environment: null, operation: 'GET /a',
  count: 100, rate: 1, span_count: 100, basis: 'entry',
  error_count: 0, error_rate: 0, http_4xx: 0, http_5xx: 0,
  satisfied: 100, tolerating: 0, frustrated: 0, apdex: 1,
  p50: '1000000', p90: '2000000', p99: '3000000', ...over,
})

let rows: ReturnType<typeof op>[] = []

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    red: vi.fn(() => Promise.resolve(rows)),
    serviceSettings: vi.fn().mockResolvedValue({ apdex_threshold_ms: 500, is_default: true }),
  },
}))

const routes = [
  { path: '/services', component: { template: '<div />' } },
  { path: '/services/:service', component: { template: '<div />' } },
  { path: '/services/:service/operations', component: ServiceOperationsView },
  { path: '/services/:service/operations/:operation', component: { template: '<div />' } },
  { path: '/traces', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/services/checkout/operations')
  await router.isReady()
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const wrapper = mount(
    { components: { TooltipProvider, ServiceOperationsView }, template: '<TooltipProvider><ServiceOperationsView /></TooltipProvider>' },
    { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
  )
  await flushPromises()
  return { wrapper, router }
}

describe('ServiceOperationsView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    timeRange.value = '30m'
    tenant.value = null
    environment.value = null
    rows = [op(), op({ operation: 'SELECT users', basis: 'all_spans', count: 4000 })]
    vi.clearAllMocks()
  })

  it('defaults to entry points and hides the service’s own outbound work', async () => {
    const { wrapper } = await mountView()
    const names = wrapper.findAll('[data-testid="operation-row"]').map((r) => r.attributes('data-operation'))
    expect(names).toEqual(['GET /a'])
    wrapper.unmount()
  })

  it('says what it is hiding and why, rather than dropping rows silently', async () => {
    const { wrapper } = await mountView()
    const footer = wrapper.find('[data-testid="operations-footer"]')
    expect(footer.exists()).toBe(true)
    expect(footer.text()).toContain('1 more operation is hidden')
    wrapper.unmount()
  })

  it('has no footer when nothing is hidden', async () => {
    rows = [op()]
    const { wrapper } = await mountView()
    expect(wrapper.find('[data-testid="operations-footer"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('shows the outbound work under All operations, badged as not an entry point', async () => {
    const { wrapper } = await mountView()
    const all = wrapper.findAll('button').find((b) => b.text() === 'All operations')
    await all!.trigger('click')
    await flushPromises()
    const rowsOut = wrapper.findAll('[data-testid="operation-row"]')
    expect(rowsOut).toHaveLength(2)
    const db = rowsOut.find((r) => r.attributes('data-operation') === 'SELECT users')
    expect(db!.find('[data-testid="not-entry-badge"]').exists()).toBe(true)
    wrapper.unmount()
  })

  it('keeps the rollup’s remainder row visible in BOTH modes', async () => {
    // `__other__` is a sum over entry and non-entry alike; dropping it would make the visible rows
    // stop adding up to the service.
    rows = [op(), op({ operation: '__other__', basis: 'all_spans' })]
    const { wrapper } = await mountView()
    const names = wrapper.findAll('[data-testid="operation-row"]').map((r) => r.attributes('data-operation'))
    expect(names).toContain('__other__')
    wrapper.unmount()
  })

  it('renders the remainder as a sentence, never as a clickable endpoint', async () => {
    rows = [op({ operation: '__other__' })]
    const { wrapper } = await mountView()
    const other = wrapper.find('[data-testid="operation-row"][data-other="true"]')
    expect(other.exists()).toBe(true)
    expect(other.attributes('role')).toBeUndefined()
    wrapper.unmount()
  })

  it('scopes the query to this service, quoting a name that needs it', async () => {
    const { wrapper } = await mountView()
    const { api } = await import('@/lib/core/api')
    expect((api.red as ReturnType<typeof vi.fn>).mock.calls[0][0]).toContain('service.name:checkout')
    wrapper.unmount()
  })
})

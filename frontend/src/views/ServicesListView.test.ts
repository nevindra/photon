// Task 8: ServicesListView migrated off LOCAL time onto the app-wide lib/context.ts window
// (owned by the ContextBar mounted in AppShell — Task 5). No test previously existed for this
// view, so this is a new minimal one (kept in TS per the branch's "new test files are .ts"
// convention). Mirrors RumVitalsView.test.js's full-mount shape: a real router + TooltipProvider
// ancestor (AppShell renders NavRail, which renders Reka Tooltips) + a fresh QueryClient, with
// `api.js` fully mocked so nothing hits the network.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ServicesListView from './ServicesListView.vue'
import AppShell from '@/components/common/AppShell.vue'
import { timeRange, customRange, startNs, endNs, setTimeRange, setCustomRange } from '@/lib/core/context'

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    services: vi.fn().mockResolvedValue(['checkout', 'web']),
    red: vi.fn().mockResolvedValue([
      { service: 'checkout', count: 100, rate: 10, error_count: 2, error_rate: 0.02, p50: '1', p90: '2', p99: '3', apdex: 0.9 },
    ]),
    tracesLatency: vi.fn().mockResolvedValue({ buckets: [], p50: 0, p90: 0, p99: 0 }),
    tracesHistogram: vi.fn().mockResolvedValue([]),
  },
}))

import { api } from '@/lib/core/api'

const routes = [
  { path: '/services', component: ServicesListView },
  { path: '/services/:service', component: { template: '<div />' } },
  { path: '/traces', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/services')
  await router.isReady()
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const wrapper = mount(
    { components: { TooltipProvider, ServicesListView }, template: '<TooltipProvider><ServicesListView /></TooltipProvider>' },
    { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
  )
  return { wrapper, router }
}

describe('ServicesListView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    timeRange.value = '30m'
    vi.clearAllMocks()
  })

  it('sets the "Backend › Services" breadcrumb on AppShell (no title/range props of its own)', async () => {
    const { wrapper } = await mountView()
    await flushPromises()
    expect(wrapper.findComponent(AppShell).props('crumb')).toBe('Backend › Services')
    wrapper.unmount()
  })

  it('drives the services RED query off the global context window, not local state', async () => {
    setTimeRange('15m')
    const { wrapper } = await mountView()
    await flushPromises()
    expect(api.red).toHaveBeenCalledWith('', startNs.value, endNs.value, 'service', expect.objectContaining({ signal: expect.any(AbortSignal) }))
    wrapper.unmount()
  })

  it('re-queries when a custom range is applied via context (drag-to-zoom sets the global window)', async () => {
    const { wrapper } = await mountView()
    await flushPromises()
    vi.clearAllMocks()
    setCustomRange({ startMs: 0, endMs: 1000 })
    await flushPromises()
    expect(api.red).toHaveBeenCalledWith('', startNs.value, endNs.value, 'service', expect.objectContaining({ signal: expect.any(AbortSignal) }))
    expect(startNs.value).toBe('0')
    expect(endNs.value).toBe('1000000000')
    wrapper.unmount()
  })
})

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { api } from '@/lib/core/api'
import { mockStorage, mockUsageSeries, mockRetention } from '@/lib/core/mock'
import DataView from '@/views/DataView.vue'

// DataView renders inside AppShell (which mounts NavRail — needs a real router + TooltipProvider
// ancestor, mirroring UptimeDashboard.test.js) and its tab bodies compose the TanStack Query
// data composables, so the mount needs a QueryClient too. We spy `api` so the composables resolve
// from the reshaped mocks instead of hitting a (missing) `/api`.
const routes = [
  { path: '/data', component: DataView },
  { path: '/login', component: { template: '<div />' } },
]

async function mountDataView(initial = '/data') {
  vi.spyOn(api, 'getStorage').mockResolvedValue(structuredClone(mockStorage))
  vi.spyOn(api, 'getUsageSeries').mockResolvedValue(mockUsageSeries('24h'))
  vi.spyOn(api, 'getRetention').mockResolvedValue({ ...mockRetention })
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(initial)
  await router.isReady()
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(
    { components: { TooltipProvider, DataView }, template: '<TooltipProvider><DataView /></TooltipProvider>' },
    { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] } },
  )
}

describe('DataView', () => {
  beforeEach(() => localStorage.clear())
  afterEach(() => vi.restoreAllMocks())

  // The Overview/Storage/Retention/Delete switch is now a query-driven sub-nav folded into the
  // ContextBar (each tab is a NavTabItem RouterLink writing `?tab=`), not a Reka <TabsList>, so the
  // selectors moved from `[role="tab"]` to the tabs' data-testids + their aria-current active state.
  it('renders four tab links: Overview, Storage, Retention, Delete', async () => {
    const w = await mountDataView()
    await flushPromises()
    const joined = ['overview', 'storage', 'retention', 'delete']
      .map((t) => w.get(`[data-testid="data-tab-${t}"]`).text())
      .join(' ')
    expect(joined).toMatch(/Overview/)
    expect(joined).toMatch(/Storage/)
    expect(joined).toMatch(/Retention/)
    expect(joined).toMatch(/Delete/)
  })

  it('syncs the active tab from the ?tab= query param', async () => {
    const w = await mountDataView('/data?tab=retention')
    await flushPromises()
    // The active tab carries aria-current="page"; the others don't.
    expect(w.get('[data-testid="data-tab-retention"]').attributes('aria-current')).toBe('page')
    expect(w.get('[data-testid="data-tab-overview"]').attributes('aria-current')).toBeUndefined()
  })
})

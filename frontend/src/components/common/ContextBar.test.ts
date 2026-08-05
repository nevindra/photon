import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ContextBar from './ContextBar.vue'
import { api } from '@/lib/core/api'
import { tenant, timeRange } from '@/lib/core/context'

// ContextBar owns the tenant picker (useTenants) — mock the registry so no fetch/mock-fallback runs.
vi.mock('@/lib/core/api', () => ({
  api: { mock: false, tenants: vi.fn().mockResolvedValue({ tenants: [] }) },
}))

const queryPlugin = (): [typeof VueQueryPlugin, { queryClient: QueryClient }] => [
  VueQueryPlugin,
  { queryClient: new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } }) },
]

const wrap = (props = {}) =>
  mount(
    {
      components: { ContextBar, TooltipProvider },
      template: '<TooltipProvider><ContextBar v-bind="$attrs" /></TooltipProvider>',
    },
    { attrs: props, attachTo: document.body, global: { plugins: [queryPlugin()] } },
  )

// Variant that fills the middle `search` slot, mirroring how AppShell forwards a view's SearchBar.
const wrapWithSearch = (props = {}) =>
  mount(
    {
      components: { ContextBar, TooltipProvider },
      template:
        '<TooltipProvider><ContextBar v-bind="$attrs"><template #search><div data-testid="search-slot">search here</div></template></ContextBar></TooltipProvider>',
    },
    { attrs: props, attachTo: document.body, global: { plugins: [queryPlugin()] } },
  )

beforeEach(() => {
  tenant.value = null
  timeRange.value = '30m'
})

describe('ContextBar', () => {
  it('renders the crumb', () => {
    const w = wrap({ crumb: 'Home · Overview' })
    expect(w.text()).toContain('Home · Overview')
  })

  it('renders the middle `search` slot when a searchable view forwards one', () => {
    const w = wrapWithSearch({ crumb: 'Logs' })
    expect(w.find('[data-testid="search-slot"]').exists()).toBe(true)
    expect(w.text()).toContain('Logs')
  })

  it('renders without a search slot (non-searchable views leave the middle empty)', () => {
    const w = wrap({ crumb: 'Metrics' })
    expect(w.find('[data-testid="search-slot"]').exists()).toBe(false)
    expect(w.text()).toContain('Metrics')
  })

  it('hides the tenant picker when the registry is empty', async () => {
    const w = wrap({ crumb: 'Logs' })
    await flushPromises()
    expect(w.find('[data-testid="tenant-picker"]').exists()).toBe(false)
  })

  it('shows the tenant picker when tenants exist, reflecting the active tenant', async () => {
    ;(api.tenants as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      tenants: [{ name: 'divtik', token: '…abcd', ui_url: null, created_at: 0 }],
    })
    tenant.value = 'divtik'
    const w = wrap({ crumb: 'Logs' })
    await flushPromises()
    expect(w.get('[data-testid="tenant-picker"]').text()).toContain('divtik')
  })
})

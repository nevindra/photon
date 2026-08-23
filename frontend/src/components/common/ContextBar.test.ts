import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ContextBar from './ContextBar.vue'
import { api } from '@/lib/core/api'
import { tenant, timeRange, environment, environmentDefaulted } from '@/lib/core/context'

// ContextBar owns the tenant picker (useTenants) and the environment picker (a
// `deployment.environment` facet over logs AND spans) — mock all three so no fetch/mock-fallback
// runs.
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    tenants: vi.fn().mockResolvedValue({ tenants: [] }),
    facet: vi.fn().mockResolvedValue({ values: [], capped: false }),
    tracesFacet: vi.fn().mockResolvedValue({ values: [], capped: false }),
  },
}))

// Seeds the LOGS facet; the spans facet stays empty, so these also prove the union survives one
// signal having nothing to report.
const facetValues = (...values: string[]) =>
  (api.facet as ReturnType<typeof vi.fn>).mockResolvedValue({
    values: values.map((value, i) => ({ value, count: 100 - i })),
    capped: false,
  })

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
  environment.value = null
  environmentDefaulted.value = false
  facetValues()
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

// Environment picker. It is populated from a `deployment.environment` facet, so an install whose
// producers never declare one gets nothing to look at.
describe('ContextBar environment picker', () => {
  it('picker_is_hidden_when_the_facet_has_fewer_than_two_values', async () => {
    // Nothing to choose between: one environment (or none) is not a dimension, it is a constant.
    for (const values of [[], ['prod']]) {
      facetValues(...values)
      const w = wrap({ crumb: 'Logs' })
      await flushPromises()
      expect(w.find('[data-testid="env-picker"]').exists()).toBe(false)
      w.unmount()
    }
  })

  it('shows the picker once two environments exist', async () => {
    facetValues('prod', 'staging')
    const w = wrap({ crumb: 'Logs' })
    await flushPromises()
    expect(w.find('[data-testid="env-picker"]').exists()).toBe(true)
  })

  it('prod_is_selected_when_present', async () => {
    facetValues('staging', 'prod', 'dev')
    const w = wrap({ crumb: 'Logs' })
    await flushPromises()
    expect(environment.value).toBe('prod')
    expect(w.get('[data-testid="env-picker"]').text()).toContain('prod')
  })

  it('nothing_is_selected_when_prod_is_absent', async () => {
    facetValues('staging', 'dev')
    const w = wrap({ crumb: 'Logs' })
    await flushPromises()
    expect(environment.value).toBeNull()
    expect(w.get('[data-testid="env-picker"]').text()).toContain('All environments')
  })

  // The facet must NOT be filtered by the selected environment, or picking `prod` would collapse
  // the facet to one value and hide the picker that just got used.
  it('facets over every environment, not the selected one', async () => {
    facetValues('prod', 'staging')
    environment.value = 'staging'
    const w = wrap({ crumb: 'Logs' })
    await flushPromises()
    for (const fn of [api.facet, api.tracesFacet]) {
      expect(fn).toHaveBeenCalledWith(
        'deployment.environment',
        '',
        expect.anything(),
        expect.anything(),
        expect.anything(),
        expect.anything(),
      )
    }
    expect(w.find('[data-testid="env-picker"]').exists()).toBe(true)
  })

  // Trace-only installs are common (APM first, logs later) — the picker has to come from the span
  // facet too, or those installs never see the dimension at all.
  it('unions the spans facet, so a trace-only install still gets a picker', async () => {
    ;(api.tracesFacet as ReturnType<typeof vi.fn>).mockResolvedValue({
      values: [{ value: 'prod', count: 9 }, { value: 'staging', count: 4 }],
      capped: false,
    })
    const w = wrap({ crumb: 'Traces' })
    await flushPromises()
    expect(w.get('[data-testid="env-picker"]').text()).toContain('prod')
    expect(environment.value).toBe('prod')
  })
})

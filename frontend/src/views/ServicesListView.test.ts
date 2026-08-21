// The Services index as a fleet board (ADR-0004). What these tests hold down:
//
//   - One request feeds the whole page (`servicesOverview`), plus a second for the previous window
//     so Silence can be told from never-reported. No `1 + N` fan-out.
//   - The Site band IS the Site filter: clicking a card sets the global tenant, clicking the active
//     one clears it. There is no separate picker.
//   - A Site install (no tenant registry) renders no band at all, and the same page still works.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ServicesListView from './ServicesListView.vue'
import AppShell from '@/components/common/AppShell.vue'
import type { OverviewRow, OverviewSite, ServicesOverview } from '@/lib/core/api'
import {
  timeRange, customRange, tenant, environment, startNs, endNs, prevStartNs, prevEndNs,
  setTimeRange, setCustomRange,
} from '@/lib/core/context'

const row = (over: Record<string, unknown> = {}): OverviewRow => ({
  service: 'checkout', tenant: null, environment: null, operation: null,
  count: 1000, rate: 10, span_count: 1200, basis: 'entry',
  error_count: 0, error_rate: 0, http_4xx: 0, http_5xx: 0,
  satisfied: 1000, tolerating: 0, frustrated: 0, apdex: 1,
  p50: '1000000', p90: '2000000', p99: '3000000', spark: [], error_spark: [],
  ...over,
} as OverviewRow)
const site = (over: Partial<OverviewSite> = {}): OverviewSite =>
  ({ name: 'jkt', status: 'up', last_seen_ms: 1, mode: 'summary', ui_url: null, ...over })

let overview: ServicesOverview = { source: 'raw', window: { start: '0', end: '1', buckets: 30 }, sites: [], rows: [row()] }

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    services: vi.fn().mockResolvedValue(['checkout', 'web']),
    servicesOverview: vi.fn(() => Promise.resolve(overview)),
    red: vi.fn().mockResolvedValue([]),
    serviceDependencies: vi.fn().mockResolvedValue({ database: [], external: [] }),
    alertIncidents: vi.fn().mockResolvedValue([]),
    alertRules: vi.fn().mockResolvedValue([]),
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
  await flushPromises()
  return { wrapper, router }
}

describe('ServicesListView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    timeRange.value = '30m'
    tenant.value = null
    environment.value = null
    overview = { source: 'raw', window: { start: '0', end: '1', buckets: 30 }, sites: [], rows: [row()] }
    vi.clearAllMocks()
  })

  it('sets the "Backend › Services" breadcrumb on AppShell', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.findComponent(AppShell).props('crumb')).toBe('Backend › Services')
    wrapper.unmount()
  })

  it('feeds the whole page from ONE overview request, plus one for the previous window', async () => {
    setTimeRange('15m')
    const { wrapper } = await mountView()
    expect(api.servicesOverview).toHaveBeenCalledWith('', startNs.value, endNs.value, 30, expect.anything())
    // The previous window carries no sparklines: 30 unused series per service is a real cost, and
    // all it exists for is telling "it stopped" from "it never started".
    expect(api.servicesOverview).toHaveBeenCalledWith('', prevStartNs.value, prevEndNs.value, 1, expect.anything())
    expect(api.servicesOverview).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })

  it('re-queries when a custom range is applied via context', async () => {
    const { wrapper } = await mountView()
    vi.clearAllMocks()
    setCustomRange({ startMs: 0, endMs: 1000 })
    await flushPromises()
    expect(api.servicesOverview).toHaveBeenCalledWith('', '0', '1000000000', 30, expect.anything())
    wrapper.unmount()
  })

  it('renders no Site band on a Site install — there is no registry to band', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.find('[data-testid="site-band"]').exists()).toBe(false)
    expect(wrapper.findAll('[data-testid="service-row"]').length).toBe(1)
    wrapper.unmount()
  })

  it('bands the Sites on central, leading with All sites', async () => {
    overview = {
      source: 'raw', window: { start: '0', end: '1', buckets: 30 },
      sites: [site({ name: 'jkt' }), site({ name: 'sgp' })],
      rows: [row({ tenant: 'jkt' }), row({ tenant: 'sgp' })],
    }
    const { wrapper } = await mountView()
    const cards = wrapper.findAll('[data-testid="site-card"]')
    expect(cards.map((c) => c.attributes('data-site'))).toEqual(['__all__', 'jkt', 'sgp'])
    wrapper.unmount()
  })

  it('uses the band as the Site filter, and clicking the active card clears it', async () => {
    overview = {
      source: 'raw', window: { start: '0', end: '1', buckets: 30 },
      sites: [site({ name: 'jkt' })],
      rows: [row({ tenant: 'jkt' })],
    }
    const { wrapper } = await mountView()
    await wrapper.find('[data-site="jkt"]').trigger('click')
    expect(tenant.value).toBe('jkt')
    await wrapper.find('[data-site="jkt"]').trigger('click')
    expect(tenant.value).toBeNull()
    wrapper.unmount()
  })

  it('tints a stale Site and refuses to call its rows OK', async () => {
    overview = {
      source: 'raw', window: { start: '0', end: '1', buckets: 30 },
      sites: [site({ name: 'sgp', status: 'stale' })],
      rows: [row({ tenant: 'sgp', count: 0, error_count: 0, apdex: null })],
    }
    const { wrapper } = await mountView()
    expect(wrapper.find('[data-site="sgp"]').attributes('data-level')).toBe('unknown')
    expect(wrapper.find('[data-testid="site-stale"]').exists()).toBe(true)
    wrapper.unmount()
  })

  it('shows attention cards only for services that are not OK', async () => {
    overview = {
      source: 'raw', window: { start: '0', end: '1', buckets: 30 }, sites: [],
      rows: [row({ service: 'ok-svc' }), row({ service: 'bad-svc', error_rate: 0.5, error_count: 500 })],
    }
    const { wrapper } = await mountView()
    const cards = wrapper.findAll('[data-testid="attention-card"]')
    expect(cards).toHaveLength(1)
    expect(cards[0].attributes('data-service')).toBe('bad-svc')
    wrapper.unmount()
  })

  it('says which read path answered when it was the rollup', async () => {
    overview = { source: 'rollup', window: { start: '0', end: '1', buckets: 30 }, sites: [], rows: [row()] }
    const { wrapper } = await mountView()
    expect(wrapper.find('[data-testid="source-badge"]').exists()).toBe(true)
    wrapper.unmount()
  })

  it('does not badge the source when raw spans answered — that is the normal case', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.find('[data-testid="source-badge"]').exists()).toBe(false)
    wrapper.unmount()
  })
})

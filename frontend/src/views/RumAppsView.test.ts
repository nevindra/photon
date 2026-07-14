// Integration test for the RUM executive summary (`/rum`). A memory router + a fresh QueryClient +
// a <TooltipProvider> (AppShell/NavRail render Reka Tooltips). The api layer is mocked; the view
// fans out one vitals / errors / pages query per app, so all three are mocked. Proves the view
// mounts, aggregates the fleet (KPI strip + a ranked row per app), feeds the global context window
// to the fan-out, and drills into an app on row click.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { startNs, endNs, customRange, clearScope, setTimeRange } from '@/lib/core/context'
import RumAppsView from './RumAppsView.vue'

const t = ([goodMax, poorMin]: [number, number]) => ({ good_max: goodMax, poor_min: poorMin })
const vitalsFor = (app: string) => ({
  app,
  vitals:
    app === 'web-storefront'
      ? [
          { metric: 'web_vitals.lcp', p75: 2800, rating: 'needs', ...t([2500, 4000]), dist: { good: 58, needs: 31, poor: 11, total: 100 } },
          { metric: 'web_vitals.inp', p75: 184, rating: 'good', ...t([200, 500]), dist: { good: 84, needs: 12, poor: 4, total: 100 } },
          { metric: 'web_vitals.cls', p75: 0.06, rating: 'good', ...t([0.1, 0.25]), dist: { good: 88, needs: 9, poor: 3, total: 100 } },
        ]
      : [
          { metric: 'web_vitals.lcp', p75: 1900, rating: 'good', ...t([2500, 4000]), dist: { good: 91, needs: 7, poor: 2, total: 100 } },
          { metric: 'web_vitals.inp', p75: 120, rating: 'good', ...t([200, 500]), dist: { good: 95, needs: 4, poor: 1, total: 100 } },
          { metric: 'web_vitals.cls', p75: 0.03, rating: 'good', ...t([0.1, 0.25]), dist: { good: 96, needs: 3, poor: 1, total: 100 } },
        ],
})

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    rumApps: vi.fn().mockResolvedValue({
      apps: [
        { name: 'web-storefront', key: 'pk_live_web_storefront', allowed_origins: ['https://storefront.example.com'], sample_rate: 1, rate_limit: 5000, created_at: 0 },
        { name: 'admin-dashboard', key: 'pk_live_admin_dashboard', allowed_origins: ['https://admin.example.com'], sample_rate: 1, rate_limit: 5000, created_at: 0 },
      ],
    }),
    rumVitals: vi.fn((app: string) => Promise.resolve(vitalsFor(app))),
    rumErrors: vi.fn((app: string) =>
      Promise.resolve({
        app,
        errors:
          app === 'web-storefront'
            ? [{ fingerprint: 'f1', exception_type: 'TypeError', message: 'boom', count: 42, sessions: 19 }]
            : [],
      }),
    ),
    rumPages: vi.fn((app: string) =>
      Promise.resolve({
        app,
        pages: [{ route: '/checkout', pageviews: 5000, lcp_p75: 4300, inp_p75: 210, cls_p75: 0.09 }],
      }),
    ),
  },
}))

const routes = [
  { path: '/rum', component: { template: '<div />' } },
  { path: '/rum/:appId', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } },
  })
  return [VueQueryPlugin, { queryClient }]
}

async function mountSummary() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/rum')
  await router.isReady()
  const wrapper = mount(
    {
      components: { TooltipProvider, RumAppsView },
      template: '<TooltipProvider><RumAppsView /></TooltipProvider>',
    },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  return { wrapper, router }
}

describe('RumAppsView (executive summary)', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    clearScope()
    setTimeRange('30m')
  })

  it('mounts, shows the KPI strip, and ranks one row per app', async () => {
    const { wrapper } = await mountSummary()
    await flushPromises()
    expect(wrapper.find('[data-testid="rum-summary"]').exists()).toBe(true)
    // Fleet KPI strip is present.
    expect(wrapper.text()).toContain('Apps passing CWV')
    // Manage-apps entry point is present once apps exist.
    expect(wrapper.text()).toContain('Manage apps')
    const rows = wrapper.findAll('[data-testid="rum-app-row"]')
    expect(rows).toHaveLength(2)
    // Unhealthiest app first: web-storefront (LCP needs-improvement) outranks the all-good admin.
    expect(rows[0].attributes('data-app')).toBe('web-storefront')
    wrapper.unmount()
  })

  it('feeds the global context window to the per-app fan-out', async () => {
    const { api } = await import('@/lib/core/api')
    setTimeRange('15m')
    vi.mocked(api.rumVitals).mockClear()
    const { wrapper } = await mountSummary()
    await flushPromises()
    expect(api.rumApps).toHaveBeenCalled()
    const call = vi.mocked(api.rumVitals).mock.calls[0]
    expect(call[1]).toBe(startNs.value)
    expect(call[2]).toBe(endNs.value)
    wrapper.unmount()
  })

  it('drills into an app on row click, carrying the time range', async () => {
    const { wrapper, router } = await mountSummary()
    await flushPromises()
    const push = vi.spyOn(router, 'push')
    await wrapper.get('[data-app="web-storefront"]').trigger('click')
    // correlate() carries the active window (default 30m from beforeEach) onto the hop.
    expect(push).toHaveBeenCalledWith(expect.stringContaining('/rum/web-storefront'))
    expect(push).toHaveBeenCalledWith(expect.stringContaining('range=30m'))
    wrapper.unmount()
  })
})

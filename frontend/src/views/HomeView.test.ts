// Task 13: HomeView overview dashboard. New view → new test in TS (branch convention: NEW test
// files are `.ts`). Mirrors ServiceDetailView.test.ts's full-mount shape: a real router +
// TooltipProvider ancestor (RedTable/StatusDot/Meter chrome) + a fresh QueryClient, with `api.js`
// fully mocked so nothing hits the network. Asserts the dashboard binds the three "worlds"
// (backend RED, RUM vitals, uptime) and renders a backend service row for `checkout`.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import HomeView from './HomeView.vue'
import { api } from '@/lib/core/api'
import { tenant, setTenant, clearTenant } from '@/lib/core/context'

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    red: vi.fn().mockResolvedValue([{ service: 'checkout', rate: 5, error_rate: 0.042, p99: 1.8e9, apdex: 0.9 }]),
    rumApps: vi.fn().mockResolvedValue({
      apps: [{ name: 'web', key: 'pk_live_web', allowed_origins: ['https://web.example.com'], sample_rate: 1, rate_limit: 5000, created_at: 0 }],
    }),
    rumVitals: vi.fn().mockResolvedValue({ app: 'web', vitals: [{ metric: 'web_vitals.lcp', p75: 3100, rating: 'needs-improvement' }] }),
    listMonitors: vi.fn().mockResolvedValue([{ id: '1', name: 'api', last_state: 'up' }]),
    tenantsSummary: vi.fn().mockResolvedValue([]),
  },
}))

// Explicit routes (not a `/:x(.*)*` catch-all) + an initial `push('/home')`, matching
// ServiceDetailView.test.ts: AppShell's NavRail/ContextBar chrome hangs vue-router's render
// against a lone repeating-wildcard route, so we enumerate the destinations Home drills into.
const router = createRouter({
  history: createMemoryHistory(),
  routes: [
    { path: '/home', component: { template: '<div/>' } },
    { path: '/services', component: { template: '<div/>' } },
    { path: '/services/:service', component: { template: '<div/>' } },
    { path: '/rum', component: { template: '<div/>' } },
    { path: '/rum/:appId', component: { template: '<div/>' } },
    { path: '/uptime', component: { template: '<div/>' } },
    { path: '/logs', component: { template: '<div/>' } },
    { path: '/traces', component: { template: '<div/>' } },
    { path: '/login', component: { template: '<div/>' } },
  ],
})

describe('HomeView', () => {
  beforeEach(() => window.history.replaceState(null, '', '/home'))

  it('renders the KPI strip and the backend service row', async () => {
    router.push('/home')
    await router.isReady()
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    const w = mount(
      { components: { HomeView, TooltipProvider }, template: '<TooltipProvider><HomeView/></TooltipProvider>' },
      { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
    )
    await flushPromises()
    expect(w.get('[data-testid="home"]').text()).toContain('checkout')
  })

  it('hides the tenant board when the tenants summary is empty', async () => {
    router.push('/home')
    await router.isReady()
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    const w = mount(
      { components: { HomeView, TooltipProvider }, template: '<TooltipProvider><HomeView/></TooltipProvider>' },
      { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
    )
    await flushPromises()
    expect(w.find('[data-testid="home-tenants"]').exists()).toBe(false)
  })

  it('renders a tenant board with one card per tenant, down-tenants marked unreachable', async () => {
    ;(api.tenantsSummary as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { name: 'acme', mode: 'summary', status: 'up', last_seen_ms: Date.now(), ingest_rows_per_sec: 12, open_incidents: 0, hot_bytes: 1024, ui_url: null, spark: [] },
      { name: 'globex', mode: 'full', status: 'down', last_seen_ms: Date.now() - 600_000, ingest_rows_per_sec: 0, open_incidents: 2, hot_bytes: 0, ui_url: null, spark: [] },
    ])
    router.push('/home')
    await router.isReady()
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    const w = mount(
      { components: { HomeView, TooltipProvider }, template: '<TooltipProvider><HomeView/></TooltipProvider>' },
      { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
    )
    await flushPromises()
    const board = w.get('[data-testid="home-tenants"]')
    expect(board.text()).toContain('acme')
    expect(board.text()).toContain('globex')
    expect(board.text()).toContain('Unreachable')
  })

  it('narrows the board to the active tenant card when the tenant filter is set', async () => {
    setTenant('globex')
    ;(api.tenantsSummary as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { name: 'globex', mode: 'full', status: 'up', last_seen_ms: Date.now(), ingest_rows_per_sec: 3, open_incidents: 0, hot_bytes: 0, ui_url: null, spark: [] },
      { name: 'initech', mode: 'summary', status: 'up', last_seen_ms: Date.now(), ingest_rows_per_sec: 1, open_incidents: 0, hot_bytes: 0, ui_url: null, spark: [] },
    ])
    router.push('/home')
    await router.isReady()
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    const w = mount(
      { components: { HomeView, TooltipProvider }, template: '<TooltipProvider><HomeView/></TooltipProvider>' },
      { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
    )
    await flushPromises()

    const cards = w.findAll('[data-testid="tenant-card"]')
    expect(cards.length).toBe(1)
    expect(cards[0].attributes('data-tenant')).toBe('globex')
    expect(w.text()).toContain('1 more hidden by the tenant filter')
    clearTenant()
  })

  // Clicking a full-mode tenant card sets the tenant context dimension and drills into Logs (the
  // tenant then narrows the log search via `tenantQueryTerm()`); a summary-mode card has no local
  // data to browse, so it links out to the tenant's own UI instead.
  it('clicking a full-mode tenant card sets the tenant context and opens Logs', async () => {
    clearTenant()
    ;(api.tenantsSummary as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { name: 'globex', mode: 'full', status: 'up', last_seen_ms: Date.now(), ingest_rows_per_sec: 3, open_incidents: 0, hot_bytes: 0, ui_url: null, spark: [] },
    ])
    router.push('/home')
    await router.isReady()
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    const w = mount(
      { components: { HomeView, TooltipProvider }, template: '<TooltipProvider><HomeView/></TooltipProvider>' },
      { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
    )
    await flushPromises()

    await w.get('[data-tenant="globex"]').trigger('click')
    await flushPromises()

    expect(tenant.value).toBe('globex')
    expect(router.currentRoute.value.path).toBe('/logs')
  })

  // A subset-mirroring tenant (`full:traces`) deliberately ships no logs — drilling into /logs
  // would land on an empty view, so the card routes to the first signal it actually mirrors.
  it('clicking a full:traces tenant card drills into Traces, not Logs', async () => {
    clearTenant()
    ;(api.tenantsSummary as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { name: 'divtik', mode: 'full:traces', status: 'up', last_seen_ms: Date.now(), ingest_rows_per_sec: 3, open_incidents: 0, hot_bytes: 0, ui_url: null, spark: [] },
    ])
    router.push('/home')
    await router.isReady()
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    const w = mount(
      { components: { HomeView, TooltipProvider }, template: '<TooltipProvider><HomeView/></TooltipProvider>' },
      { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
    )
    await flushPromises()

    await w.get('[data-tenant="divtik"]').trigger('click')
    await flushPromises()

    expect(tenant.value).toBe('divtik')
    expect(router.currentRoute.value.path).toBe('/traces')
    clearTenant()
  })
})

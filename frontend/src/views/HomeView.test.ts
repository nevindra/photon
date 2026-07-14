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

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    red: vi.fn().mockResolvedValue([{ service: 'checkout', rate: 5, error_rate: 0.042, p99: 1.8e9, apdex: 0.9 }]),
    rumApps: vi.fn().mockResolvedValue({
      apps: [{ name: 'web', key: 'pk_live_web', allowed_origins: ['https://web.example.com'], sample_rate: 1, rate_limit: 5000, created_at: 0 }],
    }),
    rumVitals: vi.fn().mockResolvedValue({ app: 'web', vitals: [{ metric: 'web_vitals.lcp', p75: 3100, rating: 'needs-improvement' }] }),
    listMonitors: vi.fn().mockResolvedValue([{ id: '1', name: 'api', last_state: 'up' }]),
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
})

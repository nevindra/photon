// Integration test for the Ops dashboard (`/uptime`). Covers the monitor filter, which is both a
// plain UI affordance and the landing spot for the service → Uptime pivot (`/uptime?q=<service>`,
// see lib/core/useCorrelate.ts). A Monitor carries no service field — only `name` and `target` —
// so the pivot is an honest name/target text match; these tests pin that contract.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import UptimeDashboard from './UptimeDashboard.vue'

// `vi.mock` is hoisted above every top-level binding, so the fixture the factory closes over has
// to be hoisted with it.
const { MONITORS } = vi.hoisted(() => {
  const monitor = (id: string, name: string, target: string) => ({
    id,
    name,
    type: 'http',
    target,
    interval_secs: 60,
    timeout_secs: 10,
    retries: 1,
    ignore_tls: false,
    follow_redirects: true,
    channel_ids: [],
    enabled: true,
    last_state: 'up',
    last_check_at: 0,
    last_latency_ms: 42,
    created_at: 0,
    updated_at: 0,
  })
  return {
    MONITORS: [
      monitor('m1', 'checkout-api health', 'https://checkout.example.com/healthz'),
      monitor('m2', 'Admin dashboard', 'https://admin.example.com'),
      monitor('m3', 'edge ping', 'https://checkout-api.internal/ping'),
    ],
  }
})

vi.mock('@/lib/core/api', () => ({
  api: { mock: false, listMonitors: vi.fn().mockResolvedValue(MONITORS) },
}))

const routes = [
  { path: '/uptime', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

async function mountDashboard(initial = '/uptime') {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(initial)
  await router.isReady()
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } },
  })
  const wrapper = mount(
    {
      components: { TooltipProvider, UptimeDashboard },
      template: '<TooltipProvider><UptimeDashboard /></TooltipProvider>',
    },
    { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
  )
  await flushPromises()
  return { wrapper, router }
}

const rowNames = (wrapper: ReturnType<typeof mount>) =>
  MONITORS.filter((m) => wrapper.text().includes(m.name)).map((m) => m.name)

describe('UptimeDashboard monitor filter', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
  })

  it('lists every monitor unfiltered', async () => {
    const { wrapper } = await mountDashboard()
    expect(rowNames(wrapper)).toHaveLength(3)
    wrapper.unmount()
  })

  it('seeds the filter from ?q= so the service → Uptime pivot lands scoped', async () => {
    const { wrapper } = await mountDashboard('/uptime?q=checkout')
    const names = rowNames(wrapper)
    // Matches on the NAME (m1) and on the TARGET (m3) — a monitor has no service field.
    expect(names).toContain('checkout-api health')
    expect(names).toContain('edge ping')
    expect(names).not.toContain('Admin dashboard')
    wrapper.unmount()
  })

  it('matches case-insensitively and mirrors typing back into ?q=', async () => {
    const { wrapper } = await mountDashboard()
    await wrapper.get('[data-testid="uptime-filter"]').setValue('ADMIN')
    await flushPromises()
    expect(rowNames(wrapper)).toEqual(['Admin dashboard'])
    expect(new URLSearchParams(window.location.search).get('q')).toBe('ADMIN')
    wrapper.unmount()
  })

  it('distinguishes "no matches" from "no monitors yet"', async () => {
    const { wrapper } = await mountDashboard('/uptime?q=nothing-matches-this')
    expect(wrapper.find('[data-testid="uptime-no-matches"]').exists()).toBe(true)
    expect(wrapper.text()).not.toContain('No monitors yet')
    wrapper.unmount()
  })

  it('drops ?q= from the URL when the filter is cleared', async () => {
    const { wrapper } = await mountDashboard('/uptime?q=admin')
    await wrapper.get('[data-testid="uptime-filter"]').setValue('')
    await flushPromises()
    expect(new URLSearchParams(window.location.search).get('q')).toBeNull()
    expect(rowNames(wrapper)).toHaveLength(3)
    wrapper.unmount()
  })
})

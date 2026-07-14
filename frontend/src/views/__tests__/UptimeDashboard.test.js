import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { TooltipProvider } from '@/components/ui/tooltip'

vi.mock('@/lib/uptime/uptimeQueries', () => ({
  useMonitors: () => ({ data: { value: [
    { id: 'a', name: 'Alpha', target: 'https://a.test', type: 'http', enabled: true, last_state: 'up', last_latency_ms: 12 },
    { id: 'b', name: 'Bravo', target: 'https://b.test', type: 'tcp', enabled: true, last_state: 'down', last_latency_ms: null },
  ] }, isLoading: { value: false }, isError: { value: false } }),
  useCreateMonitor: () => ({ mutate: vi.fn(), isPending: { value: false } }),
  useHeartbeats: () => ({ data: { value: { heartbeats: [], uptime_pct: 100 } } }),
  useMonitor: () => ({ data: { value: null } }),
  useIncidents: () => ({ data: { value: [] } }),
  useUpdateMonitor: () => ({ mutate: vi.fn() }),
  useDeleteMonitor: () => ({ mutate: vi.fn() }),
  usePauseMonitor: () => ({ mutate: vi.fn() }),
  useResumeMonitor: () => ({ mutate: vi.fn() }),
}))

import UptimeDashboard from '@/views/UptimeDashboard.vue'

// UptimeDashboard now renders inside AppShell (like every other view), which mounts NavRail —
// NavRail uses useRoute/useRouter plus Reka <Tooltip>s, so the mount needs a real router and a
// TooltipProvider ancestor. Mirrors the harness in LogsView.test.js / AppShell.test.js.
const routes = [
  { path: '/uptime', component: UptimeDashboard },
  { path: '/login', component: { template: '<div />' } },
]

async function mountUptime() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/uptime')
  await router.isReady()
  return mount(
    { components: { TooltipProvider, UptimeDashboard }, template: '<TooltipProvider><UptimeDashboard /></TooltipProvider>' },
    { global: { plugins: [router] } },
  )
}

describe('UptimeDashboard', () => {
  beforeEach(() => localStorage.clear())

  it('renders a row per monitor (table view by default)', async () => {
    const w = await mountUptime()
    await flushPromises()
    expect(w.text()).toContain('Alpha')
    expect(w.text()).toContain('Bravo')
    expect(w.find('table').exists()).toBe(true)
    // Migrated onto the global ContextBar (Task 9): the Ops crumb renders.
    expect(w.text()).toContain('Ops')
  })

  it('switches to the cards layout via the view toggle', async () => {
    const w = await mountUptime()
    await flushPromises()
    const cardsBtn = w.findAll('button').find((b) => b.text().toLowerCase() === 'cards')
    expect(cardsBtn).toBeTruthy()
    await cardsBtn.trigger('click')
    expect(w.find('table').exists()).toBe(false)
    expect(w.text()).toContain('Alpha') // still listed, now as cards
  })
})

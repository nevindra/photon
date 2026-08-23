// `/home` — the Attention Board. Full-mount shape borrowed from ServiceDetailView.test.ts: a real
// router + TooltipProvider ancestor + a fresh QueryClient, with `api` fully mocked so nothing hits
// the network. What is asserted here is the page's promise: every world is always tallied, the
// things that failed a judgment are listed worst-first, and nothing that fired is ever hidden.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import HomeView from './HomeView.vue'
import { api } from '@/lib/core/api'
import { clearTenant, setTenant } from '@/lib/core/context'
import { setWatchRange } from '@/lib/home/watchRange'

const HEALTHY = { service: 'checkout', count: 500, rate: 5, error_rate: 0, p99: 1.8e8, apdex: 0.98 }
const CRITICAL = { service: 'payments-api', count: 500, rate: 5, error_rate: 0.082, p99: 1.4e9, apdex: 0.62 }

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    red: vi.fn(),
    rumApps: vi.fn(),
    rumVitals: vi.fn(),
    listMonitors: vi.fn(),
    infraHosts: vi.fn(),
    tenants: vi.fn(),
    tenantsSummary: vi.fn(),
    alertRules: vi.fn(),
    alertIncidents: vi.fn(),
  },
}))

const mocked = (fn: unknown) => fn as ReturnType<typeof vi.fn>

const router = createRouter({
  history: createMemoryHistory(),
  routes: [
    { path: '/home', component: { template: '<div/>' } },
    { path: '/services', component: { template: '<div/>' } },
    { path: '/services/:service', component: { template: '<div/>' } },
    { path: '/rum', component: { template: '<div/>' } },
    { path: '/rum/:appId', component: { template: '<div/>' } },
    { path: '/infra', component: { template: '<div/>' } },
    { path: '/infra/:host', component: { template: '<div/>' } },
    { path: '/uptime', component: { template: '<div/>' } },
    { path: '/alerts', component: { template: '<div/>' } },
    { path: '/tenants', component: { template: '<div/>' } },
    { path: '/logs', component: { template: '<div/>' } },
    { path: '/login', component: { template: '<div/>' } },
  ],
})

async function mountBoard() {
  await router.push('/home')
  await router.isReady()
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const w = mount(
    { components: { HomeView, TooltipProvider }, template: '<TooltipProvider><HomeView/></TooltipProvider>' },
    { global: { plugins: [router, [VueQueryPlugin, { queryClient }]] }, attachTo: document.body },
  )
  await flushPromises()
  return w
}

describe('HomeView · attention board', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/home')
    clearTenant()
    setWatchRange('15m')
    mocked(api.red).mockResolvedValue([HEALTHY])
    mocked(api.rumApps).mockResolvedValue({ apps: [] })
    mocked(api.rumVitals).mockResolvedValue({ app: '', vitals: [] })
    mocked(api.listMonitors).mockResolvedValue([])
    mocked(api.infraHosts).mockResolvedValue({ hosts: [] })
    mocked(api.tenants).mockResolvedValue({ tenants: [] })
    mocked(api.tenantsSummary).mockResolvedValue([])
    mocked(api.alertRules).mockResolvedValue([])
    mocked(api.alertIncidents).mockResolvedValue([])
  })
  afterEach(() => vi.clearAllMocks())

  it('tallies every world even when nothing is wrong', async () => {
    const w = await mountBoard()
    const rollup = w.get('[data-testid="world-rollup"]')
    for (const world of ['frontend', 'backend', 'infra', 'ops']) {
      expect(rollup.find(`[data-world="${world}"]`).exists()).toBe(true)
    }
    // The proof that N subjects are watched is what makes an empty attention list believable.
    expect(w.get('[data-testid="world-backend"]').text()).toContain('1 services')
    expect(w.get('[data-testid="world-backend"]').text()).toContain('1 OK')
    expect(w.find('[data-testid="all-clear"]').exists()).toBe(true)
    expect(w.find('[data-testid="attention-list"]').exists()).toBe(false)
    w.unmount()
  })

  it('lists a failing subject with its native reason and drills into it', async () => {
    mocked(api.red).mockResolvedValue([HEALTHY, CRITICAL])
    const w = await mountBoard()
    const list = w.get('[data-testid="attention-list"]')
    expect(list.text()).toContain('payments-api')
    expect(list.text()).toContain('Critical')
    expect(list.text()).toContain('8.2%')
    // Healthy subjects are tallied, never listed.
    expect(list.text()).not.toContain('checkout')

    await list.get('[data-subject="service:payments-api"]').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/services/payments-api')
    w.unmount()
  })

  // The hole the board exists to close: zero traffic used to read as `idle` and vanish.
  it('reports a service that stopped reporting as Down', async () => {
    mocked(api.red).mockImplementation((_q: string, startNs: string) =>
      // The previous range starts a full Watch Range earlier — that copy is what Silence is
      // detected against. 20m sits between the two starts (15m and 30m ago) whatever minute
      // `watchNowMs` snapped to.
      Promise.resolve(BigInt(startNs) < BigInt(Date.now() - 20 * 60_000) * 1_000_000n ? [{ ...HEALTHY, service: 'ledger-worker' }] : []),
    )
    const w = await mountBoard()
    const row = w.get('[data-subject="service:ledger-worker"]')
    expect(row.attributes('data-level')).toBe('down')
    expect(row.text()).toContain('No spans')
    w.unmount()
  })

  it('narrows the board when a roll-up card is clicked, and releases on a second click', async () => {
    mocked(api.red).mockResolvedValue([CRITICAL])
    mocked(api.listMonitors).mockResolvedValue([
      { id: 'm1', name: 'checkout probe', type: 'http', target: 'https://x', interval_secs: 60, timeout_secs: 5, retries: 1, enabled: true, last_state: 'down' },
    ])
    const w = await mountBoard()
    expect(w.get('[data-testid="attention-list"]').text()).toContain('payments-api')

    await w.get('[data-testid="world-ops"]').trigger('click')
    await flushPromises()
    expect(w.get('[data-testid="world-ops"]').attributes('aria-pressed')).toBe('true')
    expect(w.get('[data-testid="attention-list"]').text()).not.toContain('payments-api')
    expect(w.get('[data-testid="attention-list"]').text()).toContain('checkout probe')
    expect(new URLSearchParams(window.location.search).get('world')).toBe('ops')

    await w.get('[data-testid="world-ops"]').trigger('click')
    await flushPromises()
    expect(w.get('[data-testid="attention-list"]').text()).toContain('payments-api')
    expect(new URLSearchParams(window.location.search).get('world')).toBeNull()
    w.unmount()
  })

  it('raises a subject to the incident severity and badges it', async () => {
    mocked(api.red).mockResolvedValue([HEALTHY])
    mocked(api.alertRules).mockResolvedValue([
      {
        id: 'r1', name: 'Checkout error rate', description: null, enabled: true, signal: 'traces',
        condition: { signal: 'traces', service: 'checkout', kind: 'error_rate', window_secs: 300, cmp: 'gt', threshold: 0.05 },
        for_secs: 60, interval_secs: 60, severity: 'critical', channel_ids: [], created_at: 0, updated_at: 0,
      },
    ])
    mocked(api.alertIncidents).mockResolvedValue([
      { id: 1, rule_id: 'r1', series_key: '', started_at: Math.floor(Date.now() / 1000) - 300, ended_at: null, peak_value: 0.08, severity: 'critical', summary: 'error_rate 8.2% > 5%' },
    ])
    const w = await mountBoard()
    const row = w.get('[data-subject="service:checkout"]')
    expect(row.attributes('data-level')).toBe('critical')
    expect(row.text()).toContain('1 incident')
    w.unmount()
  })

  // A rule genuinely about the fleet resolves to no Subject. Hiding it would make the page lie in
  // exactly the case the page exists for.
  it('shows incidents that resolve to no subject in their own banner', async () => {
    mocked(api.alertRules).mockResolvedValue([
      {
        id: 'r9', name: 'Any host CPU saturated', description: null, enabled: true, signal: 'metrics',
        condition: { signal: 'metrics', metric_name: 'system.cpu.utilization', agg: 'avg', window_secs: 300, cmp: 'gt', threshold: 0.9 },
        for_secs: 60, interval_secs: 60, severity: 'warning', channel_ids: [], created_at: 0, updated_at: 0,
      },
    ])
    mocked(api.alertIncidents).mockResolvedValue([
      { id: 7, rule_id: 'r9', series_key: '', started_at: Math.floor(Date.now() / 1000) - 60, ended_at: null, peak_value: 0.95, severity: 'warning', summary: 'cpu 95% > 90%' },
    ])
    const w = await mountBoard()
    const banner = w.get('[data-testid="unattributed-incidents"]')
    expect(banner.text()).toContain('1 incident with no subject')
    await banner.trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/alerts')
    w.unmount()
  })

  it('adds a Sites world only on an install with a tenant registry', async () => {
    const w1 = await mountBoard()
    expect(w1.find('[data-world="sites"]').exists()).toBe(false)
    w1.unmount()

    mocked(api.tenantsSummary).mockResolvedValue([
      { name: 'ap-south', mode: 'summary', status: 'down', last_seen_ms: Date.now() - 900_000, ingest_rows_per_sec: 0, open_incidents: 3, hot_bytes: 0, ui_url: null, spark: [] },
    ])
    const w2 = await mountBoard()
    expect(w2.get('[data-world="sites"]').text()).toContain('1 sites')
    expect(w2.get('[data-subject="site:ap-south"]').attributes('data-level')).toBe('down')
    w2.unmount()
  })

  // The ContextBar's tenant picker turns the board into a read-only lens over one Site. Services,
  // RUM apps and hosts carry the tenant dimension; uptime monitors and the alert engine do not —
  // they are Central's own. Leaving Ops on screen under an "eu-west" filter would claim Central's
  // probes belong to eu-west.
  it('narrows to one Site under a tenant filter, and drops the worlds that cannot narrow', async () => {
    mocked(api.tenants).mockResolvedValue({ tenants: [{ name: 'eu-west' }] })
    mocked(api.tenantsSummary).mockResolvedValue([
      { name: 'eu-west', mode: 'full', status: 'up', last_seen_ms: Date.now(), ingest_rows_per_sec: 12, open_incidents: 0, hot_bytes: 0, ui_url: null, spark: [] },
      { name: 'us-east', mode: 'full', status: 'down', last_seen_ms: Date.now() - 900_000, ingest_rows_per_sec: 0, open_incidents: 1, hot_bytes: 0, ui_url: null, spark: [] },
    ])
    mocked(api.listMonitors).mockResolvedValue([
      { id: 'm1', name: 'central probe', type: 'http', target: 'https://x', interval_secs: 60, timeout_secs: 5, retries: 1, enabled: true, last_state: 'down' },
    ])
    // A row the backend did not stamp with a tenant. Under a lens it must NOT be labelled
    // "central" — the whole board is eu-west, and the breadcrumb already says so.
    mocked(api.red).mockResolvedValue([CRITICAL])
    setTenant('eu-west')
    const w = await mountBoard()

    const row = w.get('[data-subject="service:payments-api"]')
    expect(row.text()).not.toContain('central')
    expect(row.text()).not.toContain('sites')
    expect(w.find('[data-world="ops"]').exists()).toBe(false)
    expect(w.text()).not.toContain('central probe')
    // The Sites world narrows to the Site being inspected, like every other view does.
    expect(w.get('[data-world="sites"]').text()).toContain('1 sites')
    expect(w.find('[data-subject="site:us-east"]').exists()).toBe(false)
    // Every tenant-aware query carries the grammar term.
    expect(mocked(api.red).mock.calls[0][0]).toBe('tenant:eu-west')
    expect(mocked(api.rumApps).mock.calls[0][1]).toBe('eu-west')
    w.unmount()
    clearTenant()
  })

  // The board judges over its own bounded Watch Range; two time controls in one header would be
  // two sources of truth.
  it('replaces the global time picker with the Watch Range picker', async () => {
    const w = await mountBoard()
    expect(w.find('[data-testid="watch-range"]').exists()).toBe(true)
    expect(w.find('[data-testid="time-range-picker"]').exists()).toBe(false)
    w.unmount()
  })
})

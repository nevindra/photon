// Integration test for the Infrastructure host detail (`/infra/:host`) Processes table. A memory
// router + a fresh QueryClient + a <TooltipProvider> (AppShell/NavRail render Reka Tooltips),
// mirroring InfraHostsView.test.ts's mount harness. The api layer is mocked; the heavy chart
// children (HostStatTiles/HostResourcePanels, which drive uPlot — unhappy in jsdom) are stubbed so
// the test stays focused on the process table. Proves the view renders one row per process, sorted
// by CPU descending (heaviest first) by default, and re-sorts on a header click.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { customRange, setTimeRange } from '@/lib/core/context'
import { api } from '@/lib/core/api'
import InfraHostDetailView from './InfraHostDetailView.vue'

// NOTE: `vi.mock` is hoisted to the top of the file, so its factory must not close over any
// top-level bindings — the last-seen ns literal is inlined below rather than referenced via a const.
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    infraHost: vi.fn().mockResolvedValue({
      host: 'web-1',
      os: 'linux',
      cores: 8,
      totalRamBytes: 16 * 1024 ** 3,
      gpus: [],
      lastSeenNs: '1700000000000000000',
    }),
    infraHostSeries: vi.fn().mockResolvedValue({ resource: 'cpu', series: [] }),
    infraHostProcesses: vi.fn().mockResolvedValue({
      // Deliberately NOT pre-sorted, so the default CPU-desc sort has to do real work.
      processes: [
        { process: 'cron-loop', cpuPct: 3.1, rssBytes: 48 * 1024 ** 2, fds: 16, threads: 3, restarts: 0, lastSeenNs: '1700000000000000000' },
        { process: 'api', cpuPct: 42.5, rssBytes: 512 * 1024 ** 2, fds: 128, threads: 12, restarts: 1, lastSeenNs: '1700000000000000000' },
        { process: 'worker', cpuPct: 18.3, rssBytes: 256 * 1024 ** 2, fds: 64, threads: 8, restarts: 2, lastSeenNs: '1700000000000000000' },
      ],
    }),
  },
}))

const routes = [
  { path: '/infra', component: { template: '<div />' } },
  { path: '/infra/:host', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } },
  })
  return [VueQueryPlugin, { queryClient }]
}

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/infra/web-1')
  await router.isReady()
  const wrapper = mount(
    {
      components: { TooltipProvider, InfraHostDetailView },
      template: '<TooltipProvider><InfraHostDetailView /></TooltipProvider>',
    },
    {
      global: {
        plugins: [router, queryPlugin()],
        // The trend/glance charts (uPlot) are irrelevant here and misbehave in jsdom — stub them.
        stubs: { HostStatTiles: true, HostResourcePanels: true },
      },
      attachTo: document.body,
    },
  )
  return { wrapper, router }
}

describe('InfraHostDetailView processes table', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    setTimeRange('30m')
  })

  it('renders one row per process, sorted by CPU descending by default', async () => {
    const { wrapper } = await mountView()
    await flushPromises()

    expect(wrapper.find('[data-testid="host-processes"]').exists()).toBe(true)
    const rows = wrapper.findAll('[data-testid="process-row"]')
    expect(rows).toHaveLength(3)
    // Heaviest CPU first: api (42.5) > worker (18.3) > cron-loop (3.1).
    expect(rows[0].attributes('data-process')).toBe('api')
    expect(rows[1].attributes('data-process')).toBe('worker')
    expect(rows[2].attributes('data-process')).toBe('cron-loop')
    // RSS is rendered human-readably (formatBytes), not as raw bytes.
    expect(rows[0].text()).toContain('512.0 MB')
    expect(rows[0].text()).toContain('42.5%')

    wrapper.unmount()
  })

  it('re-sorts ascending by CPU when the CPU header is clicked', async () => {
    const { wrapper } = await mountView()
    await flushPromises()

    const cpuHeader = wrapper.findAll('th').find((th) => th.text().startsWith('CPU'))
    expect(cpuHeader).toBeTruthy()
    await cpuHeader!.trigger('click')

    const rows = wrapper.findAll('[data-testid="process-row"]')
    // Ascending now: cron-loop (3.1) < worker (18.3) < api (42.5).
    expect(rows[0].attributes('data-process')).toBe('cron-loop')
    expect(rows[2].attributes('data-process')).toBe('api')

    wrapper.unmount()
  })

  it('renders null numeric metrics as — and sorts them last', async () => {
    // A process reporting only CPU — every other numeric metric is null this window.
    vi.mocked(api.infraHostProcesses).mockResolvedValueOnce({
      processes: [
        { process: 'api', cpuPct: 42.5, rssBytes: 512 * 1024 ** 2, fds: 128, threads: 12, restarts: 1, lastSeenNs: '1700000000000000000' },
        { process: 'sidecar', cpuPct: 1.4, rssBytes: null, fds: null, threads: null, restarts: null, lastSeenNs: '1700000000000000000' },
      ],
    })
    const { wrapper } = await mountView()
    await flushPromises()

    // The null-metric row renders every missing numeric as an em dash.
    const sidecar = wrapper.findAll('[data-testid="process-row"]').find((r) => r.attributes('data-process') === 'sidecar')
    expect(sidecar).toBeTruthy()
    expect(sidecar!.text()).toContain('—')

    // Sort by RSS descending: the row whose rssBytes is null sorts last regardless of direction.
    const rssHeader = wrapper.findAll('th').find((th) => th.text().startsWith('RSS'))
    await rssHeader!.trigger('click') // -> desc
    let rows = wrapper.findAll('[data-testid="process-row"]')
    expect(rows[rows.length - 1].attributes('data-process')).toBe('sidecar')

    // And still last when the same column is toggled to ascending.
    await rssHeader!.trigger('click') // -> asc
    rows = wrapper.findAll('[data-testid="process-row"]')
    expect(rows[rows.length - 1].attributes('data-process')).toBe('sidecar')

    wrapper.unmount()
  })
})

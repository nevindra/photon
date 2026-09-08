// Integration test for the Infrastructure host list (`/infra`). A memory router + a fresh
// QueryClient + a <TooltipProvider> (AppShell/NavRail render Reka Tooltips), mirroring
// RumAppsView.test.ts's mount harness. The api layer is mocked. Proves the view mounts, unions the
// metrics hosts and the managed agents into ONE table ranked worst first, counts them in the
// summary line that doubles as the filter, and drills into a host on row click.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { customRange, setTimeRange } from '@/lib/core/context'
import InfraHostsView from './InfraHostsView.vue'

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    infraHosts: vi.fn().mockResolvedValue({
      hosts: [
        { host: 'web-1', cpuUtil: 0.3, memUtil: 0.5, diskUtil: 0.4, diskUtilAvg: 0.4, diskGroups: 1, gpuUtil: null, gpuUtilAvg: null, gpuGroups: 0, lastSeenNs: '1700000000000000000', hasGpu: false },
        { host: 'gpu-node-1', cpuUtil: 0.6, memUtil: 0.7, diskUtil: 0.9, diskUtilAvg: 0.61, diskGroups: 3, gpuUtil: 0.13, gpuUtilAvg: 0.13, gpuGroups: 1, lastSeenNs: '1700000000000000000', hasGpu: true },
      ],
    }),
    agents: vi.fn().mockResolvedValue({
      hosts: [
        { id: 'a1', host: 'web-1', site: null, status: 'online', agent_version: '1.7.0', latest_agent_version: null, capabilities: ['read'], poll_secs: 15, last_seen_ms: Date.now(), enrolled_ms: 0, labels: {}, docker: { version: '27.1.1', api_version: '1.46', storage_driver: 'overlay2', os: 'Ubuntu 22.04', arch: 'x86_64', containers_running: 2, containers_total: 2, images: 3, disk: { images_bytes: 0, volumes_bytes: 0, build_cache_bytes: 0, reclaimable_bytes: 0, total_bytes: null, used_bytes: null } } },
        { id: 'a2', host: 'dsasa', site: null, status: 'pending', agent_version: null, latest_agent_version: null, capabilities: [], poll_secs: 15, last_seen_ms: null, enrolled_ms: 0, labels: {}, docker: null },
      ],
    }),
  },
}))

const routes = [
  { path: '/infra', component: { template: '<div />' } },
  { path: '/infra/:host', component: { template: '<div />' } },
  { path: '/infra/:host/agent', component: { template: '<div />' } },
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
  router.push('/infra')
  await router.isReady()
  const wrapper = mount(
    {
      components: { TooltipProvider, InfraHostsView },
      template: '<TooltipProvider><InfraHostsView /></TooltipProvider>',
    },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  await flushPromises()
  return { wrapper, router }
}

describe('InfraHostsView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    setTimeRange('30m')
  })

  it('renders one row per host from both sources, worst first, with the reason in words', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.find('[data-testid="infra-hosts"]').exists()).toBe(true)
    const rows = wrapper.findAll('[data-testid="fleet-row"]')
    expect(rows.map((r) => [r.attributes('data-host'), r.attributes('data-level')])).toEqual([
      ['gpu-node-1', 'error'],
      ['dsasa', 'pending'],
      ['web-1', 'ok'],
    ])
    expect(rows[0].find('[data-testid="fleet-verdict"]').text()).toContain('disk 90%')
    expect(rows[0].find('[data-testid="fleet-verdict"]').text()).toContain('worst of 3 mounts · average 61%')
    expect(rows[1].find('[data-testid="fleet-row-note"]').text()).toContain('Waiting for its first sync')
    expect(rows[2].find('[data-testid="fleet-docker"]').text()).toContain('2 / 2')
    expect(rows[2].text()).toContain('Ubuntu 22.04 · x86_64')
    wrapper.unmount()
  })

  it('counts the fleet in the summary line and filters the table from it', async () => {
    const { wrapper } = await mountView()
    const summary = wrapper.find('[data-testid="infra-fleet-summary"]')
    expect(summary.find('[data-level="all"]').text()).toContain('3')
    expect(summary.find('[data-level="error"]').attributes('data-count')).toBe('1')
    expect(summary.find('[data-level="pending"]').attributes('data-count')).toBe('1')
    expect(summary.find('[data-level="warning"]').exists()).toBe(false) // zero: not offered
    await summary.find('[data-level="pending"]').trigger('click')
    expect(wrapper.findAll('[data-testid="fleet-row"]').map((r) => r.attributes('data-host'))).toEqual(['dsasa'])
    expect(new URLSearchParams(window.location.search).get('status')).toBe('pending')
    wrapper.unmount()
  })

  it('drills into a host on row click', async () => {
    const { wrapper, router } = await mountView()
    const push = vi.spyOn(router, 'push')
    await wrapper.get('[data-host="web-1"]').trigger('click')
    expect(push).toHaveBeenCalledWith('/infra/web-1')
    wrapper.unmount()
  })
})

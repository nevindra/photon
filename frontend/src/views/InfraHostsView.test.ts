// Integration test for the Infrastructure host list (`/infra`). A memory router + a fresh
// QueryClient + a <TooltipProvider> (AppShell/NavRail render Reka Tooltips), mirroring
// RumAppsView.test.ts's mount harness. The api layer is mocked. Proves the view mounts, renders one
// row per reporting host, and drills into a host on row click.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { customRange, clearScope, setTimeRange } from '@/lib/core/context'
import InfraHostsView from './InfraHostsView.vue'

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    infraHosts: vi.fn().mockResolvedValue({
      hosts: [
        { host: 'web-1', cpuUtil: 0.3, memUtil: 0.5, lastSeenNs: '1700000000000000000', hasGpu: false },
        { host: 'gpu-node-1', cpuUtil: 0.6, memUtil: 0.7, lastSeenNs: '1700000000000000000', hasGpu: true },
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
  router.push('/infra')
  await router.isReady()
  const wrapper = mount(
    {
      components: { TooltipProvider, InfraHostsView },
      template: '<TooltipProvider><InfraHostsView /></TooltipProvider>',
    },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  return { wrapper, router }
}

describe('InfraHostsView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    clearScope()
    setTimeRange('30m')
  })

  it('mounts and renders one card per reporting host', async () => {
    const { wrapper } = await mountView()
    await flushPromises()
    expect(wrapper.find('[data-testid="infra-hosts"]').exists()).toBe(true)
    const cards = wrapper.findAll('[data-testid="infra-host-card"]')
    expect(cards).toHaveLength(2)
    expect(cards[0].attributes('data-host')).toBe('web-1')
    expect(cards[1].attributes('data-host')).toBe('gpu-node-1')
    wrapper.unmount()
  })

  it('drills into a host on row click', async () => {
    const { wrapper, router } = await mountView()
    await flushPromises()
    const push = vi.spyOn(router, 'push')
    await wrapper.get('[data-host="web-1"]').trigger('click')
    expect(push).toHaveBeenCalledWith('/infra/web-1')
    wrapper.unmount()
  })
})

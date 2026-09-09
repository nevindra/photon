// The Agent tab: the enroll prompt for an unmanaged host, and the status / capabilities / Docker
// facts panel for a managed one, including the upgrade call-to-action.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import HostAgentPanel from './HostAgentPanel.vue'

const agent = vi.fn()
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    tenants: vi.fn().mockResolvedValue({ tenants: [] }),
    agent: (...a: unknown[]) => agent(...a),
  },
}))

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } })
  return [VueQueryPlugin, { queryClient }]
}

async function mountPanel(host = 'web-1') {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }] })
  router.push('/')
  await router.isReady()
  const wrapper = mount(
    { components: { TooltipProvider, HostAgentPanel }, template: `<TooltipProvider><HostAgentPanel host="${host}" /></TooltipProvider>` },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  await flushPromises()
  return wrapper
}

describe('HostAgentPanel', () => {
  beforeEach(() => {
    agent.mockReset()
    document.body.innerHTML = ''
  })

  it('offers enrolment when the host has no agent', async () => {
    agent.mockResolvedValue(null)
    const wrapper = await mountPanel('lonely')
    expect(wrapper.find('[data-testid="agent-not-enrolled"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="enroll-host"]').text()).toContain('Enroll lonely')
    wrapper.unmount()
  })

  it('shows status, reported capabilities, Docker disk and an upgrade CTA for a managed host', async () => {
    agent.mockResolvedValue({
      id: 'mh', host: 'web-1', site: null, status: 'online', agent_version: '1.7.2', latest_agent_version: '1.8.0',
      capabilities: ['read', 'lifecycle'], poll_secs: 15, last_seen_ms: Date.now() - 5000, enrolled_ms: Date.now() - 86_400_000,
      docker: { version: '27.3.1', api_version: '1.47', storage_driver: 'overlay2', os: 'Ubuntu', arch: 'x86_64', containers_running: 3, containers_total: 4, images: 9,
        disk: { images_bytes: 1e9, volumes_bytes: 2e9, build_cache_bytes: 0, reclaimable_bytes: 5e8, total_bytes: 1e11, used_bytes: 5e10 } },
      labels: { role: 'web' },
    })
    const wrapper = await mountPanel()
    expect(wrapper.find('[data-testid="agent-status"]').text()).toBe('Online')
    expect(wrapper.find('[data-testid="agent-upgrade-badge"]').text()).toContain('1.8.0')
    expect(wrapper.find('[data-testid="agent-upgrade"]').exists()).toBe(true)
    const caps = wrapper.findAll('[data-testid="agent-capabilities"] li')
    expect(caps.find((c) => c.attributes('data-cap') === 'lifecycle')?.attributes('data-granted')).toBe('true')
    expect(caps.find((c) => c.attributes('data-cap') === 'exec')?.attributes('data-granted')).toBe('false')
    expect(wrapper.find('[data-testid="docker-reclaimable"]').text()).toContain('reclaimable')
    expect(wrapper.find('[data-testid="agent-docker"]').text()).toContain('3')
    wrapper.unmount()
  })
})

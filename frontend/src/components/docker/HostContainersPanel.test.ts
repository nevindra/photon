// The Containers tab body: compose-project grouping with the managed chip, the view switch,
// the text filter, and the row → drawer hand-off (URL `?c=`). The stale banner for an offline
// host. The drawer's log/stat children hit the api too, so they are stubbed here.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import HostContainersPanel from './HostContainersPanel.vue'

const hostInventory = vi.fn()
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    hostInventory: (...a: unknown[]) => hostInventory(...a),
    dockerEvents: vi.fn().mockResolvedValue({
      events: [
        { id: 'h1', ts_ms: Date.now() - 3_000, type: 'container', action: 'exec_die', actor: 'demo-db-1', attributes: {} },
        { id: 'h2', ts_ms: Date.now() - 3_000, type: 'container', action: 'exec_start: /bin/sh -c pg_isready', actor: 'demo-db-1', attributes: {} },
        { id: 'h3', ts_ms: Date.now() - 13_000, type: 'container', action: 'exec_start: /bin/sh -c pg_isready', actor: 'demo-db-1', attributes: {} },
        { id: 'e1', ts_ms: Date.now() - 60_000, type: 'container', action: 'oom', actor: 'worker', attributes: {} },
      ],
    }),
    containerLogs: vi.fn().mockResolvedValue({ lines: [], truncated: false }),
    containerStats: vi.fn().mockResolvedValue({ cpu_pct: [], mem_bytes: [], net_rx_bps: [], net_tx_bps: [], block_read_bps: [], block_write_bps: [], mem_limit_bytes: null }),
  },
}))

function container(over: Record<string, unknown>) {
  return {
    id: 'id-' + over.name, name: over.name, image: 'img:1', image_id: 'sha256:x', command: '', state: 'running', health: 'none', status_text: 'Up 1 hour', created_ms: 0, started_ms: 0,
    finished_ms: null, exit_code: null, restart_count: 0, restart_policy: 'no', ports: [], mounts: [], networks: [], labels: {}, env_keys: [], env: {},
    compose_project: null, compose_service: null, managed_stack_id: null, cpu_pct: 1, mem_bytes: 1e6, mem_limit_bytes: null, net_rx_bps: null, net_tx_bps: null, block_read_bps: null, block_write_bps: null,
    ...over,
  }
}
const INV = {
  host: 'web-1', agent_status: 'online', fetched_ms: Date.now(), stale: false,
  containers: [
    container({ name: 'demo-api-1', compose_project: 'demo', compose_service: 'api', managed_stack_id: 'st1', image: 'ghcr.io/x/api:1', ports: [{ host_ip: '0.0.0.0', host_port: 4030, container_port: 4030, protocol: 'tcp' }, { host_ip: null, host_port: null, container_port: 9000, protocol: 'tcp' }] }),
    container({ name: 'demo-db-1', compose_project: 'demo', compose_service: 'db', image: 'postgres:16' }),
    container({ name: 'lonely', state: 'exited', exit_code: 1 }),
  ],
  images: [{ id: 'sha256:a', repo_tags: ['postgres:16'], size_bytes: 1e8, created_ms: 0, used_by: 1, dangling: false }],
  volumes: [], networks: [], facts: null,
}

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } })
  return [VueQueryPlugin, { queryClient }]
}

async function mountPanel() {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }] })
  router.push('/')
  await router.isReady()
  const wrapper = mount(
    { components: { TooltipProvider, HostContainersPanel }, template: '<TooltipProvider><HostContainersPanel host="web-1" /></TooltipProvider>' },
    { global: { plugins: [router, queryPlugin()], stubs: { ContainerStatsView: true, ContainerLogView: true } }, attachTo: document.body },
  )
  await flushPromises()
  return wrapper
}

describe('HostContainersPanel', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    hostInventory.mockReset()
    document.body.innerHTML = ''
  })

  it('groups containers by compose project with the managed chip, standalone last', async () => {
    hostInventory.mockResolvedValue(INV)
    const wrapper = await mountPanel()
    const groups = wrapper.findAll('[data-testid="container-group"]')
    expect(groups.map((g) => g.attributes('data-project'))).toEqual(['demo', ''])
    expect(groups[0].find('[data-testid="managed-chip"]').exists()).toBe(true)
    expect(groups[0].findAll('[data-testid="container-row"]').map((r) => r.attributes('data-container'))).toEqual(['demo-api-1', 'demo-db-1'])
    expect(wrapper.find('[data-testid="container-kpis"]').text()).toContain('2 / 3')
    expect(wrapper.find('[data-testid="docker-events"]').text()).toContain('OOM-killed')
    wrapper.unmount()
  })

  it('links a published port to the service on the host and leaves an unpublished one plain', async () => {
    hostInventory.mockResolvedValue(INV)
    const wrapper = await mountPanel()
    const links = wrapper.findAll('[data-testid="port-link"]')
    expect(links).toHaveLength(1)
    expect(links[0].attributes('href')).toBe('http://web-1:4030')
    expect(links[0].attributes('target')).toBe('_blank')
    expect(wrapper.find('[data-container="demo-api-1"]').text()).toContain('9000/tcp')
    // Following the link must not also open the drawer.
    await links[0].trigger('click')
    expect(new URLSearchParams(window.location.search).get('c')).toBeNull()
    wrapper.unmount()
  })

  it('folds a health check\'s exec chatter into one row that opens on click', async () => {
    hostInventory.mockResolvedValue(INV)
    const wrapper = await mountPanel()
    const fold = wrapper.find('[data-testid="docker-event-fold"]')
    expect(fold.exists()).toBe(true)
    expect(fold.attributes('data-count')).toBe('3')
    expect(fold.text()).toContain('every 10s')
    expect(wrapper.findAll('[data-testid="docker-event"]')).toHaveLength(1) // just the OOM
    await fold.find('button').trigger('click')
    expect(wrapper.findAll('[data-testid="docker-event"]')).toHaveLength(4)
    wrapper.unmount()
  })

  it('filters rows and switches to the images view', async () => {
    hostInventory.mockResolvedValue(INV)
    const wrapper = await mountPanel()
    await wrapper.find('[data-testid="container-filter"]').setValue('postgres')
    await flushPromises()
    expect(wrapper.findAll('[data-testid="container-row"]')).toHaveLength(1)
    expect(new URLSearchParams(window.location.search).get('q')).toBe('postgres')
    const images = wrapper.findAll('[data-testid="inventory-view"] button').find((b) => b.text().startsWith('images'))!
    await images.trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="images-table"]').exists()).toBe(true)
    expect(wrapper.findAll('[data-testid="image-row"]')).toHaveLength(1)
    wrapper.unmount()
  })

  it('opens the drawer from a row and writes ?c=', async () => {
    hostInventory.mockResolvedValue(INV)
    const wrapper = await mountPanel()
    await wrapper.find('[data-container="lonely"]').trigger('click')
    await flushPromises()
    expect(new URLSearchParams(window.location.search).get('c')).toBe('lonely')
    const drawer = document.body.querySelector('[data-testid="container-drawer"]')
    expect(drawer).not.toBeNull()
    expect(drawer!.textContent).toContain('lonely')
    expect(drawer!.textContent).toContain('Exited')
    wrapper.unmount()
  })

  it('labels an offline host’s inventory as stale rather than hiding it', async () => {
    hostInventory.mockResolvedValue({ ...INV, agent_status: 'offline', stale: true, fetched_ms: Date.now() - 3 * 86_400_000 })
    const wrapper = await mountPanel()
    const banner = wrapper.find('[data-testid="inventory-stale"]')
    expect(banner.exists()).toBe(true)
    expect(banner.text()).toContain('3d ago')
    expect(wrapper.findAll('[data-testid="container-row"]')).toHaveLength(3)
    wrapper.unmount()
  })

  it('points an unmanaged host at the Agent tab', async () => {
    hostInventory.mockResolvedValue(null)
    const wrapper = await mountPanel()
    expect(wrapper.find('[data-testid="containers-not-managed"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="containers-not-managed"] a').attributes('href')).toBe('/infra/web-1/agent')
    wrapper.unmount()
  })
})

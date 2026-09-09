// `/deploy/stacks`: the count band, the attention-first ordering, the needs-attention filter
// (URL `?attention=1`), and the drill-in to the detail route.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import StacksView from './StacksView.vue'

function stack(over: Record<string, unknown>) {
  return {
    id: 'st_' + over.name + '_' + over.host, name: over.name, host: over.host, site: null, source: 'compose', template_id: null, template_name: null, template_version: null, status: 'running',
    desired_generation: 1, applied_generation: 1, files: [], variables: {}, secrets: [], deploy_options: { pull: 'missing', force_recreate: false, recreate_volumes: false },
    services: [{ name: 'a', image: 'x', containers: [{ id: '1', name: 'a-1', state: 'running', health: 'none' }] }], drift: [], apm_services: [], acknowledged_warnings: [], last_error: null,
    created_by: 'r', created_ms: Date.now() - 1000, updated_ms: Date.now() - 1000, last_applied_ms: null,
    ...over,
  }
}

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    tenants: vi.fn().mockResolvedValue({ tenants: [] }),
    agents: vi.fn().mockResolvedValue({ hosts: [] }),
    templates: vi.fn().mockResolvedValue({ templates: [] }),
    stacks: vi.fn().mockResolvedValue({
      stacks: [
        { id: 'st_ok', name: 'alpha', host: 'web-1', site: null, source: 'compose', template_id: null, template_name: null, template_version: null, status: 'running', desired_generation: 1, applied_generation: 1, files: [], variables: {}, secrets: [], deploy_options: { pull: 'missing', force_recreate: false, recreate_volumes: false }, services: [], drift: [], apm_services: [], acknowledged_warnings: [], last_error: null, created_by: 'r', created_ms: 0, updated_ms: 0, last_applied_ms: null },
        { id: 'st_bad', name: 'beta', host: 'edge-1', site: null, source: 'compose', template_id: null, template_name: null, template_version: null, status: 'failed', desired_generation: 2, applied_generation: 1, files: [], variables: {}, secrets: [], deploy_options: { pull: 'missing', force_recreate: false, recreate_volumes: false }, services: [], drift: [], apm_services: [], acknowledged_warnings: [], last_error: 'port taken', created_by: 'r', created_ms: 0, updated_ms: 0, last_applied_ms: null },
      ],
    }),
  },
}))

const routes = [
  { path: '/deploy/stacks', component: { template: '<div />' } },
  { path: '/deploy/stacks/:id', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]
function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  return [VueQueryPlugin, { queryClient: new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } }) }]
}
async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/deploy/stacks')
  await router.isReady()
  const wrapper = mount({ components: { TooltipProvider, StacksView }, template: '<TooltipProvider><StacksView /></TooltipProvider>' }, { global: { plugins: [router, queryPlugin()] }, attachTo: document.body })
  await flushPromises()
  return { wrapper, router }
}

describe('StacksView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/deploy/stacks')
    document.body.innerHTML = ''
    void stack
  })

  it('counts, orders attention first, and drills into a row', async () => {
    const { wrapper, router } = await mountView()
    expect(wrapper.find('[data-testid="stack-kpis"]').text()).toContain('1 failed')
    const rows = wrapper.findAll('[data-testid="stack-row"]')
    expect(rows.map((r) => r.attributes('data-stack'))).toEqual(['st_bad', 'st_ok'])
    expect(rows[0].text()).toContain('port taken')
    expect(rows[0].text()).toContain('1 → 2')
    // Grouped under hosts, the host with the failed stack first.
    expect(wrapper.findAll('[data-testid="host-group"]').map((g) => g.attributes('data-host'))).toEqual(['edge-1', 'web-1'])
    await rows[1].trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/deploy/stacks/st_ok')
    wrapper.unmount()
  })

  it('narrows to what needs attention and writes ?attention=1', async () => {
    const { wrapper } = await mountView()
    const btn = wrapper.find('[data-testid="stack-attention"] [data-chip="attention"]')
    expect(btn.text()).toContain('Needs attention')
    await btn.trigger('click')
    await flushPromises()
    expect(wrapper.findAll('[data-testid="stack-row"]')).toHaveLength(1)
    expect(new URLSearchParams(window.location.search).get('attention')).toBe('1')
    wrapper.unmount()
  })
})

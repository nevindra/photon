// `/deploy/rollouts`: decision-first ordering, the count band, the filter (URL `?show=`), drill-in.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import RolloutsView from './RolloutsView.vue'

const target = (host: string, status: string, stage = 1) => ({ host, site: null, stage, status, attempts: 1, last_error: null, stack_id: null, previous_version: '1', agent_status: 'online', updated_ms: Date.now() })
const rollout = (id: string, state: string, targets: unknown[]) => ({
  id, name: id, template_id: 't', template_name: 'T', template_version_id: 'tv', template_version: '2', previous_version: '1', stack_name: 't',
  selector: { kind: 'hosts', hosts: ['a'] }, strategy: { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 300 },
  state, stage: 1, stages: 2, targets, variables: {}, created_by: 'r', created_ms: 0, updated_ms: Date.now(), started_ms: null, finished_ms: null,
})
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    agents: vi.fn().mockResolvedValue({ hosts: [] }),
    templates: vi.fn().mockResolvedValue({ templates: [] }),
    tenants: vi.fn().mockResolvedValue({ tenants: [] }),
    rollouts: vi.fn().mockImplementation(async () => ({ rollouts: [
      { id: 'r_done', name: 'done', template_id: 't', template_name: 'T', template_version_id: 'tv', template_version: '2', previous_version: '1', stack_name: 't', selector: { kind: 'hosts', hosts: ['a'] }, strategy: { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 300 }, state: 'completed', stage: 2, stages: 2, targets: [{ host: 'a', site: null, stage: 1, status: 'healthy', attempts: 1, last_error: null, stack_id: null, previous_version: '1', agent_status: 'online', updated_ms: 0 }], variables: {}, created_by: 'r', created_ms: 0, updated_ms: 1, started_ms: null, finished_ms: null },
      { id: 'r_gate', name: 'gate', template_id: 't', template_name: 'T', template_version_id: 'tv', template_version: '2', previous_version: '1', stack_name: 't', selector: { kind: 'site', sites: ['s'] }, strategy: { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 300 }, state: 'paused_gate', stage: 1, stages: 2, targets: [{ host: 'a', site: 's', stage: 1, status: 'healthy', attempts: 1, last_error: null, stack_id: null, previous_version: '1', agent_status: 'online', updated_ms: 0 }, { host: 'b', site: 's', stage: 2, status: 'pending', attempts: 0, last_error: null, stack_id: null, previous_version: '1', agent_status: 'offline', updated_ms: 0 }], variables: {}, created_by: 'r', created_ms: 0, updated_ms: 2, started_ms: null, finished_ms: null },
    ] })),
  },
}))

const routes = [{ path: '/deploy/rollouts', component: { template: '<div />' } }, { path: '/deploy/rollouts/:id', component: { template: '<div />' } }, { path: '/login', component: { template: '<div />' } }]
function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  return [VueQueryPlugin, { queryClient: new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } }) }]
}
async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/deploy/rollouts')
  await router.isReady()
  const wrapper = mount({ components: { TooltipProvider, RolloutsView }, template: '<TooltipProvider><RolloutsView /></TooltipProvider>' }, { global: { plugins: [router, queryPlugin()] }, attachTo: document.body })
  await flushPromises()
  return { wrapper, router }
}

describe('RolloutsView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/deploy/rollouts')
    document.body.innerHTML = ''
    void target
    void rollout
  })

  it('puts the rollout that needs a decision first and counts it', async () => {
    const { wrapper, router } = await mountView()
    expect(wrapper.find('[data-testid="rollout-kpis"]').text()).toContain('Needs a decision')
    const rows = wrapper.findAll('[data-testid="rollout-row"]')
    expect(rows.map((r) => r.attributes('data-rollout'))).toEqual(['r_gate', 'r_done'])
    expect(rows[0].text()).toContain('At the gate')
    expect(rows[0].find('[data-testid="rollout-progress"]').text()).toContain('1 / 2 healthy')
    await rows[0].trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/deploy/rollouts/r_gate')
    wrapper.unmount()
  })

  it('filters to done and writes ?show=', async () => {
    const { wrapper } = await mountView()
    const done = wrapper.findAll('[data-testid="rollout-filter"] button').find((b) => b.text() === 'Done')!
    await done.trigger('click')
    await flushPromises()
    expect(wrapper.findAll('[data-testid="rollout-row"]').map((r) => r.attributes('data-rollout'))).toEqual(['r_done'])
    expect(new URLSearchParams(window.location.search).get('show')).toBe('done')
    wrapper.unmount()
  })
})

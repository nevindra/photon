// `/deploy/templates`: the card grid, category chips (URL `?cat=`), the Ours / Imported switch, and
// the two doors out of a card — open the detail, or start the deploy wizard.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import TemplatesView from './TemplatesView.vue'

const tpl = (id: string, over: Record<string, unknown> = {}) => ({
  id, name: id, description: 'd', icon: '', categories: ['edge'], source: 'local', registry_id: null, latest_version: '1.0.0',
  versions: [{ id: 'tv_' + id, version: '1.0.0', created_ms: 0, created_by: 'x', notes: null, deployed_count: 0 }], updated_ms: Date.now(), stacks: 0, ...over,
})
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    agents: vi.fn().mockResolvedValue({ hosts: [] }),
    templates: vi.fn().mockResolvedValue({ templates: [
      { id: 'vas', name: 'vas', description: 'd', icon: '', categories: ['edge', 'nodeflux'], source: 'local', registry_id: null, latest_version: '2.0.0', versions: [{ id: 'tv_vas', version: '2.0.0', created_ms: 0, created_by: 'x', notes: null, deployed_count: 2 }], updated_ms: Date.now(), stacks: 2 },
      { id: 'kuma', name: 'kuma', description: 'd', icon: '', categories: ['monitoring'], source: 'portainer', registry_id: 'r', latest_version: '1.0.0', versions: [{ id: 'tv_kuma', version: '1.0.0', created_ms: 0, created_by: 'x', notes: null, deployed_count: 0 }], updated_ms: Date.now(), stacks: 0 },
    ] }),
    template: vi.fn().mockResolvedValue(null),
    templateVersion: vi.fn().mockResolvedValue(null),
    registries: vi.fn().mockResolvedValue({ registries: [] }),
    imageRegistries: vi.fn().mockResolvedValue({ credentials: [] }),
  },
}))

const routes = [
  { path: '/deploy/templates', component: { template: '<div />' } },
  { path: '/deploy/templates/:id', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]
function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  return [VueQueryPlugin, { queryClient: new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } }) }]
}
async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/deploy/templates')
  await router.isReady()
  const wrapper = mount({ components: { TooltipProvider, TemplatesView }, template: '<TooltipProvider><TemplatesView /></TooltipProvider>' }, { global: { plugins: [router, queryPlugin()] }, attachTo: document.body })
  await flushPromises()
  return { wrapper, router }
}

describe('TemplatesView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/deploy/templates')
    document.body.innerHTML = ''
    void tpl
  })

  it('renders cards and narrows by category and source', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.findAll('[data-testid="template-card"]')).toHaveLength(2)
    await wrapper.find('[data-category="monitoring"]').trigger('click')
    await flushPromises()
    expect(wrapper.findAll('[data-testid="template-card"]').map((c) => c.attributes('data-template'))).toEqual(['kuma'])
    expect(new URLSearchParams(window.location.search).get('cat')).toBe('monitoring')
    await wrapper.find('[data-category="monitoring"]').trigger('click')
    const ours = wrapper.findAll('[data-testid="template-source"] button').find((b) => b.text() === 'Ours')!
    await ours.trigger('click')
    await flushPromises()
    expect(wrapper.findAll('[data-testid="template-card"]').map((c) => c.attributes('data-template'))).toEqual(['vas'])
    wrapper.unmount()
  })

  it('opens the detail from a card and the wizard from Deploy', async () => {
    const { wrapper, router } = await mountView()
    await wrapper.find('[data-deploy="vas"]').trigger('click')
    await flushPromises()
    expect(document.body.querySelector('[data-testid="deploy-dialog"]')).not.toBeNull()
    expect(router.currentRoute.value.path).toBe('/deploy/templates')
    await wrapper.find('[data-template="kuma"]').trigger('click')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/deploy/templates/kuma')
    wrapper.unmount()
  })
})

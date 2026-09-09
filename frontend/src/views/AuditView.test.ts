// `/audit`: renders the log newest-first, expands a row into its detail line, and narrows by
// result through the Segmented (URL-synced `?result=`). The api layer is mocked; the shell's
// Reka tooltips need a TooltipProvider, as in the other view tests.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import AuditView from './AuditView.vue'

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    agents: vi.fn().mockResolvedValue({ hosts: [] }),
    audit: vi.fn().mockImplementation(async (filter: { result?: string }) => {
      const entries = [
        { id: 'a1', ts_ms: Date.now() - 60_000, actor: 'rina', role: 'operator', action: 'container.restart', target: { host: 'web-1', container: 'api' }, result: 'ok', detail: 'exit 0', payload_digest: null },
        { id: 'a2', ts_ms: Date.now() - 120_000, actor: 'bayu', role: 'viewer', action: 'stack.apply', target: { host: 'web-2', stack: 'demo' }, result: 'denied', detail: 'requires the Operator role', payload_digest: 'sha256:abcd' },
      ].filter((e) => !filter.result || e.result === filter.result)
      return { entries, total: entries.length }
    }),
  },
}))

const routes = [
  { path: '/audit', component: { template: '<div />' } },
  { path: '/infra/:host', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } })
  return [VueQueryPlugin, { queryClient }]
}

async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/audit')
  await router.isReady()
  const wrapper = mount(
    { components: { TooltipProvider, AuditView }, template: '<TooltipProvider><AuditView /></TooltipProvider>' },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  await flushPromises()
  return { wrapper, router }
}

describe('AuditView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/audit')
  })

  it('renders one row per entry with a readable action and a result pill', async () => {
    const { wrapper } = await mountView()
    const rows = wrapper.findAll('[data-testid="audit-row"]')
    expect(rows).toHaveLength(2)
    expect(rows[0].text()).toContain('Restart container')
    expect(rows[0].text()).toContain('api')
    expect(rows[0].attributes('data-result')).toBe('ok')
    expect(rows[1].text()).toContain('Denied')
    expect(wrapper.find('[data-testid="audit-summary"]').text()).toContain('2 entries')
    expect(wrapper.find('[data-testid="audit-summary"]').text()).toContain('1 denied')
    wrapper.unmount()
  })

  it('expands a row into its detail with the digest and a link to the host', async () => {
    const { wrapper } = await mountView()
    expect(wrapper.find('[data-testid="audit-detail"]').exists()).toBe(false)
    await wrapper.findAll('[data-testid="audit-row"]')[1].trigger('click')
    const detail = wrapper.find('[data-testid="audit-detail"]')
    expect(detail.exists()).toBe(true)
    expect(detail.text()).toContain('requires the Operator role')
    expect(detail.text()).toContain('sha256:abcd')
    expect(detail.find('a').attributes('href')).toBe('/infra/web-2')
    wrapper.unmount()
  })

  it('narrows by result and writes it to the URL', async () => {
    const { wrapper } = await mountView()
    const denied = wrapper.findAll('[data-testid="audit-result"] button').find((b) => b.text() === 'Denied')!
    await denied.trigger('click')
    await flushPromises()
    expect(wrapper.findAll('[data-testid="audit-row"]')).toHaveLength(1)
    expect(new URLSearchParams(window.location.search).get('result')).toBe('denied')
    wrapper.unmount()
  })
})

// Smoke test for RumErrorDetailView (`/rum/:appId/errors/:fingerprint`). Mirrors
// RumErrorsView.test.ts / RumPageDetailView.test.ts's TooltipProvider host-mount pattern (AppShell
// → NavRail renders Reka UI tooltips that need a TooltipProvider ancestor in the render tree, not
// just a locally-registered component). Proves the view queries the error detail with the route's
// app + fingerprint and the global context window, renders the hero exception type, and that the
// per-event "Open trace" link points at `/traces/<trace_id>`.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { startNs, endNs, customRange, clearScope, setTimeRange } from '@/lib/core/context'
import RumErrorDetailView from './RumErrorDetailView.vue'

// Timestamps in RumErrorDetailResult (first_seen/last_seen/series[].t/events[].timestamp) are
// Date.now()-based epoch-MILLISECOND numbers (mirrors mockRumErrorDetail in lib/core/mock.ts) —
// NOT the decimal-nanosecond bigint-string convention used for startNs/endNs.
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    rumErrorDetail: vi.fn().mockResolvedValue({
      app: 'web-storefront',
      fingerprint: 'fp1',
      exception_type: 'TypeError',
      message: 'x is undefined',
      error_kind: 'exception',
      first_seen: 1_700_000_000_000,
      last_seen: 1_700_000_100_000,
      occurrences: 5,
      sessions: 3,
      series: [{ t: 1_700_000_000_000, count: 5 }],
      tags: [{ field: 'browser.name', values: [{ value: 'Chrome', count: 5 }] }],
      sample_stack: 'TypeError: x is undefined\n  at f (a.js:1)',
      events: [
        {
          timestamp: 1_700_000_100_000,
          route: '/checkout',
          browser: 'Chrome',
          device: 'mobile',
          session: 's1',
          trace_id: 'a'.repeat(32),
        },
      ],
    }),
  },
}))

const routes = [
  { path: '/rum', component: { template: '<div />' } },
  { path: '/rum/:appId', component: { template: '<div />' } },
  { path: '/rum/:appId/errors', component: { template: '<div />' } },
  { path: '/rum/:appId/errors/:fingerprint', component: { template: '<div />' } },
  { path: '/traces/:traceId', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } },
  })
  return [VueQueryPlugin, { queryClient }]
}

async function mountDetail() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/rum/web-storefront/errors/fp1')
  await router.isReady()
  const wrapper = mount(
    {
      components: { TooltipProvider, RumErrorDetailView },
      template: '<TooltipProvider><RumErrorDetailView /></TooltipProvider>',
    },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  return { wrapper, router }
}

describe('RumErrorDetailView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    customRange.value = null
    clearScope()
    setTimeRange('30m')
  })

  it('loads detail for the route app + fingerprint and renders the hero + trace jump', async () => {
    const { api } = await import('@/lib/core/api')
    vi.mocked(api.rumErrorDetail).mockClear()
    const { wrapper } = await mountDetail()
    await flushPromises()

    // useRumErrorDetail(app, fingerprint, startNs, endNs) →
    // api.rumErrorDetail(app, fingerprint, startNs, endNs, { signal }).
    const call = vi.mocked(api.rumErrorDetail).mock.calls[0]
    expect(call[0]).toBe('web-storefront')
    expect(call[1]).toBe('fp1')
    expect(call[2]).toBe(startNs.value)
    expect(call[3]).toBe(endNs.value)

    expect(wrapper.text()).toContain('TypeError')

    // per-event "Open trace" link points at /traces/<trace_id>
    const link = wrapper.find(`a[href="/traces/${'a'.repeat(32)}"]`)
    expect(link.exists()).toBe(true)

    wrapper.unmount()
  })
})

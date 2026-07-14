import { describe, it, expect, vi, beforeAll, afterAll } from 'vitest'
import { nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import TraceDetailView from './TraceDetailView.vue'
import SpanDetailPanel from '@/components/traces/SpanDetailPanel.vue'
import TraceWaterfall from '@/components/traces/TraceWaterfall.vue'
import { correlate } from '@/lib/core/useCorrelate'

const { copySpy } = vi.hoisted(() => ({ copySpy: vi.fn() }))
vi.mock('@/lib/core/useCopy', () => ({ useCopy: () => ({ copy: copySpy }) }))

// TraceWaterfall virtualizes its span rows; virtual-core reads the scroll element's offsetHeight
// and yields an empty range for a zero-height viewport (all jsdom reports). Stub a real height.
let restoreH, restoreW
beforeAll(() => {
  restoreH = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight')
  restoreW = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth')
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, get: () => 1200 })
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, get: () => 800 })
})
afterAll(() => {
  if (restoreH) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', restoreH)
  if (restoreW) Object.defineProperty(HTMLElement.prototype, 'offsetWidth', restoreW)
})

// Let the trace query resolve AND the waterfall virtualizer measure + render its slice.
async function settle() {
  await flushPromises()
  await nextTick()
  await flushPromises()
}

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: true,
    getTrace: vi.fn(async (id) => ({
      trace_id: id,
      spans: [
        {
          span_id: 'r',
          parent_span_id: null,
          start_time_nanos: 0n,
          end_time_nanos: 1_000_000n,
          duration_nanos: 1_000_000,
          name: 'root',
          service: 'api',
          status_code: 0,
        },
      ],
      elapsed_ms: 1,
    })),
  },
}))

import { api } from '@/lib/core/api'

const routes = [
  { path: '/logs', component: { template: '<div />' } },
  { path: '/traces', component: { template: '<div />' } },
  { path: '/traces/:traceId', component: TraceDetailView },
]

async function mountAt(path) {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(path)
  await router.isReady()
  // Fresh QueryClient per mount (no cross-test cache); retry off so a settled error surfaces at once.
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  // Stub AppShell so we don't need a router-driven NavRail/Tooltip here — but still render the
  // `crumb` prop (as data-testid="crumb" text) so the breadcrumb wiring is observable, plus the
  // header-chrome slots the view now folds into the ContextBar (lead = back button, actions =
  // trace-id chip + view-all-logs) so those controls stay observable in this stub.
  const wrapper = mount(TraceDetailView, {
    global: {
      plugins: [router, [VueQueryPlugin, { queryClient }]],
      stubs: {
        AppShell: {
          props: ['crumb'],
          template:
            '<div><div data-testid="crumb">{{ crumb }}</div><slot name="lead" /><slot name="actions" /><slot name="toolbar" /><slot /></div>',
        },
      },
    },
    attachTo: document.body,
  })
  return { wrapper, router }
}

describe('TraceDetailView', () => {
  it('loads the trace from the :traceId route param and renders the waterfall', async () => {
    api.getTrace.mockClear()
    const { wrapper } = await mountAt('/traces/abc123')
    await settle()
    // useTrace threads an AbortSignal as the trailing opts arg.
    expect(api.getTrace).toHaveBeenCalledWith('abc123', undefined, expect.objectContaining({ signal: expect.any(AbortSignal) }))
    expect(wrapper.findAll('[data-span-row]').length).toBe(1)
    wrapper.unmount()
  })

  // Time itself is global now (lib/context.js via the ContextBar mounted in AppShell) — this view
  // has no local time window of its own, only the breadcrumb to wire up: "Traces › <short id>".
  it('passes a "Traces › <short trace id>" crumb to AppShell', async () => {
    const { wrapper } = await mountAt('/traces/abcdef1234567890')
    await settle()
    expect(wrapper.find('[data-testid="crumb"]').text()).toBe('Traces › abcdef123456')
    wrapper.unmount()
  })

  it('passes the ?t= time hint through to api.getTrace', async () => {
    api.getTrace.mockClear()
    const { wrapper } = await mountAt('/traces/t9?t=123')
    await settle()
    expect(api.getTrace).toHaveBeenCalledWith('t9', '123', expect.objectContaining({ signal: expect.any(AbortSignal) }))
    wrapper.unmount()
  })

  it('reloads when the :traceId param changes', async () => {
    api.getTrace.mockClear()
    const { wrapper, router } = await mountAt('/traces/first')
    await settle()
    router.push('/traces/second')
    await settle()
    expect(api.getTrace).toHaveBeenCalledWith('second', undefined, expect.objectContaining({ signal: expect.any(AbortSignal) }))
    wrapper.unmount()
  })

  it('routes back to /traces on the back button', async () => {
    const { wrapper, router } = await mountAt('/traces/abc')
    await settle()
    const push = vi.spyOn(router, 'push')
    await wrapper.find('[data-testid="back-to-traces"]').trigger('click')
    expect(push).toHaveBeenCalledWith('/traces')
    wrapper.unmount()
  })

  it('pivots to /logs with the whole-trace query on "view all logs"', async () => {
    const { wrapper, router } = await mountAt('/traces/abc')
    await settle()
    const push = vi.spyOn(router, 'push')
    const btn = wrapper.findAll('button').find((b) => b.text().includes('View all logs'))
    await btn.trigger('click')
    // Now routed through correlate(), so the pivot also carries the active time window (range=…).
    expect(push).toHaveBeenCalledWith(correlate({ path: '/logs', query: { q: 'trace_id:abc' } }))
    wrapper.unmount()
  })

  it('pivots to /logs with the span query when SpanDetailPanel emits view-logs', async () => {
    const { wrapper, router } = await mountAt('/traces/abc')
    await settle()
    const push = vi.spyOn(router, 'push')
    wrapper
      .findComponent(SpanDetailPanel)
      .vm.$emit('view-logs', { query: 'trace_id:abc span_id:r' })
    // Now routed through correlate(), so the pivot also carries the active time window (range=…).
    expect(push).toHaveBeenCalledWith(correlate({ path: '/logs', query: { q: 'trace_id:abc span_id:r' } }))
    wrapper.unmount()
  })

  it('renders one "Time by service" breakdown segment per service', async () => {
    api.getTrace.mockClear()
    api.getTrace.mockImplementationOnce(async (id) => ({
      trace_id: id,
      spans: [
        {
          span_id: 'r',
          parent_span_id: null,
          start_time_nanos: 0n,
          end_time_nanos: 100_000_000n,
          duration_nanos: 100_000_000,
          name: 'root',
          service: 'web',
          status_code: 0,
        },
        {
          span_id: 'c1',
          parent_span_id: 'r',
          start_time_nanos: 0n,
          end_time_nanos: 40_000_000n,
          duration_nanos: 40_000_000,
          name: 'call-api',
          service: 'api',
          status_code: 0,
        },
      ],
      elapsed_ms: 1,
    }))
    const { wrapper } = await mountAt('/traces/multi')
    await settle()
    const segments = wrapper.findAll('[data-testid="breakdown-segment"]')
    expect(segments.length).toBe(2)
    wrapper.unmount()
  })

  it('passes the ?span= query as initialSpanId to the waterfall', async () => {
    const { wrapper } = await mountAt('/traces/abc123?span=r')
    await settle()
    expect(wrapper.findComponent(TraceWaterfall).props('initialSpanId')).toBe('r')
    wrapper.unmount()
  })

  it('leaves initialSpanId unset when ?span= is absent', async () => {
    const { wrapper } = await mountAt('/traces/abc123')
    await settle()
    // Binding `undefined` resolves to TraceWaterfall's own declared default (null).
    expect(wrapper.findComponent(TraceWaterfall).props('initialSpanId')).toBeNull()
    wrapper.unmount()
  })

  it('copies the trace ID via useCopy', async () => {
    copySpy.mockClear()
    const { wrapper } = await mountAt('/traces/abc123')
    await settle()
    await wrapper.find('[title="Copy trace ID"]').trigger('click')
    expect(copySpy).toHaveBeenCalledWith('abc123', 'trace ID')
    wrapper.unmount()
  })

  it('clears the selected span (and its node prop) when Escape is pressed', async () => {
    const { wrapper } = await mountAt('/traces/abc')
    await settle()
    await wrapper.find('[data-span-row]').trigger('click')
    await nextTick()
    const panel = wrapper.findComponent(SpanDetailPanel)
    expect(panel.props('span')).not.toBeNull()
    expect(panel.props('node')).toBeTruthy()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    await nextTick()
    expect(wrapper.findComponent(SpanDetailPanel).props('span')).toBeNull()
    wrapper.unmount()
  })
})

// Full-chrome parity test for the traces landing page (mirrors LogsView.test.js): SearchBar +
// TracesFilters + a Volume/Latency chart toggle + results toolbar (mode
// segmented toggle + errors-only switch + sort) + TraceTable/SpanTable + peek drawer, all wired
// to the spans `api.*` methods with a debounced search. No network — every api method the mounted
// tree can reach (services/searchTraces/searchSpans/getTrace/tracesFields/tracesFacet/
// tracesHistogram/tracesLatency) is stubbed.
import { describe, it, expect, vi, beforeEach, beforeAll, afterAll } from 'vitest'
import { nextTick, toValue } from 'vue'
import { mount, flushPromises, DOMWrapper } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import TracesExplorer from './TracesExplorer.vue'
import TracesFilters from '@/components/traces/TracesFilters.vue'
import SpanVolumeHistogram from '@/components/traces/SpanVolumeHistogram.vue'
import LatencyHistogram from '@/components/traces/LatencyHistogram.vue'
import TraceTable from '@/components/traces/TraceTable.vue'
import SpanTable from '@/components/traces/SpanTable.vue'
import TracePeekDrawer from '@/components/traces/TracePeekDrawer.vue'
import { timeRange, customRange, startNs, endNs } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'

// TraceTable virtualizes its rows; virtual-core reads the scroll element's offsetHeight and yields
// an empty range for a zero-height viewport (all jsdom reports). Stub a real height so rows mount.
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

// Let the query resolve AND the virtualizer measure + render its slice.
async function settle() {
  await flushPromises()
  await nextTick()
  await flushPromises()
}

// SelectMenu (Sort / Refresh mode) renders its options only while open, teleported to
// document.body — open the trigger, then query the body (same pattern as SelectMenu.test.js).
async function openSelect(wrapper, ariaLabel) {
  await wrapper.get(`[aria-label="${ariaLabel}"]`).trigger('click')
  await nextTick()
  await new Promise((r) => setTimeout(r, 0))
  return new DOMWrapper(document.body)
}

// api.searchTraces is mocked wholesale below (bypassing api.js's hydrateTraces), so this fixture
// must already be in "hydrated" UI shape — BigInt start_ts/duration_ns — matching TraceTable's
// own test fixtures.
function trace(id, overrides = {}) {
  return {
    trace_id: id,
    root_service: 'checkout',
    root_name: 'POST /checkout',
    start_ts: 1_700_000_000_000_000_000n,
    duration_ns: 5_000_000n,
    span_count: 4,
    error_count: 0,
    services: ['checkout', 'payments'],
    ...overrides,
  }
}

// api.searchSpans is likewise mocked wholesale, so span rows must be pre-hydrated (BigInt
// start_time_nanos/duration_nanos) — the shape SpanTable renders straight off.
function span(id, overrides = {}) {
  return {
    span_id: id,
    trace_id: 't1',
    service: 'checkout',
    name: 'POST /checkout',
    start_time_nanos: 1_700_000_000_000_000_000n,
    end_time_nanos: 1_700_000_005_000_000_000n,
    duration_nanos: 5_000_000n,
    status_code: 0,
    attributes: {},
    ...overrides,
  }
}

// Wrap useSearchTraces/useSearchSpans so the test can read the reactive query options the view
// passes them (chiefly `refetchInterval`, to assert the live-tail pause) while still delegating to
// the real composables — everything else in tracesQueries is the genuine article.
const captured = vi.hoisted(() => ({ trace: null, span: null }))
vi.mock('@/lib/traces/tracesQueries', async (importOriginal) => {
  const actual = await importOriginal()
  return {
    ...actual,
    useSearchTraces: (key, build, opts) => {
      captured.trace = opts
      return actual.useSearchTraces(key, build, opts)
    },
    useSearchSpans: (key, build, opts) => {
      captured.span = opts
      return actual.useSearchSpans(key, build, opts)
    },
  }
})

// useLiveTail (consumed for real) opens its stream via `openLiveStream` — mocked here (exactly
// like useLiveTail.test.js) so a test can push streamed rows straight into the buffer via the
// captured `onRows` callback, without a real EventSource.
const liveStreamState = vi.hoisted(() => ({ handle: null }))
vi.mock('@/lib/core/liveStream', () => ({
  openLiveStream: vi.fn((opts) => {
    liveStreamState.handle = { opts, close: vi.fn() }
    return liveStreamState.handle
  }),
}))

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    services: vi.fn().mockResolvedValue(['checkout', 'payments']),
    searchTraces: vi.fn().mockResolvedValue({
      traces: [trace('t1'), trace('t2', { error_count: 1 })],
      matched_count: 2,
      elapsed_ms: 7,
    }),
    searchSpans: vi.fn().mockResolvedValue({
      rows: [span('s1'), span('s2', { status_code: 2 })],
      matched_count: 2,
      elapsed_ms: 5,
    }),
    getTrace: vi.fn().mockResolvedValue({ trace_id: 't1', spans: [] }),
    tracesFields: vi.fn().mockResolvedValue([{ name: 'region', kind: 'attribute' }]),
    tracesFacet: vi.fn().mockResolvedValue({ values: [{ value: 'us', count: 3 }], capped: false }),
    tracesHistogram: vi.fn().mockResolvedValue([]),
    tracesLatency: vi.fn().mockResolvedValue({ buckets: [], p50: '0', p90: '0', p99: '0' }),
  },
}))

import { api } from '@/lib/core/api'
import { openLiveStream } from '@/lib/core/liveStream'

const routes = [
  { path: '/traces', component: { template: '<div />' } },
  { path: '/traces/:traceId', component: { template: '<div />' } },
  { path: '/logs', component: { template: '<div />' } },
]

async function makeRouter(initial = '/traces') {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(initial)
  await router.isReady()
  return router
}

async function mountExplorer() {
  const router = await makeRouter('/traces')
  // A fresh QueryClient per mount so no cache leaks between tests; retry off so a 400 surfaces at once.
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const wrapper = mount(
    {
      components: { TooltipProvider, TracesExplorer },
      template: '<TooltipProvider><TracesExplorer /></TooltipProvider>',
    },
    {
      global: { plugins: [router, [VueQueryPlugin, { queryClient }]] },
      attachTo: document.body,
    },
  )
  await settle()
  return { wrapper, router }
}

// A prior test's useUrlState/sort sync leaves `?range=...&sort=...` on window.location — reset
// it (and every mock's call history) so each test starts from a clean slate. Time is now owned by
// the global context.js singleton rather than view-local state — reset it too, so a preset/custom
// range set by one case never leaks into the next.
beforeEach(() => {
  window.history.replaceState(null, '', '/')
  timeRange.value = '30m'
  customRange.value = null
  captured.trace = null
  captured.span = null
  liveStreamState.handle = null
  vi.clearAllMocks()
  api.services.mockResolvedValue(['checkout', 'payments'])
  api.searchTraces.mockResolvedValue({
    traces: [trace('t1'), trace('t2', { error_count: 1 })],
    matched_count: 2,
    elapsed_ms: 7,
  })
  api.searchSpans.mockResolvedValue({
    rows: [span('s1'), span('s2', { status_code: 2 })],
    matched_count: 2,
    elapsed_ms: 5,
  })
  api.getTrace.mockResolvedValue({ trace_id: 't1', spans: [] })
  api.tracesFields.mockResolvedValue([{ name: 'region', kind: 'attribute' }])
  api.tracesFacet.mockResolvedValue({ values: [{ value: 'us', count: 3 }], capped: false })
  api.tracesHistogram.mockResolvedValue([])
  api.tracesLatency.mockResolvedValue({ buckets: [], p50: '0', p90: '0', p99: '0' })
})

describe('TracesExplorer (integration)', () => {
  it('renders the trace table from the stubbed api.searchTraces', async () => {
    const { wrapper } = await mountExplorer()
    expect(api.searchTraces).toHaveBeenCalled()
    expect(wrapper.findAll('[data-testid="trace-row"]').length).toBe(2)
    expect(wrapper.text()).toContain('POST /checkout')
    wrapper.unmount()
  })

  // The view used to own RANGE_MS/timeRange/customRange/nowTick locally; it now reads the app-wide
  // window from lib/context.js (mounted globally via ContextBar in AppShell), so picking a preset
  // there — with no TracesExplorer-local wiring at all — must reshape the search request.
  it('traces search uses the global context window', async () => {
    timeRange.value = '15m'
    const { wrapper } = await mountExplorer()

    expect(api.searchTraces).toHaveBeenCalled()
    const lastCall = api.searchTraces.mock.calls.at(-1)[0]
    expect(lastCall.start).toBe(startNs.value)
    expect(lastCall.end).toBe(endNs.value)
    wrapper.unmount()
  })

  it('changes the request sort when the sort toggle is clicked', async () => {
    const { wrapper } = await mountExplorer()
    api.searchTraces.mockClear()

    const body = await openSelect(wrapper, 'Sort results')
    await body.find('[data-testid="select-option-slowest"]').trigger('click')
    // The search is debounced (180ms) — wait it out.
    await new Promise((resolve) => setTimeout(resolve, 250))
    await flushPromises()

    expect(api.searchTraces).toHaveBeenCalled()
    const lastCall = api.searchTraces.mock.calls.at(-1)[0]
    expect(lastCall.sort).toBe('slowest')
    wrapper.unmount()
  })

  it('excludes a value when the facet rail toggles it from the default all-checked state, and re-searches', async () => {
    const { wrapper } = await mountExplorer()
    api.searchTraces.mockClear()

    // Single-state model: with no prior `region:` term (all-mode, every value checked), toggling a
    // value UNCHECKS it → writes a `-region:us` exclusion (not an include).
    wrapper.findComponent(TracesFilters).vm.$emit('toggle-value', { field: 'region', value: 'us' })
    await new Promise((resolve) => setTimeout(resolve, 250))
    await flushPromises()

    expect(api.searchTraces).toHaveBeenCalled()
    const lastCall = api.searchTraces.mock.calls.at(-1)[0]
    expect(lastCall.query).toBe('-region:us')
    wrapper.unmount()
  })

  it('excludes a value when the quick filters toggle it from the default all-checked state, and re-searches', async () => {
    const { wrapper } = await mountExplorer()
    api.searchTraces.mockClear()

    // Same single-state semantics from the quick-filters rail: toggling `service:checkout` with no
    // prior include writes the `-service:checkout` exclusion.
    wrapper.findComponent(TracesFilters).vm.$emit('toggle-value', { field: 'service', value: 'checkout' })
    await new Promise((resolve) => setTimeout(resolve, 250))
    await flushPromises()

    expect(api.searchTraces).toHaveBeenCalled()
    const lastCall = api.searchTraces.mock.calls.at(-1)[0]
    expect(lastCall.query).toBe('-service:checkout')
    wrapper.unmount()
  })

  it('swaps the volume chart for the latency chart via the chart-mode toggle', async () => {
    const { wrapper } = await mountExplorer()

    expect(wrapper.findComponent(SpanVolumeHistogram).exists()).toBe(true)
    expect(wrapper.findComponent(LatencyHistogram).exists()).toBe(false)

    await wrapper.find('[data-testid="chart-latency"]').trigger('click')
    await flushPromises()

    expect(wrapper.findComponent(SpanVolumeHistogram).exists()).toBe(false)
    expect(wrapper.findComponent(LatencyHistogram).exists()).toBe(true)
    wrapper.unmount()
  })

  it('rewrites the query with a duration range when the latency chart emits brush', async () => {
    const { wrapper } = await mountExplorer()

    // Switch to the latency chart so LatencyHistogram is mounted, then brush a duration band.
    await wrapper.find('[data-testid="chart-latency"]').trigger('click')
    await flushPromises()

    api.searchTraces.mockClear()
    wrapper.findComponent(LatencyHistogram).vm.$emit('brush', { minNs: 1_000_000, maxNs: 2_000_000 })
    // The search is debounced (180ms) — wait it out.
    await new Promise((resolve) => setTimeout(resolve, 250))
    await flushPromises()

    expect(api.searchTraces).toHaveBeenCalled()
    const lastCall = api.searchTraces.mock.calls.at(-1)[0]
    expect(lastCall.query).toBe('duration>=1ms duration<=2ms')
    wrapper.unmount()
  })

  it('toggles to spans mode: renders SpanTable and runs the spans query', async () => {
    const { wrapper } = await mountExplorer()
    expect(wrapper.findComponent(TraceTable).exists()).toBe(true)
    expect(wrapper.findComponent(SpanTable).exists()).toBe(false)

    await wrapper.find('[data-testid="mode-spans"]').trigger('click')
    await settle()

    expect(wrapper.findComponent(SpanTable).exists()).toBe(true)
    expect(wrapper.findComponent(TraceTable).exists()).toBe(false)
    expect(api.searchSpans).toHaveBeenCalled()
    // The meta label follows the mode.
    expect(wrapper.text()).toContain('spans')
    wrapper.unmount()
  })

  it('round-trips the result mode through the URL like sort', async () => {
    // Seed the URL in spans mode, before mount, exactly the way `sort` seeds.
    window.history.replaceState(null, '', '/traces?mode=spans')
    const { wrapper } = await mountExplorer()
    expect(wrapper.findComponent(SpanTable).exists()).toBe(true)

    // Switch back to traces → the mode is re-stamped onto the URL (not dropped).
    await wrapper.find('[data-testid="mode-traces"]').trigger('click')
    await flushPromises()
    expect(new URLSearchParams(window.location.search).get('mode')).toBe('traces')
    expect(wrapper.findComponent(TraceTable).exists()).toBe(true)
    wrapper.unmount()
  })

  it('errors-only switch adds and removes status:error in the query', async () => {
    const { wrapper } = await mountExplorer()

    api.searchTraces.mockClear()
    await wrapper.find('[data-testid="errors-only-toggle"]').trigger('click')
    await new Promise((resolve) => setTimeout(resolve, 250))
    await flushPromises()
    expect(api.searchTraces.mock.calls.at(-1)[0].query).toContain('status:error')

    api.searchTraces.mockClear()
    await wrapper.find('[data-testid="errors-only-toggle"]').trigger('click')
    await new Promise((resolve) => setTimeout(resolve, 250))
    await flushPromises()
    expect(api.searchTraces.mock.calls.at(-1)[0].query).not.toContain('status:error')
    wrapper.unmount()
  })

  it('sends trace attribute columns (from columns-changed) in the search request and refetches', async () => {
    const { wrapper } = await mountExplorer()
    api.searchTraces.mockClear()

    wrapper.findComponent(TraceTable).vm.$emit('columns-changed', ['http.route'])
    await new Promise((resolve) => setTimeout(resolve, 50))
    await flushPromises()

    expect(api.searchTraces).toHaveBeenCalled()
    expect(api.searchTraces.mock.calls.at(-1)[0].columns).toEqual(['http.route'])
    wrapper.unmount()
  })

  it('opens the peek drawer on a row action and pauses live tail while it is open', async () => {
    const { wrapper } = await mountExplorer()

    // Manual (the default mode) → no polling.
    expect(toValue(captured.trace.refetchInterval)).toBe(false)
    // Selecting 5s arms the interval.
    const body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-5s"]').trigger('click')
    await flushPromises()
    expect(toValue(captured.trace.refetchInterval)).toBe(5000)

    // A row click opens the drawer (NOT a route navigation).
    wrapper.findComponent(TraceTable).vm.$emit('open-trace', { traceId: 't1', timeHintNs: '999' })
    await flushPromises()
    expect(wrapper.findComponent(TracePeekDrawer).props('open')).toBe(true)
    // ...and live tail pauses while the drawer is open.
    expect(toValue(captured.trace.refetchInterval)).toBe(false)
    wrapper.unmount()
  })

  it('navigates to the full trace view only from the drawer, not on the row click', async () => {
    const { wrapper, router } = await mountExplorer()
    const push = vi.spyOn(router, 'push')

    // Row click opens the drawer — it must NOT navigate.
    await wrapper.find('[data-trace-id="t1"]').trigger('click')
    await flushPromises()
    expect(push).not.toHaveBeenCalled()
    expect(wrapper.findComponent(TracePeekDrawer).props('open')).toBe(true)

    // "Open full view" from the drawer navigates, carrying the time hint.
    wrapper.findComponent(TracePeekDrawer).vm.$emit('open-full', { traceId: 't1', spanId: null })
    expect(push).toHaveBeenCalledWith('/traces/t1?t=' + 1_700_000_000_000_000_000n.toString())
    wrapper.unmount()
  })

  it('selecting Live while in traces mode auto-switches to Spans and streams grain "spans"', async () => {
    const { wrapper } = await mountExplorer()
    expect(wrapper.findComponent(TraceTable).exists()).toBe(true)
    expect(wrapper.findComponent(SpanTable).exists()).toBe(false)

    const body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-live"]').trigger('click')
    await settle()

    // Live only streams flat spans — Traces has no meaningful streamed row, so picking Live
    // flips the result mode for the user rather than silently doing nothing.
    expect(wrapper.findComponent(SpanTable).exists()).toBe(true)
    expect(wrapper.findComponent(TraceTable).exists()).toBe(false)
    expect(api.searchSpans).toHaveBeenCalled()
    expect(openLiveStream).toHaveBeenCalledWith(expect.objectContaining({ grain: 'spans' }))
    // Live streaming replaces polling — the list query's own refetchInterval stays off.
    expect(toValue(captured.span.refetchInterval)).toBe(false)
    wrapper.unmount()
  })

  it('renders streamed rows through the SpanTable while Live is active', async () => {
    const { wrapper } = await mountExplorer()

    const body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-live"]').trigger('click')
    await settle()
    expect(liveStreamState.handle).not.toBeNull()

    // Push a streamed row straight through the captured onRows callback (bypassing api.searchSpans
    // entirely) — the table must reflect the live buffer, not the last search page.
    liveStreamState.handle.opts.onRows([span('live-1', { name: 'GET /streamed' })])
    await settle()

    expect(wrapper.findAll('[data-span-id="live-1"]').length).toBe(1)
    expect(wrapper.text()).toContain('GET /streamed')
    wrapper.unmount()
  })

  it('steps the drawer to the adjacent trace on next/prev and clamps at the ends', async () => {
    const { wrapper } = await mountExplorer()
    const drawer = () => wrapper.findComponent(TracePeekDrawer)

    // Open on the first trace (t1) — index 0 of the two loaded traces.
    wrapper.findComponent(TraceTable).vm.$emit('open-trace', { traceId: 't1', timeHintNs: '1' })
    await flushPromises()
    expect(drawer().props('traceId')).toBe('t1')
    expect(drawer().props('index')).toBe(0)
    expect(drawer().props('total')).toBe(2)

    // prev at the first row is a no-op (clamped).
    drawer().vm.$emit('prev')
    await flushPromises()
    expect(drawer().props('traceId')).toBe('t1')

    // next → t2 (index 1), time hint follows the row's start_ts.
    drawer().vm.$emit('next')
    await flushPromises()
    expect(drawer().props('traceId')).toBe('t2')
    expect(drawer().props('index')).toBe(1)
    expect(drawer().props('timeHintNs')).toBe(String(1_700_000_000_000_000_000n))

    // next at the last row is a no-op (clamped).
    drawer().vm.$emit('next')
    await flushPromises()
    expect(drawer().props('traceId')).toBe('t2')

    // prev → back to t1.
    drawer().vm.$emit('prev')
    await flushPromises()
    expect(drawer().props('traceId')).toBe('t1')
    wrapper.unmount()
  })

  it('pivots to the logs view filtered by trace_id when the drawer emits view-logs', async () => {
    const { wrapper, router } = await mountExplorer()
    const push = vi.spyOn(router, 'push')

    wrapper.findComponent(TracePeekDrawer).vm.$emit('view-logs', { traceId: 't1', timeHintNs: '1' })
    // Now routed through correlate(), so the pivot also carries the active time window (range=…).
    expect(push).toHaveBeenCalledWith(correlate({ path: '/logs', query: { q: 'trace_id:t1' } }))
    wrapper.unmount()
  })

  it('rounds the aggregate window to a 60s bucket (stable across sub-minute nowTick ticks)', async () => {
    // Pin the wall clock 20s into a minute bucket so a sub-minute tick stays inside it.
    const base = 1_700_000_000_000
    const nowSpy = vi.spyOn(Date, 'now').mockReturnValue(base)
    const { wrapper } = await mountExplorer()
    // The facet rail is fed the ROUNDED window, not the raw now-anchored one.
    const firstStart = wrapper.findComponent(TracesFilters).props('startMs')

    // Advance the wall clock by 12s (the live tick's cadence) and force a fresh search so the
    // list query re-anchors `nowTick` via buildRequest.
    nowSpy.mockReturnValue(base + 12_000)
    const body = await openSelect(wrapper, 'Sort results')
    await body.find('[data-testid="select-option-slowest"]').trigger('click')
    await new Promise((resolve) => setTimeout(resolve, 250))
    await flushPromises()

    // The raw window moved by 12s, but the 60s-bucketed agg window the rail sees is unchanged.
    expect(wrapper.findComponent(TracesFilters).props('startMs')).toBe(firstStart)
    nowSpy.mockRestore()
    wrapper.unmount()
  })

  it('maps only-value / toggle-value (uncheck → exclude) from the rails onto the query text', async () => {
    // Seed the query text (`q`) in the URL, the way useUrlState hydrates it on setup.
    window.history.replaceState(null, '', '/traces?q=' + encodeURIComponent('service:a,b'))
    const { wrapper } = await mountExplorer()
    const currentText = () => wrapper.findComponent(TracesFilters).props('query')

    // "Only a" narrows the service OR-list to exactly a.
    wrapper.findComponent(TracesFilters).vm.$emit('only-value', { field: 'service', value: 'a' })
    await flushPromises()
    expect(currentText()).toBe('service:a')

    // Single-state: the rails no longer emit a separate `exclude-value` — unchecking a value on a
    // field with no prior include (all-mode) writes the `-kind:client` exclusion via toggle-value.
    wrapper.findComponent(TracesFilters).vm.$emit('toggle-value', { field: 'kind', value: 'client' })
    await flushPromises()
    expect(currentText()).toContain('-kind:client')
    wrapper.unmount()
  })

  it('passes the drawer selection to the active table as selectedId', async () => {
    const { wrapper } = await mountExplorer()
    expect(wrapper.findComponent(TraceTable).props('selectedId')).toBeFalsy()

    // Opening the peek drawer on a row hands that trace id to the table as selectedId.
    wrapper.findComponent(TraceTable).vm.$emit('open-trace', { traceId: 't1', timeHintNs: '999' })
    await flushPromises()
    expect(wrapper.findComponent(TraceTable).props('selectedId')).toBe('t1')
    wrapper.unmount()
  })
})

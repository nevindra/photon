// Guards the app-wide Tooltip regression: NavRail and VolumeHistogram both render Reka
// <Tooltip>s, which throw "Injection Symbol(TooltipProviderContext) not found" unless a
// <TooltipProvider> ancestor exists. App.vue provides exactly one at the root; this test
// mirrors that wrapping and asserts LogsView mounts (and renders its chrome) without
// throwing. It would fail loudly if the provider were removed.
import { describe, it, expect, vi, beforeEach, beforeAll, afterAll } from 'vitest'
import { mount, shallowMount, flushPromises, DOMWrapper } from '@vue/test-utils'
import { nextTick, toValue } from 'vue'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import LogsView from './LogsView.vue'
import LogDetailDrawer from '@/components/logs/LogDetailDrawer.vue'
import LogTable from '@/components/logs/LogTable.vue'
import LogsFilters from '@/components/logs/LogsFilters.vue'
import SearchBar from '@/components/common/SearchBar.vue'
import { api } from '@/lib/core/api'
import { timeRange, customRange, startNs, endNs } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'

// LogTable virtualizes its rows; virtual-core reads the scroll element's offsetHeight and yields
// an empty range for a zero-height viewport (all jsdom reports). Stub a real height/width for the
// whole file so a live-streamed row (see the mode-picker describe block below) actually renders
// instead of windowing down to nothing (mirrors TracesExplorer.test.js's identical stub).
let restoreOffsetHeight, restoreOffsetWidth
beforeAll(() => {
  restoreOffsetHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight')
  restoreOffsetWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth')
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, get: () => 600 })
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, get: () => 900 })
})
afterAll(() => {
  if (restoreOffsetHeight) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', restoreOffsetHeight)
  if (restoreOffsetWidth) Object.defineProperty(HTMLElement.prototype, 'offsetWidth', restoreOffsetWidth)
})

// Wrap useSearchLogs so a test can read the reactive query options LogsView passes it (chiefly
// `refetchInterval`, to assert the live-tail mode picker) while still delegating to the real
// composable — everything else in logsQueries.js (useServices/useFields/etc., also consumed by
// FacetRail/VolumeHistogram) is the genuine article. Mirrors TracesExplorer.test.js's technique.
const captured = vi.hoisted(() => ({ search: null }))
vi.mock('@/lib/logs/logsQueries', async (importOriginal) => {
  const actual = await importOriginal()
  return {
    ...actual,
    useSearchLogs: (key, build, opts) => {
      captured.search = opts
      return actual.useSearchLogs(key, build, opts)
    },
  }
})

// useLiveTail (the real composable) opens its SSE stream through this thin EventSource wrapper.
// Mock it the same way useLiveTail.test.js does, so Live mode can be driven from a test without a
// real EventSource (unavailable in jsdom).
let streamHandle
vi.mock('@/lib/core/liveStream', () => ({
  openLiveStream: vi.fn((opts) => {
    streamHandle = { opts, close: vi.fn() }
    return streamHandle
  }),
}))

// LogsView (and the leaf components it renders) now use TanStack Query, so every mount needs a
// QueryClient. Use a FRESH client per mount with retries off and no GC, so one test's cache never
// leaks into the next and failed/mock queries resolve deterministically.
function queryPlugin() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } },
  })
  return [VueQueryPlugin, { queryClient }]
}

// No network. `search` resolves to the envelope shape ({ rows, matched_count, elapsed_ms });
// `rows` is empty (so the empty-state chrome still renders) but `matched_count` is a large
// server total — that mismatch is what proves the toolbar reads the server count, not
// `rows.length`. `fields`/`facet`/`histogram` back the facet rail, column picker, and the
// server histogram. `mock` mirrors the real api's plain-boolean field so AppShell's :mock
// binding renders.
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    services: vi.fn().mockResolvedValue([]),
    search: vi.fn().mockResolvedValue({ rows: [], matched_count: 4210, elapsed_ms: 12 }),
    fields: vi.fn().mockResolvedValue([{ name: 'service.name', kind: 'promoted' }]),
    facet: vi.fn().mockResolvedValue({ values: [], capped: false }),
    histogram: vi.fn().mockResolvedValue([]),
    login: vi.fn().mockResolvedValue({ ok: true }),
    logout: vi.fn().mockResolvedValue(undefined),
  },
}))

// LogsView + AppShell now use useRoute/useRouter, so a router must be installed.
const routes = [
  { path: '/logs', component: { template: '<div />' } },
  { path: '/traces/:traceId', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

async function makeRouter(initial = '/logs') {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(initial)
  await router.isReady()
  return router
}

// LiveControl's refresh-mode picker (SelectMenu) renders its options only while open, teleported
// to document.body — open the trigger, then query the body (same pattern as SelectMenu.test.js /
// the "Columns" popover assertion above).
async function openSelect(wrapper, ariaLabel) {
  await wrapper.get(`[aria-label="${ariaLabel}"]`).trigger('click')
  await nextTick()
  await new Promise((r) => setTimeout(r, 0))
  return new DOMWrapper(document.body)
}

async function mountLogs() {
  const router = await makeRouter('/logs')
  return mount(
    {
      components: { TooltipProvider, LogsView },
      template: '<TooltipProvider><LogsView /></TooltipProvider>',
    },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
}

// Time is now owned by the global context.js singleton (Task 6) rather than view-local state —
// reset it before every test so a preset/custom range set by one case never leaks into the next.
beforeEach(() => {
  timeRange.value = '30m'
  customRange.value = null
})

describe('LogsView (integration)', () => {
  it('mounts inside a TooltipProvider without throwing', async () => {
    const wrapper = await mountLogs()
    await flushPromises()
    expect(wrapper.exists()).toBe(true)
    wrapper.unmount()
  })

  it('renders the nav rail and the log table chrome', async () => {
    const wrapper = await mountLogs()
    await flushPromises()

    // NavRail
    expect(wrapper.find('nav').exists()).toBe(true)
    // LogTable header + list
    expect(wrapper.find('[role="listbox"]').exists()).toBe(true)
    expect(wrapper.text()).toContain('Message')
    // Empty state (search() returned [])
    expect(wrapper.text()).toContain('No logs match')

    wrapper.unmount()
  })

  it('calls the (mocked) api on mount', async () => {
    const { api } = await import('@/lib/core/api')
    const wrapper = await mountLogs()
    await flushPromises()
    expect(api.services).toHaveBeenCalled()
    expect(api.fields).toHaveBeenCalled()
    expect(api.search).toHaveBeenCalled()
    wrapper.unmount()
  })

  it('shows the server matched_count in the toolbar, not the loaded row count', async () => {
    const wrapper = await mountLogs()
    await flushPromises()
    // The server reports 4210 total matches while only an (empty) page of rows is loaded, so
    // rows.length is 0. The toolbar must show the formatted server total ("4,210"), proving it
    // reads matched_count rather than results.length.
    expect(wrapper.text()).toContain('4,210')
    wrapper.unmount()
  })

  // Regression: LogTable's Time/Level/Service/Message columns are hardcoded and NOT gated by the
  // `columns` prop — that prop only ever renders `row.attributes?.[key]`. So a `kind: 'fixed'`
  // field (like `timestamp`) must never be offered as a toggleable column (it would add a
  // permanently-blank column), while a real `kind: 'attribute'` field must still be offered.
  it('offers only attribute fields in the column picker, not fixed built-ins', async () => {
    const { api } = await import('@/lib/core/api')
    // `fields`'s query key includes the now-anchored window, which `buildRequest` re-anchors on
    // every fetch — so a *One override can be consumed by a background refetch before we assert.
    // Override persistently for the life of this test, then restore the file's default afterward.
    api.fields.mockResolvedValue([
      { name: 'timestamp', kind: 'fixed' },
      { name: 'service.name', kind: 'promoted' },
      { name: 'host.name', kind: 'attribute' },
    ])
    const wrapper = await mountLogs()
    await flushPromises()

    const trigger = wrapper.findAll('button').find((b) => b.text() === 'Columns')
    expect(trigger).toBeTruthy()
    await trigger.trigger('click')
    await nextTick()
    // Popper positioning settles on a microtask; flush once more so the teleported
    // PopoverContent is present in document.body before we query it.
    await new Promise((resolve) => setTimeout(resolve, 0))

    const body = new DOMWrapper(document.body)
    expect(body.find('[data-test="col-toggle-timestamp"]').exists()).toBe(false)
    expect(body.find('[data-test="col-toggle-service.name"]').exists()).toBe(false)
    expect(body.find('[data-test="col-toggle-host.name"]').exists()).toBe(true)

    wrapper.unmount()
    api.fields.mockResolvedValue([{ name: 'service.name', kind: 'promoted' }])
  })

  // The view used to own RANGE_MS/timeRange/customRange/nowTick locally; it now reads the
  // app-wide window from lib/context.js (mounted globally via ContextBar in AppShell), so
  // picking a preset there — with no LogsView-local wiring at all — must reshape the request.
  it('logs query uses the global context window', async () => {
    timeRange.value = '15m'
    const wrapper = await mountLogs()
    await flushPromises()

    expect(api.search).toHaveBeenCalled()
    const req = api.search.mock.calls.at(-1)[0]
    expect(req.start_ts_nanos).toBe(startNs.value)
    expect(req.end_ts_nanos).toBe(endNs.value)

    wrapper.unmount()
  })
})

// The live-tail mode picker (LiveControl) replaced the old bare `live` boolean + Switch. These
// assert the two load-bearing behaviors from the FE-4 brief: the mode picker drives the search
// query's `refetchInterval` (mirroring TracesExplorer.test.js's refetchInterval assertions), and
// Live mode's streamed rows (from useLiveTail's real SSE-backed implementation, with the
// EventSource wrapper mocked) render in the table via `displayRows`.
describe('LogsView — live tail mode picker', () => {
  beforeEach(() => {
    captured.search = null
    streamHandle = null
  })

  it('5s sets the search refetchInterval to 5000ms; manual disables it again', async () => {
    const wrapper = await mountLogs()
    await flushPromises()
    // Manual (the initial mode) never polls.
    expect(toValue(captured.search.refetchInterval)).toBe(false)

    let body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-5s"]').trigger('click')
    await flushPromises()
    expect(toValue(captured.search.refetchInterval)).toBe(5000)

    body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-manual"]').trigger('click')
    await flushPromises()
    expect(toValue(captured.search.refetchInterval)).toBe(false)

    wrapper.unmount()
  })

  it('renders streamed rows in the table once Live mode is selected', async () => {
    const wrapper = await mountLogs()
    await flushPromises()

    const body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-live"]').trigger('click')
    await flushPromises()
    // Selecting Live opened the (mocked) SSE stream.
    expect(streamHandle).toBeTruthy()

    streamHandle.opts.onRows([
      {
        id: 'live-1',
        timestamp: 1_700_000_000_000_000_000n,
        severity: 'info',
        service: 'checkout',
        body: 'streamed live row',
        attributes: {},
      },
    ])
    await nextTick()
    await flushPromises()

    expect(wrapper.text()).toContain('streamed live row')
    wrapper.unmount()
  })
})

// Regression: the span/trace → logs correlation pivot. A pivot is now `router.push('/logs?q=…')`,
// which MOUNTS A FRESH LogsView whose route already carries `q`. LogsView must seed `text` from
// `route.query.q` at setup (before the scheduleSearch watcher) or the `trace_id:…` query is
// silently dropped. These use shallowMount so all heavy children (AppShell/SearchBar/…) are
// auto-stubbed — no TooltipProvider ancestor needed — but a router plugin is still required.
describe('LogsView — route q param seeds the search', () => {
  // useUrlState persists `text` into window.location (history.replaceState → `?q=…`), so a prior
  // test could leak into the next mount. Reset the URL (and the search spy) before each case to
  // keep them independent and to prove the seed comes from the router query, not window.location.
  beforeEach(() => {
    window.history.replaceState(null, '', '/')
    api.search.mockClear()
  })

  it('seeds the search from the route q param present at mount', async () => {
    const router = await makeRouter('/logs?q=trace_id:abc123')
    const wrapper = shallowMount(LogsView, { global: { plugins: [router, queryPlugin()] } })
    await flushPromises()
    // The setup seed must run before the first (mount) search, so the request carries the pivot query.
    expect(api.search).toHaveBeenCalled()
    expect(api.search.mock.calls[0][0].query).toBe('trace_id:abc123')
    wrapper.unmount()
  })

  it('mounts with no q param without throwing and keeps text empty', async () => {
    const router = await makeRouter('/logs')
    const wrapper = shallowMount(LogsView, { global: { plugins: [router, queryPlugin()] } })
    await flushPromises()
    expect(api.search).toHaveBeenCalled()
    expect(api.search.mock.calls[0][0].query).toBe('')
    wrapper.unmount()
  })

  it('drawer @next/@prev step the selection and clamp at the ends', async () => {
    const threeRows = [
      { id: 'r0', timestamp: 1n, service: 'a', severity: 'info', body: 'first', attributes: {} },
      { id: 'r1', timestamp: 2n, service: 'b', severity: 'info', body: 'second', attributes: {} },
      { id: 'r2', timestamp: 3n, service: 'c', severity: 'info', body: 'third', attributes: {} },
    ]
    api.search.mockResolvedValue({ rows: threeRows, matched_count: 3, elapsed_ms: 1 })

    const router = await makeRouter('/logs')
    const wrapper = shallowMount(LogsView, {
      global: {
        plugins: [router, queryPlugin()],
        stubs: { AppShell: { template: '<div><slot /></div>' } },
      },
    })
    await flushPromises()

    const table = wrapper.findComponent(LogTable)
    const drawer = wrapper.findComponent(LogDetailDrawer)

    // Select the middle row.
    table.vm.$emit('select', 'r1')
    await nextTick()
    expect(drawer.props('index')).toBe(1)
    expect(drawer.props('total')).toBe(3)
    expect(drawer.props('row').id).toBe('r1')

    // next → r2
    drawer.vm.$emit('next')
    await nextTick()
    expect(drawer.props('index')).toBe(2)
    expect(drawer.props('row').id).toBe('r2')

    // next at the last row is a no-op (clamped)
    drawer.vm.$emit('next')
    await nextTick()
    expect(drawer.props('index')).toBe(2)

    // prev → r1 → r0, then prev at the first row is a no-op
    drawer.vm.$emit('prev')
    await nextTick()
    expect(drawer.props('index')).toBe(1)
    drawer.vm.$emit('prev')
    await nextTick()
    expect(drawer.props('index')).toBe(0)
    drawer.vm.$emit('prev')
    await nextTick()
    expect(drawer.props('index')).toBe(0)

    wrapper.unmount()
    api.search.mockResolvedValue({ rows: [], matched_count: 4210, elapsed_ms: 12 })
  })

  it('drawer @filter-value updates the query text (positive and negated)', async () => {
    const router = await makeRouter('/logs')
    const wrapper = shallowMount(LogsView, {
      global: {
        plugins: [router, queryPlugin()],
        // Render both AppShell slots so the SearchBar (forwarded via #toolbar) and the default-slot
        // content (LogDetailDrawer) both mount under the stub.
        stubs: { AppShell: { template: '<div><slot name="toolbar" /><slot /></div>' } },
      },
    })
    await flushPromises()

    const drawer = wrapper.findComponent(LogDetailDrawer)
    const searchBar = wrapper.findComponent(SearchBar)

    drawer.vm.$emit('filter-value', { field: 'service', value: 'api', negate: false })
    await nextTick()
    expect(searchBar.props('modelValue')).toBe('service:api')

    // A filter-out appends the distinct negated term.
    drawer.vm.$emit('filter-value', { field: 'level', value: 'debug', negate: true })
    await nextTick()
    expect(searchBar.props('modelValue')).toBe('service:api -level:debug')

    wrapper.unmount()
  })

  it('pivots to the trace waterfall route when the drawer emits view-trace', async () => {
    const router = await makeRouter('/logs')
    const push = vi.spyOn(router, 'push')
    // Stub AppShell with a slot-rendering template so the (auto-stubbed) LogDetailDrawer inside
    // its default slot is present to emit from — plain shallowMount stubs don't render slots.
    const wrapper = shallowMount(LogsView, {
      global: {
        plugins: [router, queryPlugin()],
        stubs: { AppShell: { template: '<div><slot /></div>' } },
      },
    })
    await flushPromises()
    wrapper
      .findComponent(LogDetailDrawer)
      .vm.$emit('view-trace', { traceId: 'tX', timeHintNs: '99' })
    // Now routed through correlate(), so the pivot also carries the active time window (range=…).
    expect(push).toHaveBeenCalledWith(correlate({ path: '/traces/tX', query: { t: '99' } }))
    wrapper.unmount()
  })
})

// R7: LogsFilters (the unified pinned + catalog panel) speaks the single-state facet model
// (facet-single-state-model.md) — `:query="text"` in, `toggle-value`/`only-value`/`clear-field`
// out — rather than the old `selected-services`/`toggle-service`/`clear-services` wiring. These
// assert LogsView routes each emit through the right pure `queryLang.js` helper.
describe('LogsView — LogsFilters single-state wiring (R7)', () => {
  it('toggle-value rewrites the query via toggleFacetValue (all-mode unchecking excludes)', async () => {
    const router = await makeRouter('/logs')
    const wrapper = shallowMount(LogsView, {
      global: {
        plugins: [router, queryPlugin()],
        stubs: { AppShell: { template: '<div><slot name="toolbar" /><slot /></div>' } },
      },
    })
    await flushPromises()

    wrapper.findComponent(LogsFilters).vm.$emit('toggle-value', { field: 'service', value: 'checkout' })
    await nextTick()

    // Starting from an empty (all-checked) query, unchecking a value writes a `-field:value`
    // exclusion — toggleFieldValue (the old handler) would instead have written a positive
    // `service:checkout` include, so this also proves the NEW helper is wired, not the old one.
    expect(wrapper.findComponent(SearchBar).props('modelValue')).toBe('-service:checkout')
    wrapper.unmount()
  })

  it('only-value narrows the query via onlyFieldValue', async () => {
    const router = await makeRouter('/logs')
    const wrapper = shallowMount(LogsView, {
      global: {
        plugins: [router, queryPlugin()],
        stubs: { AppShell: { template: '<div><slot name="toolbar" /><slot /></div>' } },
      },
    })
    await flushPromises()

    wrapper.findComponent(LogsFilters).vm.$emit('only-value', { field: 'service', value: 'checkout' })
    await nextTick()

    expect(wrapper.findComponent(SearchBar).props('modelValue')).toBe('service:checkout')
    wrapper.unmount()
  })

  it('clear-field resets the field via removeFieldAll (drops an exclusion, keeps other fields)', async () => {
    // Seeded with a NEGATED `-service:` term — `removeField` (the old handler) only drops
    // POSITIVE `field:` terms and would have left `-service:api` behind; `removeFieldAll` drops
    // both signs, so this distinguishes the two helpers.
    const router = await makeRouter({ path: '/logs', query: { q: '-service:api status:error' } })
    const wrapper = shallowMount(LogsView, {
      global: {
        plugins: [router, queryPlugin()],
        stubs: { AppShell: { template: '<div><slot name="toolbar" /><slot /></div>' } },
      },
    })
    await flushPromises()

    wrapper.findComponent(LogsFilters).vm.$emit('clear-field', 'service')
    await nextTick()

    expect(wrapper.findComponent(SearchBar).props('modelValue')).toBe('status:error')
    wrapper.unmount()
  })
})

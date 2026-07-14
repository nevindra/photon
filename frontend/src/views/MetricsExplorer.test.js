import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { toValue, nextTick } from 'vue'
import { mount, flushPromises, DOMWrapper } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { createRouter, createWebHistory } from 'vue-router'
import { TooltipProvider } from '@/components/ui/tooltip'
import { api } from '@/lib/core/api'
import { timeRange, customRange, startNs, endNs, setTimeRange } from '@/lib/core/context'
import MetricsExplorer from './MetricsExplorer.vue'
import MetricChart from '@/components/metrics/MetricChart.vue'
import { parseViz, serializeViz } from '@/lib/metrics/metricViz'

// Metrics never streams: `useMetricSeries` is wrapped (delegating to the real composable) so the
// test can read the reactive `refetchInterval` the view passes it, mirroring TracesExplorer.test.js's
// `captured` pattern. `liveStream.js` is mocked wholesale so we can assert `openLiveStream` is never
// constructed even when the LiveControl is driven to "Live" (it must resolve to a fast poll instead).
const captured = vi.hoisted(() => ({ series: null }))
vi.mock('@/lib/metrics/metricsQueries', async (importOriginal) => {
  const actual = await importOriginal()
  return {
    ...actual,
    useMetricSeries: (key, build, opts) => {
      captured.series = opts
      return actual.useMetricSeries(key, build, opts)
    },
  }
})
vi.mock('@/lib/core/liveStream', () => ({
  openLiveStream: vi.fn(() => ({ close: vi.fn() })),
}))
import { openLiveStream } from '@/lib/core/liveStream'

// A tiny fixed catalog: a gauge/sum-style metric + a histogram (both chartable as of Task 6).
const CATALOG = [
  { name: 'http.server.requests', type: 'sum', unit: '1', temporality: 'cumulative', is_monotonic: true, series_count: 12, last_seen: '0' },
  { name: 'http.server.duration', type: 'histogram', unit: 'ms', temporality: 'cumulative', is_monotonic: null, series_count: 30, last_seen: '0' },
]
const META = {
  name: 'http.server.requests', type: 'sum', temporality: 'cumulative', unit: '1',
  is_monotonic: true, series_count: 12, last_seen: '0', attribute_keys: ['service'],
}
const SERIES = {
  results: [{
    id: 'a',
    series: [{ labels: { service: 'checkout' }, points: [{ t: '0', v: 5 }, { t: '1000000', v: 9 }], exemplars: [] }],
    default_agg: 'rate',
  }],
  step: '1', capped: false, elapsed_ms: 1,
}

// Mirrors the sibling MetricsView.test.js harness: a real QueryClient + a vue-router + a
// TooltipProvider ancestor (AppShell → NavRail uses a Reka Tooltip). MetricsExplorer is a child
// of the wrapper, so wrapper.find*/findAll traverse the whole rendered tree.
function makeHarness() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const router = createRouter({
    history: createWebHistory(),
    routes: [
      { path: '/metrics', name: 'metrics', component: MetricsExplorer },
      { path: '/metrics/catalog', name: 'metrics-catalog', component: MetricsExplorer },
      { path: '/traces', name: 'traces', component: { template: '<div/>' } },
      { path: '/', component: { template: '<div/>' } },
    ],
  })
  router.push('/metrics')
  return { queryClient, router }
}
function mountWrapped(queryClient, router) {
  return mount(
    { components: { TooltipProvider, MetricsExplorer }, template: '<TooltipProvider><MetricsExplorer /></TooltipProvider>' },
    { global: { plugins: [[VueQueryPlugin, { queryClient }], router] }, attachTo: document.body },
  )
}

// LiveControl's refresh-mode picker (SelectMenu) renders its options only while open, teleported
// to document.body — open the trigger, then query the body (same pattern as SelectMenu.test.js).
async function openSelect(wrapper, ariaLabel) {
  await wrapper.get(`[aria-label="${ariaLabel}"]`).trigger('click')
  await nextTick()
  await new Promise((r) => setTimeout(r, 0))
  return new DOMWrapper(document.body)
}

beforeEach(() => {
  vi.spyOn(api, 'metricCatalog').mockResolvedValue(CATALOG)
  vi.spyOn(api, 'metricMetadata').mockResolvedValue(META)
  vi.spyOn(api, 'metricQuery').mockResolvedValue(SERIES) // useMetricSeries calls api.metricQuery
  vi.spyOn(api, 'services').mockResolvedValue(['checkout', 'cart'])
  captured.series = null
  // restoreAllMocks (afterEach) reverts a bare vi.fn() to a no-op, so re-arm the implementation
  // fresh every test rather than relying on the vi.mock() factory's one-time initial value.
  openLiveStream.mockReset()
  openLiveStream.mockImplementation(() => ({ close: vi.fn() }))
  // Time is now global (lib/context) — reset the module singletons between tests.
  customRange.value = null
  setTimeRange('30m')
})
afterEach(() => vi.restoreAllMocks())

describe('MetricsExplorer', () => {
  it('renders the shell with the query builder', async () => {
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    expect(wrapper.find('[data-testid="query-badge"]').exists()).toBe(true)
  })

  it('charts a gauge/sum metric and shows its legend', async () => {
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    // simulate selecting the sum metric via the exposed ref
    wrapper.findComponent(MetricsExplorer).vm.metric = 'http.server.requests'
    await flushPromises()
    await flushPromises()
    expect(api.metricQuery).toHaveBeenCalled()
    // MetricChart now renders via uPlot canvas (no more per-line SVG nodes) — assert the chart
    // component is mounted with the fetched series (not the empty/not-chartable state) instead.
    expect(wrapper.findComponent(MetricChart).exists()).toBe(true)
    expect(wrapper.findComponent(MetricChart).props('series').length).toBeGreaterThan(0)
    expect(wrapper.find('[data-testid="chart-empty"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="legend-row"]').exists()).toBe(true)
  })

  it('queries the series with the global context window', async () => {
    setTimeRange('15m')
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    api.metricQuery.mockClear()
    wrapper.findComponent(MetricsExplorer).vm.metric = 'http.server.requests'
    await flushPromises()
    await flushPromises()
    expect(api.metricQuery).toHaveBeenCalled()
    // buildRequest resolves start/end from lib/context (not a view-local window).
    const body = api.metricQuery.mock.calls[0][0]
    expect(body.start).toBe(startNs.value)
    expect(body.end).toBe(endNs.value)
    expect(timeRange.value).toBe('15m')
  })

  it('charts a histogram metric (quantile-over-time) instead of the not-chartable placeholder', async () => {
    api.metricMetadata.mockResolvedValue({ ...META, name: 'http.server.duration', type: 'histogram' })
    api.metricQuery.mockResolvedValue({
      results: [{
        id: 'a',
        series: [{ labels: { service: 'checkout' }, points: [{ t: '0', v: 12 }, { t: '1000000', v: 15 }], exemplars: [] }],
        default_agg: 'p99',
      }],
      step: '1', capped: false, elapsed_ms: 1,
    })
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    api.metricQuery.mockClear()
    wrapper.findComponent(MetricsExplorer).vm.metric = 'http.server.duration'
    await flushPromises()
    await flushPromises()
    expect(api.metricQuery).toHaveBeenCalled()
    expect(wrapper.find('[data-testid="chart-not-chartable"]').exists()).toBe(false)
    // Histogram series ARE chartable (quantile-over-time) — the real chart renders, not the
    // not-chartable placeholder. uPlot draws to canvas, so assert via the chart component + data.
    expect(wrapper.findComponent(MetricChart).exists()).toBe(true)
    expect(wrapper.findComponent(MetricChart).props('series').length).toBeGreaterThan(0)
    expect(wrapper.find('[data-testid="chart-empty"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="legend-row"]').exists()).toBe(true)
  })

  it('switches to the Catalog tab and lists metrics', async () => {
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    // The Explore/Catalog switch is now route-based nav (NavTabItem is a RouterLink to
    // /metrics/catalog), so drive it by navigating rather than clicking the tab. The reused
    // component instance derives `mode` from the path.
    await router.push('/metrics/catalog')
    await flushPromises()
    expect(wrapper.get('[data-testid="mode-catalog"]').attributes('aria-current')).toBe('page')
    expect(wrapper.findAll('[data-testid="catalog-row"]').length).toBe(CATALOG.length)
  })

  // Metrics is a window-refresh chart, not a stream: the LiveControl mode maps to a poll interval
  // (or `false` for Manual), never to `openLiveStream`. Mirrors the mode→interval matrix other
  // explorers assert, but metrics' "Live" resolves to a FAST poll (2s) instead of a stream.
  it('defaults to Manual: refetchInterval is false', async () => {
    const { queryClient, router } = makeHarness()
    await router.isReady()
    mountWrapped(queryClient, router)
    await flushPromises()
    expect(toValue(captured.series.refetchInterval)).toBe(false)
  })

  it('5s mode polls every 5000ms', async () => {
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    const body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-5s"]').trigger('click')
    await flushPromises()
    expect(toValue(captured.series.refetchInterval)).toBe(5000)
  })

  it('30s mode polls every 30000ms', async () => {
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    const body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-30s"]').trigger('click')
    await flushPromises()
    expect(toValue(captured.series.refetchInterval)).toBe(30000)
  })

  it('Live mode fast-polls every 2000ms and never opens an EventSource', async () => {
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    const body = await openSelect(wrapper, 'Refresh mode')
    await body.find('[data-testid="select-option-live"]').trigger('click')
    await flushPromises()
    expect(toValue(captured.series.refetchInterval)).toBe(2000)
    expect(openLiveStream).not.toHaveBeenCalled()
  })

  // Empty state (no metric picked) now shows curated quick-start template cards instead of the
  // static "Pick a metric to chart it." placeholder — selecting one seeds the whole builder.
  it('shows quick-start templates when no metric is picked, and selecting one seeds the builder', async () => {
    const { queryClient, router } = makeHarness()
    await router.isReady()
    const wrapper = mountWrapped(queryClient, router)
    await flushPromises()
    expect(wrapper.find('[data-testid="metric-quickstarts"]').exists()).toBe(true)
    const cards = wrapper.findAll('[data-testid="quickstart-card"]')
    expect(cards.length).toBeGreaterThan(0)
    await cards[0].trigger('click')
    await flushPromises()
    const vm = wrapper.findComponent(MetricsExplorer).vm
    // The CATALOG fixture matches the "HTTP p99 latency" curated card (http.server.duration).
    expect(vm.metric).toBe('http.server.duration')
    expect(vm.agg).toBe('p99')
    expect(vm.viz).toBe('line')
  })
})

// Unit-level coverage for the URL viz codec (Task 4's lib/metricViz.ts, consumed directly by this
// view's URL seed/layering watcher) — asserts wiring, not uPlot rendering.
describe('MetricsExplorer viz URL codec (unit)', () => {
  it('round-trips a non-default viz and omits the default', () => {
    expect(parseViz(serializeViz('bar') || null)).toBe('bar')
    expect(serializeViz('line')).toBe('') // default omitted
  })
})

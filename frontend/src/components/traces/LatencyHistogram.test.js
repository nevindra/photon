// LatencyHistogram is now a thin adapter over charts/BarChart.vue — assert the prop mapping it
// does (ns→Number bucket.t, single 'count' segment, value-mode x-axis/xFormat, p50/p90/p99
// markers) and the brush re-emit, not chart pixels. uPlot never constructs in jsdom and marker
// overlays are positioned from uPlot pixel math (see BarChart/BaseChart), so we stub BarChart and
// inspect the props LatencyHistogram forwards to it instead of rendered DOM.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import LatencyHistogram from './LatencyHistogram.vue'
import BarChart from '@/components/charts/BarChart.vue'
import { api } from '@/lib/core/api'
import { formatDuration, formatNumber } from '@/lib/core/format'

// Deterministic token colours (tokens.css light triplets) so marker/bar colour assertions don't
// depend on real CSS ever loading in jsdom — mirrors useChartTheme.test.js's stub pattern.
const TOKENS = { '--muted-foreground': '0 0% 45.1%', '--sev-warn': '32 81% 35%', '--sev-error': '0 72% 51%' }

function mountLatency(props) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(LatencyHistogram, {
    props,
    global: {
      plugins: [[VueQueryPlugin, { queryClient }]],
      stubs: { BarChart: true },
    },
  })
}

// Three 1ms buckets (bucket_ns gap = 1_000_000), all values NANOSECOND strings per the wire shape.
const THREE_BUCKETS = {
  buckets: [
    { bucket_ns: '0', count: 2 },
    { bucket_ns: '1000000', count: 5 },
    { bucket_ns: '2000000', count: 1 },
  ],
  p50: '900000',
  p90: '1800000',
  p99: '1950000',
}

beforeEach(() => {
  vi.stubGlobal('getComputedStyle', () => ({
    getPropertyValue: (name) => TOKENS[name] ?? '',
  }))
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('LatencyHistogram', () => {
  it('fetches tracesLatency for the current query/window', async () => {
    const spy = vi.spyOn(api, 'tracesLatency').mockResolvedValue(THREE_BUCKETS)
    mountLatency({ query: 'status:error', startMs: 0, endMs: 60_000 })
    await flushPromises()

    expect(spy).toHaveBeenCalledWith(
      'status:error',
      '0',
      '60000000000',
      48,
      expect.objectContaining({ signal: expect.anything() }),
    )
  })

  it('maps buckets to BarChart props: value-mode x-axis, ns bucket.t → Number, single count segment', async () => {
    vi.spyOn(api, 'tracesLatency').mockResolvedValue(THREE_BUCKETS)
    const wrapper = mountLatency({ query: '', startMs: 0, endMs: 60_000 })
    await flushPromises()

    const bar = wrapper.findComponent(BarChart)
    expect(bar.props('xUnit')).toBe('value')
    expect(bar.props('xLog')).toBe(true) // latency is long-tailed → log x-axis over geometric buckets
    expect(bar.props('xFormat')).toBe(formatDuration)
    expect(bar.props('formatValue')).toBe(formatNumber)
    expect(bar.props('buckets')).toEqual([
      { t: 0, segments: [{ key: 'count', label: 'Count', color: 'hsl(0 0% 45.1%)', value: 2 }] },
      { t: 1_000_000, segments: [{ key: 'count', label: 'Count', color: 'hsl(0 0% 45.1%)', value: 5 }] },
      { t: 2_000_000, segments: [{ key: 'count', label: 'Count', color: 'hsl(0 0% 45.1%)', value: 1 }] },
    ])
  })

  it('forwards p50/p90/p99 as markers with the right ns x-values and theme-resolved colors', async () => {
    vi.spyOn(api, 'tracesLatency').mockResolvedValue(THREE_BUCKETS)
    const wrapper = mountLatency({ query: '', startMs: 0, endMs: 60_000 })
    await flushPromises()

    expect(wrapper.findComponent(BarChart).props('markers')).toEqual([
      { x: 900_000, label: 'p50', color: '#0ea5e9' },
      { x: 1_800_000, label: 'p90', color: 'hsl(32 81% 35%)' },
      { x: 1_950_000, label: 'p99', color: 'hsl(0 72% 51%)' },
    ])
  })

  it('re-emits BarChart brush {minNs,maxNs} through unchanged', async () => {
    vi.spyOn(api, 'tracesLatency').mockResolvedValue(THREE_BUCKETS)
    const wrapper = mountLatency({ query: '', startMs: 0, endMs: 60_000 })
    await flushPromises()

    wrapper.findComponent(BarChart).vm.$emit('brush', { minNs: 1_000_000, maxNs: 2_000_000 })
    expect(wrapper.emitted('brush')?.at(-1)?.[0]).toEqual({ minNs: 1_000_000, maxNs: 2_000_000 })
  })

  it('forwards isPending as BarChart loading, clearing once data resolves', async () => {
    let resolveFirst
    vi.spyOn(api, 'tracesLatency').mockImplementationOnce(
      () => new Promise((resolve) => { resolveFirst = resolve }),
    )
    const wrapper = mountLatency({ query: '', startMs: 0, endMs: 60_000 })
    await flushPromises()

    expect(wrapper.findComponent(BarChart).props('loading')).toBe(true)

    resolveFirst(THREE_BUCKETS)
    await flushPromises()

    expect(wrapper.findComponent(BarChart).props('loading')).toBe(false)
  })

  it('forwards empty buckets/markers when there is no data', async () => {
    vi.spyOn(api, 'tracesLatency').mockResolvedValue({ buckets: [], p50: '0', p90: '0', p99: '0' })
    const wrapper = mountLatency({ query: '', startMs: 0, endMs: 1000 })
    await flushPromises()

    const bar = wrapper.findComponent(BarChart)
    expect(bar.props('buckets')).toEqual([])
    expect(bar.props('markers')).toEqual([])
  })

  it('clears buckets/markers on a failed fetch (bad query is surfaced by the search box, not here)', async () => {
    vi.spyOn(api, 'tracesLatency').mockRejectedValue(Object.assign(new Error('bad'), { status: 400 }))
    const wrapper = mountLatency({ query: 'bogus(', startMs: 0, endMs: 1000 })
    await flushPromises()

    const bar = wrapper.findComponent(BarChart)
    expect(bar.props('buckets')).toEqual([])
    expect(bar.props('markers')).toEqual([])
  })
})

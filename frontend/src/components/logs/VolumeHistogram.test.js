import { describe, it, expect, vi, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import VolumeHistogram from './VolumeHistogram.vue'
import BarChart from '@/components/charts/BarChart.vue'
import { formatNumber } from '@/lib/core/format'
import { api } from '@/lib/core/api'

// VolumeHistogram is now a thin adapter over charts/BarChart.vue — it still owns its own fetch
// (`useHistogram` from logsQueries.js, unchanged), but rendering/tooltip/legend/drag-to-zoom now
// live in BarChart. Stub BarChart and assert the PROP MAPPING the adapter does, not chart pixels
// (mirrors metrics/MetricChart.test.js's technique for the equivalent LineChart adapter).
function mountHistogram(props) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(VolumeHistogram, {
    props,
    global: {
      plugins: [[VueQueryPlugin, { queryClient }]],
      stubs: { BarChart: true },
    },
  })
}

describe('VolumeHistogram', () => {
  afterEach(() => vi.restoreAllMocks())

  it('fetches histogram for the current query/window', async () => {
    const buckets = [
      { t: '0', debug: 0, info: 2, warn: 0, error: 1, fatal: 0, total: 3 },
      { t: '1000000000', debug: 0, info: 0, warn: 0, error: 0, fatal: 2, total: 2 },
    ]
    const spy = vi.spyOn(api, 'histogram').mockResolvedValue(buckets)
    const wrapper = mountHistogram({ query: 'level:error', startMs: 0, endMs: 60_000 })
    await flushPromises()

    expect(spy).toHaveBeenCalledWith(
      'level:error',
      '0',
      '60000000000',
      48,
      expect.objectContaining({ signal: expect.anything() }),
    )

    spy.mockRestore()
  })

  it('maps fetched buckets to BarChart segments: ns→ms t, bottom→top severity order, colours', async () => {
    const buckets = [
      { t: '0', debug: 1, info: 2, warn: 0, error: 1, fatal: 0, total: 4 },
      { t: '1000000000', debug: 0, info: 0, warn: 3, error: 0, fatal: 2, total: 5 },
    ]
    vi.spyOn(api, 'histogram').mockResolvedValue(buckets)
    const wrapper = mountHistogram({ query: 'level:error', startMs: 0, endMs: 60_000 })
    await flushPromises()

    const chartBuckets = wrapper.findComponent(BarChart).props('buckets')
    expect(chartBuckets).toHaveLength(2)

    // ns string -> ms Number
    expect(chartBuckets[0].t).toBe(0)
    expect(chartBuckets[1].t).toBe(1000)

    // bottom -> top order: debug, info, warn, error, fatal
    expect(chartBuckets[0].segments.map((s) => s.key)).toEqual(['debug', 'info', 'warn', 'error', 'fatal'])
    expect(chartBuckets[0].segments.map((s) => s.value)).toEqual([1, 2, 0, 1, 0])
    expect(chartBuckets[0].segments.map((s) => s.label)).toEqual(['Debug', 'Info', 'Warn', 'Error', 'Fatal'])

    // colours: only warn/error/fatal carry hue; debug/info resolve to --muted-foreground (jsdom
    // has no stylesheet, so getComputedStyle returns '' and the component's fallback triplets
    // kick in — the same ones baked into styles/tokens.css light theme). `debug` is flattened to an
    // OPAQUE grey (35% muted-fg over the white card) so its band survives being painted over the
    // opaque `info` bar on the canvas — see lib/color.js.
    const [debugSeg, infoSeg, warnSeg, errorSeg, fatalSeg] = chartBuckets[0].segments
    expect(debugSeg.color).toBe('rgb(206, 206, 206)') // flattenHsl('0 0% 45.1%', '0 0% 100%', 0.35)
    expect(infoSeg.color).toBe('hsl(0 0% 45.1%)')
    expect(warnSeg.color).toBe('hsl(32 81% 35%)')
    expect(errorSeg.color).toBe('hsl(0 72% 51%)')
    expect(fatalSeg.color).toBe('hsl(262 83% 58%)')
  })

  it('forwards startMs/endMs, stacked=true and formatValue to BarChart', async () => {
    vi.spyOn(api, 'histogram').mockResolvedValue([])
    const wrapper = mountHistogram({ query: '', startMs: 0, endMs: 60_000 })
    await flushPromises()

    const barChart = wrapper.findComponent(BarChart)
    expect(barChart.props('startMs')).toBe(0)
    expect(barChart.props('endMs')).toBe(60_000)
    expect(barChart.props('stacked')).toBe(true)
    expect(barChart.props('formatValue')).toBe(formatNumber)
  })

  it('re-emits zoom from BarChart unchanged', async () => {
    vi.spyOn(api, 'histogram').mockResolvedValue([])
    const wrapper = mountHistogram({ query: '', startMs: 0, endMs: 60_000 })
    await flushPromises()

    wrapper.findComponent(BarChart).vm.$emit('zoom', { startMs: 1000, endMs: 2000 })
    expect(wrapper.emitted('zoom')[0]).toEqual([{ startMs: 1000, endMs: 2000 }])
  })

  it('passes an empty buckets array to BarChart when there is no data in range', async () => {
    vi.spyOn(api, 'histogram').mockResolvedValue([])
    const wrapper = mountHistogram({ query: '', startMs: 0, endMs: 1000 })
    await flushPromises()

    expect(wrapper.findComponent(BarChart).props('buckets')).toEqual([])
  })

  it('sets loading=true on first load (pending, no placeholder yet)', () => {
    vi.spyOn(api, 'histogram').mockImplementation(() => new Promise(() => {}))
    const wrapper = mountHistogram({ query: '', startMs: 0, endMs: 60_000 })
    // No flushPromises: query is still pending with no data.
    expect(wrapper.findComponent(BarChart).props('loading')).toBe(true)
    wrapper.unmount()
  })

  it('keeps the previous buckets (not blanked) and drops loading while a refetch is in flight', async () => {
    const spy = vi
      .spyOn(api, 'histogram')
      .mockResolvedValueOnce([
        { t: '0', debug: 0, info: 2, warn: 0, error: 0, fatal: 0, total: 2 },
        { t: '1000000000', debug: 0, info: 1, warn: 0, error: 0, fatal: 0, total: 1 },
      ])
      // The refetch call never resolves, so we can inspect the in-flight state deterministically:
      // isFetching=true while placeholderData (the previous page's buckets) still holds `data`.
      .mockImplementationOnce(() => new Promise(() => {}))
    const wrapper = mountHistogram({ query: '', startMs: 0, endMs: 60_000 })
    await flushPromises()
    const before = wrapper.findComponent(BarChart).props('buckets')
    expect(before).toHaveLength(2)

    await wrapper.setProps({ query: 'level:error' })
    await flushPromises()

    const barChart = wrapper.findComponent(BarChart)
    // Not blanked: same buckets as before the refetch kicked off.
    expect(barChart.props('buckets')).toHaveLength(2)
    // isPending is false (placeholder data present), so BarChart isn't put into its full loading state.
    expect(barChart.props('loading')).toBe(false)

    wrapper.unmount()
    spy.mockRestore()
  })
})

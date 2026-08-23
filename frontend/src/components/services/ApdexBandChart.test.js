import { describe, it, expect, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import ApdexBandChart from './ApdexBandChart.vue'

// This chart renders through TanStack Charts (Task 21), loaded lazily so its ~34 kB gzipped chunk
// stays out of the initial bundle. Stub the lazy target and assert the mapped bucket props — the
// engine's own rendering is its business, the mapping is ours.
// (vi.mock factories are hoisted above top-level consts, so the stub must be defined via
// vi.hoisted — a bare `const StackedBandChart` referenced in the factory throws a TDZ error.)
const { StackedBandChart } = vi.hoisted(() => ({
  StackedBandChart: { props: ['buckets', 'markers'], template: '<div class="band" />' },
}))
vi.mock('@/components/charts/tanstack/lazy', () => ({ LazyStackedBandChart: StackedBandChart }))

async function mountChart(props) {
  const w = mount(ApdexBandChart, { props })
  // The stub is synchronous, but flushing keeps this identical whether or not the lazy wrapper is
  // the real `defineAsyncComponent`.
  await flushPromises()
  return w
}

describe('ApdexBandChart', () => {
  it('maps the three Apdex bands into stacked segments', async () => {
    // `ts` is an epoch-MILLISECOND number, the shape `/api/services/:service/timeseries` sends.
    const buckets = [{ ts: 1_700_000_000_000, satisfied: 8, tolerating: 1, frustrated: 1 }]
    const w = await mountChart({ buckets, startMs: 0, endMs: 1, loading: false })
    const passed = w.findComponent(StackedBandChart).props('buckets')
    expect(passed[0].segments.map((s) => s.key)).toEqual(['satisfied', 'tolerating', 'frustrated'])
    expect(passed[0].segments.map((s) => s.value)).toEqual([8, 1, 1])
    // Passed through, not divided: the chart plots milliseconds. Dividing by 1e6 (as this did back
    // when the route sent ns strings) put every bar near epoch 1970 and drew an empty chart.
    expect(passed[0].t).toBe(1_700_000_000_000)
  })

  it('forwards deploy markers to the chart', async () => {
    const markers = [{ xMs: 1_700_000_000_000, label: '1.4.2', color: 'hsl(var(--brand))' }]
    const w = await mountChart({ buckets: [], startMs: 0, endMs: 1, loading: false, markers })
    expect(w.findComponent(StackedBandChart).props('markers')).toEqual(markers)
  })
})

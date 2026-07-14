import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import ApdexBandChart from './ApdexBandChart.vue'

// BarChart is canvas/uPlot-backed; stub it and assert the mapped bucket props.
// (vi.mock factories are hoisted above top-level consts, so the stub must be defined via
// vi.hoisted — a bare `const BarChart` referenced in the factory throws a TDZ error.)
const { BarChart } = vi.hoisted(() => ({
  BarChart: { props: ['buckets', 'stacked'], template: '<div class="bar" />' },
}))
vi.mock('@/components/charts/BarChart.vue', () => ({ default: BarChart }))

describe('ApdexBandChart', () => {
  it('maps the three Apdex bands into stacked segments', () => {
    const buckets = [{ ts: '2000000', satisfied: 8, tolerating: 1, frustrated: 1 }]
    const w = mount(ApdexBandChart, { props: { buckets, startMs: 0, endMs: 1, loading: false } })
    const passed = w.findComponent(BarChart).props('buckets')
    expect(passed[0].segments.map((s) => s.key)).toEqual(['satisfied', 'tolerating', 'frustrated'])
    expect(passed[0].segments.map((s) => s.value)).toEqual([8, 1, 1])
    expect(passed[0].t).toBe(2)
  })
})

import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import ServiceVolumeChart from './ServiceVolumeChart.vue'

// BarChart is canvas/uPlot-backed; stub it and assert the mapped bucket props.
// (vi.mock factories are hoisted above top-level consts, so the stub must be defined via
// vi.hoisted — a bare `const BarChart` referenced in the factory throws a TDZ error.)
const { BarChart } = vi.hoisted(() => ({
  BarChart: { props: ['buckets', 'stacked'], template: '<div class="bar" />' },
}))
vi.mock('@/components/charts/BarChart.vue', () => ({ default: BarChart }))

describe('ServiceVolumeChart', () => {
  it('maps count/error_count into ok+error stacked segments', () => {
    const buckets = [{ ts: '1000000', count: 10, error_count: 3 }]
    const w = mount(ServiceVolumeChart, { props: { buckets, startMs: 0, endMs: 1, loading: false } })
    const passed = w.findComponent(BarChart).props('buckets')
    expect(passed[0].segments.map((s) => s.key)).toEqual(['ok', 'error'])
    expect(passed[0].segments[0].value).toBe(7) // ok = count - error_count
    expect(passed[0].segments[1].value).toBe(3) // error
    expect(passed[0].t).toBe(1) // ns → ms
  })
})

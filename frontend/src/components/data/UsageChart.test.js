// frontend/src/components/data/UsageChart.test.js
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import UsageChart from '@/components/data/UsageChart.vue'
import LineChart from '@/components/charts/LineChart.vue'

const SERIES = [
  { key: 'logs', points: [{ t: 0, v: 100 }, { t: 500, v: null }, { t: 1000, v: 300 }] },
  { key: 'traces', points: [{ t: 0, v: 40 }, { t: 500, v: 60 }, { t: 1000, v: 50 }] },
]

describe('UsageChart', () => {
  it('maps series to LineChart props with a label per key', () => {
    const w = mount(UsageChart, {
      props: { series: SERIES, startMs: 0, endMs: 1000 },
      global: { stubs: { LineChart: true } },
    })
    const line = w.findComponent(LineChart)
    expect(line.props('series')).toEqual([
      { key: 'logs', label: 'logs', points: SERIES[0].points },
      { key: 'traces', label: 'traces', points: SERIES[1].points },
    ])
  })

  it('forwards startMs/endMs straight through', () => {
    const w = mount(UsageChart, {
      props: { series: SERIES, startMs: 123, endMs: 456 },
      global: { stubs: { LineChart: true } },
    })
    const line = w.findComponent(LineChart)
    expect(line.props('startMs')).toBe(123)
    expect(line.props('endMs')).toBe(456)
  })

  it('forwards formatValue straight through', () => {
    const fmt = (n) => `${n}B`
    const w = mount(UsageChart, {
      props: { series: SERIES, startMs: 0, endMs: 1000, formatValue: fmt },
      global: { stubs: { LineChart: true } },
    })
    expect(w.findComponent(LineChart).props('formatValue')).toBe(fmt)
  })

  it('forwards area and loading straight through', () => {
    const w = mount(UsageChart, {
      props: { series: SERIES, startMs: 0, endMs: 1000, area: true, loading: true },
      global: { stubs: { LineChart: true } },
    })
    const line = w.findComponent(LineChart)
    expect(line.props('area')).toBe(true)
    expect(line.props('loading')).toBe(true)
  })

  it('defaults area and loading to false when unset', () => {
    const w = mount(UsageChart, {
      props: { series: SERIES, startMs: 0, endMs: 1000 },
      global: { stubs: { LineChart: true } },
    })
    const line = w.findComponent(LineChart)
    expect(line.props('area')).toBe(false)
    expect(line.props('loading')).toBe(false)
  })
})

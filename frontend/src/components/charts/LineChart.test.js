// Wrapper tests for LineChart: it curries buildLineOptions and translates BaseChart's generic
// events into the public line contract. uPlot never constructs in jsdom, so we exercise the
// builder it hands to BaseChart directly, and drive BaseChart's emits to assert translation.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import LineChart from './LineChart.vue'
import BaseChart from './BaseChart.vue'

// Stand-in for the loaded uPlot module — the builder only calls its path factories.
const fakeUplot = { paths: { spline: () => 'SPLINE', bars: () => 'BARS' } }

const SERIES = [
  { key: 'a', label: 'A', points: [{ t: 1000, v: 1 }, { t: 2000, v: 2 }] },
  { key: 'b', label: 'B', points: [{ t: 2000, v: 9 }] },
]

describe('LineChart', () => {
  it('curries buildLineOptions with area → gradient fill on the lead series only', () => {
    const w = mount(LineChart, { props: { series: SERIES, startMs: 1000, endMs: 2000, area: true } })
    const build = w.findComponent(BaseChart).props('buildOptions')
    const { opts } = build(fakeUplot, {})
    expect(typeof opts.series[1].fill).toBe('function') // lead series filled
    expect(opts.series[2].fill).toBeUndefined() // others unfilled (no mud)
  })

  it('omits fills when area=false', () => {
    const w = mount(LineChart, { props: { series: SERIES, startMs: 1000, endMs: 2000 } })
    const { opts } = w.findComponent(BaseChart).props('buildOptions')(fakeUplot, {})
    expect(opts.series[1].fill).toBeUndefined()
  })

  it('enables point dots at isolated single-point runs', () => {
    // series 'b' has a lone point at index 1 (null-flanked once aligned onto the shared x-axis).
    const w = mount(LineChart, { props: { series: SERIES, startMs: 1000, endMs: 2000 } })
    const { opts } = w.findComponent(BaseChart).props('buildOptions')(fakeUplot, {})
    expect(typeof opts.series[2].points.show).toBe('function')
  })

  it('derives legend items from series (label + identity colour)', () => {
    const w = mount(LineChart, { props: { series: SERIES, startMs: 1000, endMs: 2000 } })
    const items = w.findComponent(BaseChart).props('legendItems')
    expect(items.map((i) => i.key)).toEqual(['a', 'b'])
    expect(items[0].label).toBe('A')
  })

  it('translates a BaseChart select (seconds) into zoom (ms)', () => {
    const w = mount(LineChart, { props: { series: SERIES, startMs: 1000, endMs: 2000 } })
    w.findComponent(BaseChart).vm.$emit('select', { minX: 1, maxX: 2 })
    expect(w.emitted('zoom')[0]).toEqual([{ startMs: 1000, endMs: 2000 }])
  })

  it('passes legend-toggle through', () => {
    const w = mount(LineChart, { props: { series: SERIES, startMs: 1000, endMs: 2000 } })
    w.findComponent(BaseChart).vm.$emit('legend-toggle', { key: 'a', shown: false })
    expect(w.emitted('legend-toggle')[0]).toEqual([{ key: 'a', shown: false }])
  })

  it('forwards the stacked flag → builder emits cumulative bands + de-stacked tooltipData', () => {
    const w = mount(LineChart, { props: { series: SERIES, startMs: 1000, endMs: 2000, stacked: true, area: true } })
    const base = w.findComponent(BaseChart)
    const { data, opts } = base.props('buildOptions')(fakeUplot, {})
    // SERIES: a=[1,2], b aligned=[null,9]. Cumulative total = a+b, drawn total-first (reversed).
    expect(data[1]).toEqual([1, 11]) // total: a1+b(null→0) ; a2+b9
    expect(data[2]).toEqual([1, 2]) //  bottom band 'a'
    expect(typeof opts.series[1].fill).toBe('function') // every band filled when stacked && area
    expect(typeof opts.series[2].fill).toBe('function')
    // BaseChart receives the raw de-stacked values (same reversed order: b then a).
    const td = base.props('tooltipData')
    expect(td[0]).toEqual([null, 9]) // 'b' raw
    expect(td[1]).toEqual([1, 2]) //  'a' raw
  })

  it('reverses the legend order when stacked (chip ↔ top-down y-series)', () => {
    const w = mount(LineChart, { props: { series: SERIES, startMs: 1000, endMs: 2000, stacked: true } })
    const items = w.findComponent(BaseChart).props('legendItems')
    expect(items.map((i) => i.key)).toEqual(['b', 'a']) // total-signal band (drawn behind) first
  })

  it('converts band ms → seconds for the x-scale', () => {
    const w = mount(LineChart, {
      props: {
        series: SERIES,
        startMs: 1000,
        endMs: 2000,
        bands: [{ x0Ms: 1000, x1Ms: 2000, label: 'gap', color: '#f00' }],
      },
    })
    const bands = w.findComponent(BaseChart).props('bands')
    expect(bands[0]).toEqual({ x0: 1, x1: 2, label: 'gap', color: '#f00' })
  })
})

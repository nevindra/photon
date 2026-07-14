// Wrapper tests for BarChart: it curries buildBarOptions and translates BaseChart's generic
// select into either zoom (time axis) or brush (value/duration axis). uPlot never constructs in
// jsdom, so we inspect the builder BarChart hands to BaseChart and drive BaseChart's emits.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import BarChart from './BarChart.vue'
import BaseChart from './BaseChart.vue'

const fakeUplot = { paths: { spline: () => 'SPLINE', bars: () => 'BARS' } }

const BUCKETS = [
  { t: 1000, segments: [{ key: 'ok', color: '#0a0', value: 2 }, { key: 'error', color: '#f00', value: 3 }] },
  { t: 2000, segments: [{ key: 'ok', color: '#0a0', value: 4 }, { key: 'error', color: '#f00', value: 1 }] },
]

describe('BarChart', () => {
  it('stacks segments by default, drawn largest-first (cumulative tops, reversed order)', () => {
    const w = mount(BarChart, { props: { buckets: BUCKETS, startMs: 1000, endMs: 2000 } })
    const { data } = w.findComponent(BaseChart).props('buildOptions')(fakeUplot, {})
    // Reversed for stacking: y-series 1 = 'error' cumulative (the total, painted behind), 2 = 'ok'.
    expect(data[1]).toEqual([5, 5]) // 'error' cumulative (ok + error) — total
    expect(data[2]).toEqual([2, 4]) // 'ok' cumulative — drawn in front
  })

  it('forwards RAW de-stacked tooltipData for stacked bars (so the tooltip is not cumulative)', () => {
    const w = mount(BarChart, { props: { buckets: BUCKETS, startMs: 1000, endMs: 2000 } })
    const td = w.findComponent(BaseChart).props('tooltipData')
    // Aligned to the reversed y-series order: [error raw, ok raw].
    expect(td[0]).toEqual([3, 1]) // 'error' raw (data holds the cumulative tops instead)
    expect(td[1]).toEqual([2, 4]) // 'ok' raw
  })

  it('derives a legend only when there is more than one segment', () => {
    const multi = mount(BarChart, { props: { buckets: BUCKETS, startMs: 1000, endMs: 2000 } })
    // Reversed to match the stacked draw order (chip i ↔ built series i+1).
    expect(multi.findComponent(BaseChart).props('legendItems').map((i) => i.key)).toEqual(['error', 'ok'])

    const single = mount(BarChart, {
      props: { buckets: [{ t: 1000, segments: [{ key: 'count', color: '#888', value: 5 }] }], startMs: 1000, endMs: 2000 },
    })
    expect(single.findComponent(BaseChart).props('legendItems')).toEqual([])
  })

  it('time mode: a select translates to zoom (seconds → ms)', () => {
    const w = mount(BarChart, { props: { buckets: BUCKETS, startMs: 1000, endMs: 2000 } })
    w.findComponent(BaseChart).vm.$emit('select', { minX: 1, maxX: 2 })
    expect(w.emitted('zoom')[0]).toEqual([{ startMs: 1000, endMs: 2000 }])
    expect(w.emitted('brush')).toBeUndefined()
  })

  it('value mode: a select translates to brush (raw ns)', () => {
    const w = mount(BarChart, {
      props: { buckets: BUCKETS, startMs: 1000, endMs: 2000, xUnit: 'value' },
    })
    w.findComponent(BaseChart).vm.$emit('select', { minX: 1000, maxX: 5000 })
    expect(w.emitted('brush')[0]).toEqual([{ minNs: 1000, maxNs: 5000 }])
    expect(w.emitted('zoom')).toBeUndefined()
  })

  it('passes markers through, mapping x into the x-scale units (time → seconds)', () => {
    const w = mount(BarChart, {
      props: { buckets: BUCKETS, startMs: 1000, endMs: 2000, markers: [{ x: 2000, label: 'p99', color: '#f00' }] },
    })
    expect(w.findComponent(BaseChart).props('markers')[0]).toEqual({ x: 2, label: 'p99', color: '#f00' })
  })

  it('value mode markers keep raw x', () => {
    const w = mount(BarChart, {
      props: { buckets: BUCKETS, startMs: 1000, endMs: 2000, xUnit: 'value', markers: [{ x: 5000, label: 'p90', color: '#f00' }] },
    })
    expect(w.findComponent(BaseChart).props('markers')[0]).toEqual({ x: 5000, label: 'p90', color: '#f00' })
  })
})

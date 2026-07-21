// frontend/src/components/metrics/MetricChart.test.js
// MetricChart is now a thin adapter over charts/LineChart.vue — assert the prop mapping it does
// (ns→ms timestamps, labels→key/label, area/formatValue/highlightKey), not chart pixels.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricChart from './MetricChart.vue'
import LineChart from '@/components/charts/LineChart.vue'

const ONE_SERIES = [
  { labels: { service: 'checkout' }, points: [
    { t: '0', v: 10 }, { t: '1000000', v: null }, { t: '2000000', v: 30 }, { t: '3000000', v: 25 },
  ], exemplars: [] },
]

const TWO_SERIES = [
  ...ONE_SERIES,
  { labels: { service: 'cart' }, points: [
    { t: '0', v: 5 }, { t: '1000000', v: 8 }, { t: '2000000', v: 6 }, { t: '3000000', v: 9 },
  ], exemplars: [] },
]

function mountChart(props) {
  return mount(MetricChart, { props, global: { stubs: { LineChart: true } } })
}

describe('MetricChart', () => {
  it('maps series: ns-string timestamps → ms Numbers, labels → key/label', () => {
    const w = mountChart({ series: ONE_SERIES, unit: 'ms', startMs: 0, endMs: 3 })
    const series = w.findComponent(LineChart).props('series')
    expect(series).toEqual([
      {
        key: 'service=checkout',
        label: 'service=checkout',
        color: expect.any(String),
        points: [
          { t: 0, v: 10 },
          { t: 1, v: null }, // 1_000_000ns → 1ms; null preserved for gap breaks
          { t: 2, v: 30 },
          { t: 3, v: 25 },
        ],
      },
    ])
  })

  it('sets area=true for a single series, false for multiple', () => {
    const single = mountChart({ series: ONE_SERIES, unit: 'ms', startMs: 0, endMs: 3 })
    expect(single.findComponent(LineChart).props('area')).toBe(true)

    const multi = mountChart({ series: TWO_SERIES, unit: 'ms', startMs: 0, endMs: 3 })
    expect(multi.findComponent(LineChart).props('area')).toBe(false)
  })

  it('builds a formatValue that appends the unit', () => {
    const w = mountChart({ series: ONE_SERIES, unit: 'ms', startMs: 0, endMs: 3 })
    const formatValue = w.findComponent(LineChart).props('formatValue')
    expect(formatValue(1234)).toBe('1,234 ms')
  })

  it('omits the unit suffix when unit is empty or "1"', () => {
    const w = mountChart({ series: ONE_SERIES, unit: '1', startMs: 0, endMs: 3 })
    expect(w.findComponent(LineChart).props('formatValue')(42)).toBe('42')
  })

  it('forwards highlightKey, startMs, endMs, loading', () => {
    const w = mountChart({
      series: ONE_SERIES, unit: 'ms', startMs: 0, endMs: 3, highlightKey: 'service=checkout', loading: true,
    })
    const lineChart = w.findComponent(LineChart)
    expect(lineChart.props('highlightKey')).toBe('service=checkout')
    expect(lineChart.props('startMs')).toBe(0)
    expect(lineChart.props('endMs')).toBe(3)
    expect(lineChart.props('loading')).toBe(true)
  })

  it('re-emits exemplar from LineChart', () => {
    const w = mountChart({ series: ONE_SERIES, unit: 'ms', startMs: 0, endMs: 3 })
    w.findComponent(LineChart).vm.$emit('exemplar', { traceId: 'abc', t: 123 })
    expect(w.emitted('exemplar')[0]).toEqual([{ traceId: 'abc', t: 123 }])
  })

  it('percent scales 0-1 fractions to 0-100, nulls pass through unscaled', () => {
    const fractional = [
      { labels: { service: 'checkout' }, points: [{ t: '0', v: 0.4 }, { t: '1000000', v: null }] },
    ]
    const w = mountChart({ series: fractional, percent: true, startMs: 0, endMs: 3 })
    expect(w.findComponent(LineChart).props('series')[0].points).toEqual([
      { t: 0, v: 40 },
      { t: 1, v: null },
    ])
  })

  it('forwards yRange verbatim to LineChart; percent alone leaves it null', () => {
    const frac = [{ labels: {}, points: [{ t: '0', v: 0.4 }] }]
    // percent does the ×100 transform but does NOT pin the axis on its own
    const auto = mountChart({ series: frac, percent: true, startMs: 0, endMs: 3 })
    expect(auto.findComponent(LineChart).props('yRange')).toBeNull()
    // an explicit yRange is forwarded through untouched
    const pinned = mountChart({ series: frac, percent: true, yRange: [0, 100], startMs: 0, endMs: 3 })
    expect(pinned.findComponent(LineChart).props('yRange')).toEqual([0, 100])
  })

  // Explorer-safety guarantee: MetricsExplorer forwards arbitrary OTLP `unit` (incl. "%") into
  // MetricChart. `unit` is now a PURE LABEL — a metric already in 0–100 with unit "%" must not be
  // double-scaled onto a pinned axis. Only the `percent` prop transforms; only `yRange` pins.
  it('unit="%" alone does NOT transform values and does NOT pin the range', () => {
    const already0to100 = [
      { labels: { service: 'checkout' }, points: [{ t: '0', v: 42 }, { t: '1000000', v: null }] },
    ]
    const w = mountChart({ series: already0to100, unit: '%', startMs: 0, endMs: 3 })
    const lineChart = w.findComponent(LineChart)
    expect(lineChart.props('series')[0].points).toEqual([
      { t: 0, v: 42 }, // NOT ×100 — no double-scale
      { t: 1, v: null },
    ])
    expect(lineChart.props('yRange')).toBeNull() // NOT pinned to [0,100]
  })

  it('leaves non-percent units unscaled with a null yRange', () => {
    const w = mountChart({ series: ONE_SERIES, unit: 'ms', startMs: 0, endMs: 3 })
    const lineChart = w.findComponent(LineChart)
    expect(lineChart.props('series')[0].points[0]).toEqual({ t: 0, v: 10 })
    expect(lineChart.props('yRange')).toBeNull()
  })

  it('formatValue renders percent with one decimal under 10, rounded otherwise', () => {
    const w = mountChart({ series: ONE_SERIES, percent: true, startMs: 0, endMs: 3 })
    const formatValue = w.findComponent(LineChart).props('formatValue')
    expect(formatValue(4.567)).toBe('4.6%')
    expect(formatValue(0)).toBe('0%')
    expect(formatValue(42.9)).toBe('43%')
  })

  it('formatValue renders "By/s" via formatRate', () => {
    const w = mountChart({ series: ONE_SERIES, unit: 'By/s', startMs: 0, endMs: 3 })
    const formatValue = w.findComponent(LineChart).props('formatValue')
    expect(formatValue(2_150_000)).toBe('2.1 MB/s')
  })
})

const series = [{ labels: { svc: 'a' }, points: [{ t: '1000000', v: 5 }] }]
const base = { series, startMs: 0, endMs: 10, unit: 'ms' }

describe('MetricChart viz routing', () => {
  it('renders the stat panel for viz=stat', () => {
    const w = mount(MetricChart, { props: { ...base, viz: 'stat' } })
    expect(w.find('[data-testid="metric-stat"]').exists()).toBe(true)
  })
  it('renders the reused legend table for viz=table', () => {
    const w = mount(MetricChart, { props: { ...base, viz: 'table' } })
    expect(w.find('[data-testid="legend-row"]').exists()).toBe(true)
  })
  it('re-emits point-click as tMs (seconds→ms) from the underlying chart', async () => {
    const w = mount(MetricChart, { props: { ...base, viz: 'line' } })
    w.vm.onPointClick({ x: 1.5 }) // x is seconds on a time axis
    expect(w.emitted('point-click')?.[0]?.[0]).toEqual({ tMs: 1500 })
  })
})

// Wrapper DOM tests for the shared chart shell. uPlot no-ops in jsdom (no canvas 2D context — see
// useUplot), so we assert the Vue shell + event translation, NOT canvas pixels: empty/loading
// states, legend rendering + toggle, marker/band/refline overlays, and the select→emit path
// (driven through the exposed hook with a stubbed uPlot, since there is no real cursor).
import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import BaseChart from './BaseChart.vue'

// A builder with finite data (no empty state); opts shape is irrelevant here — uPlot never builds.
const withData = () => ({ opts: { series: [{}, {}] }, data: [[1, 2], [10, 20]] })
// A builder whose series arrays hold no finite values → the empty state.
const noData = () => ({ opts: { series: [{}] }, data: [[]] })

const LEGEND = [
  { key: 'a', label: 'Alpha', color: '#0ea5e9' },
  { key: 'b', label: 'Beta', color: '#f43f5e' },
]

describe('BaseChart', () => {
  it('shows the loading state', () => {
    const w = mount(BaseChart, { props: { buildOptions: noData, loading: true } })
    expect(w.get('[data-testid="chart-loading"]').text()).toContain('Loading')
    expect(w.find('[data-testid="chart-empty"]').exists()).toBe(false)
  })

  it('shows the empty state when there is no finite data and not loading', () => {
    const w = mount(BaseChart, { props: { buildOptions: noData } })
    expect(w.get('[data-testid="chart-empty"]').text()).toContain('No data')
  })

  it('does not show empty when the builder yields finite data', () => {
    const w = mount(BaseChart, { props: { buildOptions: withData } })
    expect(w.find('[data-testid="chart-empty"]').exists()).toBe(false)
  })

  it('hides the legend row for a single series (a lone chip is redundant noise)', () => {
    const w = mount(BaseChart, { props: { buildOptions: withData, legendItems: [LEGEND[0]] } })
    expect(w.find('[data-testid="chart-legend"]').exists()).toBe(false)
  })

  it('renders a legend chip per item and toggles + emits on click', async () => {
    const w = mount(BaseChart, { props: { buildOptions: withData, legendItems: LEGEND } })
    const chips = w.findAll('[data-testid="chart-legend-item"]')
    expect(chips).toHaveLength(2)
    expect(chips[0].text()).toContain('Alpha')

    await chips[0].trigger('click')
    // dimmed + struck through when off
    expect(chips[0].classes()).toContain('line-through')
    expect(w.emitted('legend-toggle')[0]).toEqual([{ key: 'a', shown: false }])

    // clicking again turns it back on
    await chips[0].trigger('click')
    expect(w.emitted('legend-toggle')[1]).toEqual([{ key: 'a', shown: true }])
    expect(chips[0].classes()).not.toContain('line-through')
  })

  it('renders marker / band / refline overlays with their labels', () => {
    const w = mount(BaseChart, {
      props: {
        buildOptions: withData,
        markers: [{ x: 5, label: 'p99', color: '#f00' }],
        bands: [{ x0: 1, x1: 2, label: 'outage', color: '#f00' }],
        refLines: [{ y: 42, label: 'avg', color: '#0a0' }],
      },
    })
    expect(w.get('[data-testid="chart-marker"]').text()).toContain('p99')
    expect(w.get('[data-testid="chart-band"]').text()).toContain('outage')
    expect(w.get('[data-testid="chart-refline"]').text()).toContain('avg')
  })

  it('emits select {minX,maxX} from the setSelect hook (px→x-value)', () => {
    const w = mount(BaseChart, { props: { buildOptions: withData } })
    const setSelect = vi.fn()
    // Stubbed uPlot: a 10px→1-unit scale, a 100px-wide drag starting at 10px → [1, 11].
    w.vm.onSetSelect({ select: { left: 10, width: 100 }, posToVal: (px) => px / 10, setSelect })
    expect(w.emitted('select')[0]).toEqual([{ minX: 1, maxX: 11 }])
    expect(setSelect).toHaveBeenCalled() // selection cleared after emit
  })

  it('ignores a zero-width select (a click, not a brush)', () => {
    const w = mount(BaseChart, { props: { buildOptions: withData } })
    w.vm.onSetSelect({ select: { left: 10, width: 0 }, posToVal: (px) => px / 10 })
    expect(w.emitted('select')).toBeUndefined()
  })

  it('builds a tooltip from the cursor hook, sorted desc by value', async () => {
    const w = mount(BaseChart, { props: { buildOptions: withData, legendItems: LEGEND } })
    // Stubbed uPlot instance at data index 1 (values 20 and 5 across the two series).
    w.vm.onSetCursor({
      cursor: { idx: 1, left: 50, top: 20 },
      data: [[1, 2], [10, 20], [7, 5]],
      scales: { x: { time: true } },
      series: [{}, { label: 'Alpha' }, { label: 'Beta' }],
    })
    await w.vm.$nextTick()
    const tip = w.get('[data-testid="chart-tooltip"]')
    expect(tip.text()).toContain('Alpha')
    expect(tip.text()).toContain('Beta')
    // Alpha (20) sorts above Beta (5)
    expect(tip.text().indexOf('Alpha')).toBeLessThan(tip.text().indexOf('Beta'))
  })

  it('de-stacks the tooltip: reads RAW values from tooltipData, not the cumulative tops in u.data', async () => {
    // Regression for the stacked-chart bug: `data` holds cumulative baselines (ok=2, error top=5)
    // so bars/areas draw correctly, but the tooltip must show each segment's OWN value (ok=2,
    // error=3). A distinctive formatValue isolates the row value from the clock-label header.
    const w = mount(BaseChart, {
      props: {
        buildOptions: withData,
        formatValue: (v) => `[${v}]`,
        legendItems: [
          { key: 'ok', label: 'ok', color: '#0a0' },
          { key: 'error', label: 'error', color: '#f00' },
        ],
        tooltipData: [[2], [3]], // RAW: ok=2, error=3
      },
    })
    w.vm.onSetCursor({
      cursor: { idx: 0, left: 50, top: 20 },
      data: [[100], [2], [5]], // CUMULATIVE: ok=2, error top=5
      scales: { x: { time: true } },
      series: [{}, { label: 'ok' }, { label: 'error' }],
    })
    await w.vm.$nextTick()
    const tip = w.get('[data-testid="chart-tooltip"]')
    expect(tip.text()).toContain('[3]') // 'error' shows its RAW value …
    expect(tip.text()).not.toContain('[5]') // … not the cumulative top 5
    expect(tip.text()).toContain('[2]') // 'ok' (raw == cumulative here)
  })

  it('emits point-click with the x value for a zero-width (click) selection', () => {
    const w = mount(BaseChart, { props: { buildOptions: withData } })
    // Drive the exposed hook with a stub uPlot instance (uPlot no-ops in jsdom).
    const u = { select: { width: 0 }, cursor: { left: 20 }, posToVal: () => 1700, setSelect() {} }
    w.vm.onSetSelect(u)
    expect(w.emitted('point-click')?.[0]?.[0]).toEqual({ x: 1700 })
  })
})

import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MiniAreaChart from './MiniAreaChart.vue'

// MiniAreaChart wraps LineChart (compact) → BaseChart. uPlot no-ops in jsdom, so the assertable DOM
// is BaseChart's container + its empty state. These smoke-test the number[] / {t,v}[] normalization,
// the "no data" path, and that the compact height is forwarded through to the BaseChart container.
describe('MiniAreaChart', () => {
  it('shows BaseChart\'s empty state when there are no points', () => {
    const w = mount(MiniAreaChart, { props: { points: [] } })
    expect(w.find('[data-testid="chart-empty"]').exists()).toBe(true)
  })

  it('renders the chart (no empty state) with finite numeric points', () => {
    const w = mount(MiniAreaChart, { props: { points: [1, 2, 3, 4] } })
    expect(w.find('[data-testid="chart-empty"]').exists()).toBe(false)
  })

  it('accepts {t,v} point objects', () => {
    const w = mount(MiniAreaChart, {
      props: { points: [{ t: 0, v: 5 }, { t: 1, v: null }, { t: 2, v: 9 }] },
    })
    expect(w.find('[data-testid="chart-empty"]').exists()).toBe(false)
  })

  it('forwards the compact height to the chart container', () => {
    const w = mount(MiniAreaChart, { props: { points: [1, 2], height: 60 } })
    expect(w.get('div').attributes('style')).toContain('height: 60px')
  })
})

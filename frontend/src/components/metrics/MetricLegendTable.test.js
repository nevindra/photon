import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricLegendTable from './MetricLegendTable.vue'

const SERIES = [
  { labels: { service: 'checkout' }, points: [{ t: '0', v: 10 }, { t: '1', v: null }, { t: '2', v: 30 }] },
  { labels: { service: 'cart' }, points: [{ t: '0', v: 5 }, { t: '1', v: 6 }, { t: '2', v: 7 }] },
]

describe('MetricLegendTable', () => {
  it('computes Last/Min/Avg/Max per series and flags the worst Max red', () => {
    const w = mount(MetricLegendTable, { props: { series: SERIES, unit: 'ms' } })
    const rows = w.findAll('[data-testid="legend-row"]')
    expect(rows).toHaveLength(2)
    // checkout: last=30 min=10 max=30 ; it has the largest Max → red badge present
    expect(w.find('[data-testid="legend-max-worst"]').exists()).toBe(true)
    expect(w.text()).toContain('service = checkout')
  })
  it('emits highlight on row hover', async () => {
    const w = mount(MetricLegendTable, { props: { series: SERIES, unit: 'ms' } })
    const row = w.findAll('[data-testid="legend-row"]')[0]
    await row.trigger('mouseenter')
    expect(w.emitted('highlight')[0][0]).toContain('service=checkout')
    await row.trigger('mouseleave')
    expect(w.emitted('highlight').at(-1)).toEqual([null])
  })
  it('sorts by a numeric column when its header is clicked', async () => {
    const w = mount(MetricLegendTable, { props: { series: SERIES, unit: 'ms' } })
    await w.get('[data-testid="legend-sort-max"]').trigger('click')
    const first = w.findAll('[data-testid="legend-row"]')[0]
    expect(first.text()).toContain('checkout') // desc by max → checkout(30) first
  })
})

import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricStat from './MetricStat.vue'

const series = [{ labels: {}, points: [{ t: 1, v: 10 }, { t: 2, v: 20 }, { t: 3, v: 30 }] }]

describe('MetricStat', () => {
  it('shows the hero value and an up-delta', () => {
    const w = mount(MetricStat, { props: { series, unit: 'ms' } })
    expect(w.find('[data-testid="stat-hero"]').text()).toContain('30')
    expect(w.find('[data-testid="stat-delta"]').text()).toMatch(/%/)
  })
  it('shows an empty dash when there is no data', () => {
    const w = mount(MetricStat, { props: { series: [], unit: '' } })
    expect(w.find('[data-testid="stat-hero"]').text()).toContain('—')
  })
})

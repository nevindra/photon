import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricPresets from './MetricPresets.vue'

describe('MetricPresets', () => {
  it('shows the preset chips for a histogram and emits apply on click', async () => {
    const w = mount(MetricPresets, { props: { metricType: 'histogram', isMonotonic: null, currentAgg: null } })
    const chips = w.findAll('[data-testid="preset-chip"]')
    expect(chips.length).toBe(4) // p99, p90, p50, count
    await chips[0].trigger('click')
    expect(w.emitted('apply')?.[0]?.[0]).toEqual({ agg: 'p99' })
  })
  it('renders nothing for an unknown type', () => {
    const w = mount(MetricPresets, { props: { metricType: '', isMonotonic: null, currentAgg: null } })
    expect(w.findAll('[data-testid="preset-chip"]').length).toBe(0)
  })
})

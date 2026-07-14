import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ServiceHealthCounts from './ServiceHealthCounts.vue'

describe('ServiceHealthCounts', () => {
  it('shows a count per status', () => {
    const rows = [
      { count: 5, error_rate: 0.06, apdex: 0.9 },
      { count: 5, error_rate: 0, apdex: 0.99 },
      { count: 5, error_rate: 0, apdex: 0.99 },
    ]
    const w = mount(ServiceHealthCounts, { props: { rows } })
    expect(w.text()).toContain('Critical')
    expect(w.text()).toContain('Healthy')
    // 2 healthy, 1 critical
    expect(w.text()).toMatch(/2\s*Healthy/)
  })
})

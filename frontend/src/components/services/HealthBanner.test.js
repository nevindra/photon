import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HealthBanner from './HealthBanner.vue'

describe('HealthBanner', () => {
  it('shows the status and joined reasons', () => {
    const w = mount(HealthBanner, { props: { status: 'critical', reasons: ['Error rate 8.2%', 'p99 1.2s ▲40%'] } })
    expect(w.text()).toContain('CRITICAL')
    expect(w.text()).toContain('Error rate 8.2% · p99 1.2s ▲40%')
  })
  it('shows a calm line when there are no reasons', () => {
    const w = mount(HealthBanner, { props: { status: 'healthy', reasons: [] } })
    expect(w.text()).toContain('No issues')
  })
})

import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HealthBanner from './HealthBanner.vue'

describe('HealthBanner', () => {
  it('shows the level and joined reasons', () => {
    const w = mount(HealthBanner, { props: { level: 'critical', reasons: ['Error rate 8.2%', 'p99 1.2s ▲40%'] } })
    expect(w.text()).toContain('CRITICAL')
    expect(w.text()).toContain('Error rate 8.2% · p99 1.2s ▲40%')
  })
  it('shows a calm line only when it actually looked and found nothing', () => {
    const w = mount(HealthBanner, { props: { level: 'ok', reasons: [] } })
    expect(w.text()).toContain('No issues')
  })
  it('never claims "no issues" for a level that saw no data', () => {
    const w = mount(HealthBanner, { props: { level: 'unknown', reasons: [] } })
    expect(w.text()).toContain('UNKNOWN')
    expect(w.text()).toContain('No data')
    expect(w.text()).not.toContain('No issues')
  })
})

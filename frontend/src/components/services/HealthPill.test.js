import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HealthPill from './HealthPill.vue'

describe('HealthPill', () => {
  it('renders the label and status for a status', () => {
    const w = mount(HealthPill, { props: { status: 'critical' } })
    expect(w.text()).toContain('Critical')
    expect(w.attributes('data-status')).toBe('critical')
  })
  it('hides the label when show-label is false', () => {
    const w = mount(HealthPill, { props: { status: 'healthy', showLabel: false } })
    expect(w.text()).not.toContain('Healthy')
  })
})

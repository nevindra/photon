import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HealthPill from './HealthPill.vue'

describe('HealthPill', () => {
  it('renders the label and the level for a level', () => {
    const w = mount(HealthPill, { props: { level: 'critical' } })
    expect(w.text()).toContain('Critical')
    expect(w.attributes('data-level')).toBe('critical')
  })
  it('renders Down, the one solid pill on the page', () => {
    const w = mount(HealthPill, { props: { level: 'down' } })
    expect(w.text()).toContain('Down')
    // Solid = the full-strength tone with reversed text, not the soft tint Critical gets.
    expect(w.classes().join(' ')).toContain('bg-sev-error')
    expect(w.classes().join(' ')).not.toContain('bg-sev-error-soft')
  })
  it('collapses to a bare dot when show-label is false', () => {
    const w = mount(HealthPill, { props: { level: 'ok', showLabel: false } })
    expect(w.text()).not.toContain('OK')
    expect(w.attributes('data-level')).toBe('ok')
  })
  it('falls back to Unknown for a level it does not know', () => {
    const w = mount(HealthPill, { props: { level: 'idle' } })
    expect(w.text()).toContain('Unknown')
  })
})

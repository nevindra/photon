import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ApdexBadge from './ApdexBadge.vue'

const band = (v) => mount(ApdexBadge, { props: { value: v } }).attributes('data-band')

describe('ApdexBadge', () => {
  it('bands score by thresholds', () => {
    expect(band(0.99)).toBe('good') // >= 0.94
    expect(band(0.90)).toBe('warn') // 0.85..0.94
    expect(band(0.50)).toBe('bad') // < 0.85
  })
  it('renders a dash for null', () => {
    expect(mount(ApdexBadge, { props: { value: null } }).text()).toContain('—')
  })
})

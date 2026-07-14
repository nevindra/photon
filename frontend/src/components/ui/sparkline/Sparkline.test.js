import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import Sparkline from './Sparkline.vue'

describe('Sparkline', () => {
  it('draws a path for a series of ≥2 points', () => {
    const w = mount(Sparkline, { props: { points: [1, 2, 3, 2, 5] } })
    expect(w.find('svg').exists()).toBe(true)
    expect(w.find('path').attributes('d')).toMatch(/^M/)
  })
  it('renders a placeholder for too-few points', () => {
    const w = mount(Sparkline, { props: { points: [1] } })
    expect(w.find('svg').exists()).toBe(false)
    expect(w.text()).toContain('—')
  })
})

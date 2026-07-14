import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { Meter } from './index.js'

describe('Meter', () => {
  it('fills to the given fraction as a percentage width', () => {
    const w = mount(Meter, { props: { value: 0.42 } })
    expect(w.get('[role="meter"] > div').attributes('style')).toContain('width: 42%')
  })

  it('clamps values above 1 and below 0', () => {
    const hi = mount(Meter, { props: { value: 2 } })
    expect(hi.get('[role="meter"] > div').attributes('style')).toContain('width: 100%')
    const lo = mount(Meter, { props: { value: -1 } })
    expect(lo.get('[role="meter"] > div').attributes('style')).toContain('width: 0%')
  })

  it('applies the tone class to the fill', () => {
    const w = mount(Meter, { props: { value: 0.5, tone: 'error' } })
    expect(w.get('[role="meter"] > div').classes()).toContain('bg-sev-error')
  })

  it('defaults a non-finite value to 0%', () => {
    const w = mount(Meter, { props: { value: Number.NaN } })
    expect(w.get('[role="meter"] > div').attributes('style')).toContain('width: 0%')
  })
})

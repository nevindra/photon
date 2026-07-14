import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import StatePill from '@/components/uptime/StatePill.vue'

describe('StatePill', () => {
  it('shows Up / Down / Paused labels', () => {
    expect(mount(StatePill, { props: { state: 'up' } }).text()).toMatch(/up/i)
    expect(mount(StatePill, { props: { state: 'down' } }).text()).toMatch(/down/i)
    expect(mount(StatePill, { props: { state: 'up', paused: true } }).text()).toMatch(/paused/i)
  })
})

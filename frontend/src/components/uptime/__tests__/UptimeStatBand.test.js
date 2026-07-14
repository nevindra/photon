import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import UptimeStatBand from '@/components/uptime/UptimeStatBand.vue'

const monitors = [
  { id: '1', enabled: true, last_state: 'up' },
  { id: '2', enabled: true, last_state: 'up' },
  { id: '3', enabled: true, last_state: 'down' },
  { id: '4', enabled: false, last_state: 'up' },   // paused
  { id: '5', enabled: true, last_state: 'pending' },
]

describe('UptimeStatBand', () => {
  it('counts total / up / down / paused', () => {
    const w = mount(UptimeStatBand, { props: { monitors } })
    const text = w.text()
    expect(text).toContain('Monitors')
    expect(text).toContain('Up')
    expect(text).toContain('Down')
    expect(text).toContain('Paused')
    // 5 total, 2 up, 1 down, 1 paused
    expect(w.findAll('.text-2xl').map((n) => n.text())).toEqual(['5', '2', '1', '1'])
  })

  it('renders zero counts for an empty list', () => {
    const w = mount(UptimeStatBand, { props: { monitors: [] } })
    expect(w.findAll('.text-2xl').map((n) => n.text())).toEqual(['0', '0', '0', '0'])
  })
})

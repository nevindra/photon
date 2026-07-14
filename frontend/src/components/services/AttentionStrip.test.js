import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import AttentionStrip from './AttentionStrip.vue'

const rows = [
  { service: 'ok', count: 100, error_rate: 0, apdex: 0.99 },
  { service: 'crit', count: 100, error_rate: 0.2, apdex: 0.5 },
  { service: 'degr', count: 100, error_rate: 0.02, apdex: 0.9 },
]

const stubs = { AttentionCard: { props: ['row'], template: '<div class="card" :data-service="row.service" />' } }

describe('AttentionStrip', () => {
  it('renders a card per non-healthy service, worst-first', () => {
    const w = mount(AttentionStrip, { props: { rows, startNs: '0', endNs: '1' }, global: { stubs } })
    const order = w.findAll('.card').map((c) => c.attributes('data-service'))
    expect(order).toEqual(['crit', 'degr'])
  })
  it('renders nothing when all healthy', () => {
    const healthy = [{ service: 'ok', count: 100, error_rate: 0, apdex: 0.99 }]
    const w = mount(AttentionStrip, { props: { rows: healthy, startNs: '0', endNs: '1' }, global: { stubs } })
    expect(w.findAll('.card').length).toBe(0)
  })
})

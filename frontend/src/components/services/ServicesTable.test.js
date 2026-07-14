import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ServicesTable from './ServicesTable.vue'

const rows = [
  { service: 'ok', count: 900, rate: 110, error_count: 1, error_rate: 0.001, p50: '1', p90: '2', p99: '3', apdex: 0.98 },
  { service: 'crit', count: 240, rate: 30, error_count: 20, error_rate: 0.083, p50: '1', p90: '2', p99: '3', apdex: 0.71 },
]

describe('ServicesTable', () => {
  it('sorts worst-first by health by default (critical row first)', () => {
    const w = mount(ServicesTable, { props: { rows } })
    const order = w.findAll('[data-testid="service-row"]').map((r) => r.attributes('data-service'))
    expect(order[0]).toBe('crit')
  })
  it('renders a Requests count and a health pill', () => {
    const w = mount(ServicesTable, { props: { rows } })
    expect(w.text()).toContain('Critical')
    expect(w.text()).toContain('240') // crit requests
  })
  it('shows an error-rate trend chip when prev-rows are supplied', () => {
    const prevRows = [{ service: 'crit', error_rate: 0.02 }]
    const w = mount(ServicesTable, { props: { rows, prevRows } })
    expect(w.find('[data-testid="err-trend"]').exists()).toBe(true)
  })
})

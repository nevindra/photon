import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import DependencyTable from './DependencyTable.vue'

const rows = [
  { name: 'postgres', system: 'orders', count: 4500, rate: 45, error_count: 270, error_rate: 0.06, p50: '1', p95: '2', p99: '3' },
]

describe('DependencyTable', () => {
  it('renders count and system', () => {
    const w = mount(DependencyTable, { props: { title: 'Database calls', rows } })
    expect(w.text()).toContain('4,500')
    expect(w.text()).toContain('orders')
  })
  it('emits open-traces with the row on click', async () => {
    const w = mount(DependencyTable, { props: { title: 'Database calls', rows } })
    await w.find('[data-testid="dependency-row"]').trigger('click')
    expect(w.emitted('open-traces')[0][0]).toMatchObject({ name: 'postgres' })
  })
})

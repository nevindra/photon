import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import RumBreakdownTable from './RumBreakdownTable.vue'
import { EmptyState } from '@/components/ui/empty-state'

// Already sorted busiest-first, matching the default sort (pageviews desc).
const rows = [
  { key: '/checkout', pageviews: 142000, lcp_p75: 4300, inp_p75: 210, cls_p75: 0.09 },
  { key: '/home', pageviews: 90000, lcp_p75: 1800, inp_p75: 120, cls_p75: 0.02 },
]

describe('RumBreakdownTable', () => {
  it('renders rows in default (pageviews desc) order', () => {
    const w = mount(RumBreakdownTable, { props: { rows, keyLabel: 'Route' } })
    const order = w.findAll('[data-testid="rum-breakdown-row"]').map((r) => r.attributes('data-key'))
    expect(order).toEqual(['/checkout', '/home'])
  })

  it('uses the keyLabel prop as the first column header', () => {
    const w = mount(RumBreakdownTable, { props: { rows, keyLabel: 'Route' } })
    expect(w.text()).toContain('Route')
  })

  it('re-sorts ascending by a column on click', async () => {
    const w = mount(RumBreakdownTable, { props: { rows, keyLabel: 'Route' } })
    await w.get('[data-testid="sort-pageviews"]').trigger('click') // desc -> asc
    const order = w.findAll('[data-testid="rum-breakdown-row"]').map((r) => r.attributes('data-key'))
    expect(order).toEqual(['/home', '/checkout'])
  })

  it('colors a poor LCP p75 cell with the poor rating', () => {
    const w = mount(RumBreakdownTable, { props: { rows, keyLabel: 'Route' } })
    const poorCell = w.get('[data-key="/checkout"] [data-rating="poor"]')
    expect(poorCell.classes()).toContain('text-sev-error')
    expect(poorCell.text()).toBe('4.3s')
  })

  it('colors a good LCP p75 cell with the good rating', () => {
    const w = mount(RumBreakdownTable, { props: { rows, keyLabel: 'Route' } })
    const goodCell = w.get('[data-key="/home"] [data-rating="good"]')
    expect(goodCell.classes()).toContain('text-success')
  })

  it('shows an empty state when there are no rows', () => {
    const w = mount(RumBreakdownTable, { props: { rows: [], keyLabel: 'Route' } })
    expect(w.findComponent(EmptyState).exists()).toBe(true)
    expect(w.find('[data-testid="rum-breakdown-row"]').exists()).toBe(false)
  })
})

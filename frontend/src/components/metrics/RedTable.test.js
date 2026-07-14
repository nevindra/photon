import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import RedTable from './RedTable.vue'

const rows = [
  { service: 'checkout', operation: 'charge.card', count: 1820, rate: 30.3, error_count: 34, error_rate: 0.0187, p50: '1500000', p90: '4200000', p99: '9100000' },
  { service: 'web', operation: 'GET /home', count: 6100, rate: 101.6, error_count: 5, error_rate: 0.00082, p50: '120000', p90: '400000', p99: '1100000' },
]

describe('RedTable', () => {
  it('renders one row per entry, defaulting to worst-error-rate first', () => {
    const w = mount(RedTable, { props: { rows, group: 'operation', loading: false } })
    const trs = w.findAll('[data-testid="red-row"]')
    expect(trs.length).toBe(2)
    // default sort = error_rate desc → checkout (1.87%) before web (0.08%)
    expect(trs[0].attributes('data-service')).toBe('checkout')
  })

  it('hides the operation column in service group', () => {
    const w = mount(RedTable, {
      props: { rows: rows.map((r) => ({ ...r, operation: null })), group: 'service', loading: false },
    })
    expect(w.find('[data-testid="col-operation"]').exists()).toBe(false)
  })

  it('emits open-exemplars with service + operation on row click', async () => {
    const w = mount(RedTable, { props: { rows, group: 'operation', loading: false } })
    await w.findAll('[data-testid="red-row"]')[0].trigger('click')
    expect(w.emitted('open-exemplars')[0][0]).toEqual({ service: 'checkout', operation: 'charge.card' })
  })

  it('re-sorts by a numeric column when its header is clicked', async () => {
    const w = mount(RedTable, { props: { rows, group: 'operation', loading: false } })
    await w.find('[data-testid="sort-rate"]').trigger('click') // rate desc → web (101.6) first
    const trs = w.findAll('[data-testid="red-row"]')
    expect(trs[0].attributes('data-service')).toBe('web')
  })

  it('renders a health dot per row (error tone when error rate is high)', () => {
    const hot = [{ service: 'pay', operation: 'charge', count: 100, rate: 5, error_count: 20, error_rate: 0.2, p50: '1', p90: '2', p99: '3' }]
    const w = mount(RedTable, { props: { rows: hot, group: 'operation', loading: false } })
    const row = w.get('[data-testid="red-row"]')
    // StatusDot renders a span with the error tone class when error_rate >= 0.05.
    expect(row.html()).toContain('bg-sev-error')
  })

  it('renders an inline rate meter per row', () => {
    const w = mount(RedTable, { props: { rows, group: 'operation', loading: false } })
    expect(w.findAll('[role="meter"]').length).toBe(2)
  })

  it('does not trap the sticky header in an inner scroll container', () => {
    // The table lives inside the view's page-level scroller; the ui/table wrapper must be
    // overflow-visible (not the default overflow-auto) so the sticky <thead> sticks to the page,
    // not to a zero-scroll inner wrapper.
    const w = mount(RedTable, { props: { rows, group: 'operation', loading: false } })
    const wrapper = w.get('table').element.parentElement
    expect(wrapper.className).toContain('overflow-visible')
    expect(wrapper.className).not.toContain('overflow-auto')
  })
})

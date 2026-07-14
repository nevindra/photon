import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import FacetFieldGroup from './FacetFieldGroup.vue'

const FIELD = { name: 'http.status_code', kind: 'promoted' }

function mountGroup(props) {
  return mount(FacetFieldGroup, {
    props: { field: FIELD, open: true, query: '', ...props },
  })
}

describe('FacetFieldGroup', () => {
  it('renders the header with data-test + aria-expanded and forwards toggle-open', async () => {
    const wrapper = mountGroup({ open: false })
    const header = wrapper.get('[data-test="facet-field-http.status_code"]')
    expect(header.attributes('aria-expanded')).toBe('false')
    await header.trigger('click')
    expect(wrapper.emitted('toggle-open')[0]).toEqual(['http.status_code'])
  })

  it('renders values via FacetValueRow with meter share = count / field max', () => {
    const wrapper = mountGroup({
      values: [
        { value: '200', count: 100 },
        { value: '404', count: 25 },
      ],
    })
    expect(wrapper.get('[data-test="facet-value-200"] [data-test="facet-meter"]').attributes('style')).toContain(
      'width: 100%',
    )
    expect(wrapper.get('[data-test="facet-value-404"] [data-test="facet-meter"]').attributes('style')).toContain(
      'width: 25%',
    )
  })

  it('pins an out-of-list constrained value (null count, no meter) to the top', () => {
    const wrapper = mountGroup({ values: [{ value: '200', count: 100 }], query: '-http.status_code:500' })
    const rows = wrapper.findAll('ul > li > div[data-test^="facet-value-"]')
    expect(rows[0].attributes('data-test')).toBe('facet-value-500')
    expect(wrapper.find('[data-test="facet-value-500"] [data-test="facet-meter"]').exists()).toBe(false)
  })

  it('forwards toggle and only with the field name attached', async () => {
    const wrapper = mountGroup({ values: [{ value: '200', count: 100 }] })
    await wrapper.get('[data-test="facet-value-200"]').trigger('click')
    expect(wrapper.emitted('toggle')[0]).toEqual([{ field: 'http.status_code', value: '200' }])
    await wrapper.get('[data-test="facet-value-200-only"]').trigger('click')
    expect(wrapper.emitted('only')[0]).toEqual([{ field: 'http.status_code', value: '200' }])
  })

  it('shows a per-field Clear (explicit data-test) that emits `clear` with the field name', async () => {
    const wrapper = mountGroup({ values: [{ value: '200', count: 100 }], activeCount: 1, query: 'http.status_code:200' })
    await wrapper.get('[data-test="facet-field-http.status_code-clear"]').trigger('click')
    expect(wrapper.emitted('clear')[0]).toEqual(['http.status_code'])
  })

  it('offers the value-search once a field exceeds the threshold or is capped', () => {
    const few = mountGroup({ values: [{ value: '200', count: 1 }] })
    expect(few.find('input[aria-label="Filter http.status_code values"]').exists()).toBe(false)

    const capped = mountGroup({ values: [{ value: '200', count: 1 }], capped: true })
    expect(capped.find('input[aria-label="Filter http.status_code values"]').exists()).toBe(true)
  })
})

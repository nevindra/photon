import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import FacetSection from './FacetSection.vue'

describe('FacetSection', () => {
  it('renders the label, an active-count badge, and the slotted rows', () => {
    const wrapper = mount(FacetSection, {
      props: { label: 'Services', count: 2 },
      slots: { default: '<div data-test="row">a</div>' },
    })
    expect(wrapper.text()).toContain('Services')
    expect(wrapper.text()).toContain('2')
    expect(wrapper.find('[data-test="row"]').exists()).toBe(true)
  })

  it('renders skeletons while loading (with the optional loading data-test) and no rows', () => {
    const wrapper = mount(FacetSection, {
      props: { label: 'Services', loading: true, loadingDataTest: 'qf-service-skeleton' },
      slots: { default: '<div data-test="row">a</div>' },
    })
    expect(wrapper.find('[data-test="qf-service-skeleton"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="row"]').exists()).toBe(false)
  })

  it('renders the empty text when empty', () => {
    const wrapper = mount(FacetSection, { props: { label: 'Services', empty: true, emptyText: 'No services' } })
    expect(wrapper.text()).toContain('No services')
  })

  it('shows the Clear button only when active and emits `clear`', async () => {
    const inactive = mount(FacetSection, { props: { label: 'Services', clearDataTest: 'fr-clear-service' } })
    expect(inactive.find('[data-test="fr-clear-service"]').exists()).toBe(false)

    const active = mount(FacetSection, {
      props: { label: 'Services', active: true, clearDataTest: 'fr-clear-service' },
    })
    await active.get('[data-test="fr-clear-service"]').trigger('click')
    expect(active.emitted('clear')).toHaveLength(1)
  })
})

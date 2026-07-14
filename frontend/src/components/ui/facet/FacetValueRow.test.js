import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import FacetValueRow from './FacetValueRow.vue'

describe('FacetValueRow', () => {
  it('renders a Checkbox marker by default that reflects `checked`', () => {
    const checked = mount(FacetValueRow, { props: { label: 'frontend', count: 9, checked: true } })
    expect(checked.findComponent({ name: 'Checkbox' }).props('modelValue')).toBe(true)

    const unchecked = mount(FacetValueRow, { props: { label: 'frontend', count: 9, checked: false } })
    expect(unchecked.findComponent({ name: 'Checkbox' }).props('modelValue')).toBe(false)
  })

  it('lets a caller override the marker via the #marker slot (with the checked slot prop)', () => {
    const wrapper = mount(FacetValueRow, {
      props: { label: 'Error', count: 3, checked: true },
      slots: { marker: `<template #marker="{ checked }"><span class="dot" :data-on="checked" /></template>` },
    })
    // The default Checkbox is replaced by the slot content.
    expect(wrapper.findComponent({ name: 'Checkbox' }).exists()).toBe(false)
    expect(wrapper.find('span.dot').attributes('data-on')).toBe('true')
  })

  it('renders the count and a track fill whose width follows `share`', () => {
    const wrapper = mount(FacetValueRow, { props: { label: 'us', count: 100, share: 0.5, checked: true } })
    expect(wrapper.find('[data-test="facet-count"]').text()).toBe('100')
    const fill = wrapper.find('[data-test="facet-meter"]')
    expect(fill.exists()).toBe(true)
    expect(fill.attributes('style')).toContain('width: 50%')
  })

  it('tints the checked track fill with the brand accent by default', () => {
    const wrapper = mount(FacetValueRow, { props: { label: 'us', count: 10, share: 0.5, checked: true } })
    expect(wrapper.find('[data-test="facet-meter"]').classes().join(' ')).toContain('bg-brand/[0.13]')
  })

  it('uses a neutral fill (no brand) when `neutralFill` is set, even if checked', () => {
    const wrapper = mount(FacetValueRow, {
      props: { label: 'error', count: 10, share: 0.5, checked: true, neutralFill: true },
    })
    const cls = wrapper.find('[data-test="facet-meter"]').classes().join(' ')
    expect(cls).toContain('bg-foreground/[0.09]')
    expect(cls).not.toContain('bg-brand')
  })

  it('a null count renders neither the count nor the meter', () => {
    const wrapper = mount(FacetValueRow, { props: { label: 'eu', count: null, share: 0.5 } })
    expect(wrapper.find('[data-test="facet-count"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="facet-meter"]').exists()).toBe(false)
  })

  it('a null share renders no meter but keeps the count', () => {
    const wrapper = mount(FacetValueRow, { props: { label: 'eu', count: 12, share: null } })
    expect(wrapper.find('[data-test="facet-count"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="facet-meter"]').exists()).toBe(false)
  })

  it('emits `toggle` when the row is clicked', async () => {
    const wrapper = mount(FacetValueRow, { props: { label: 'us', count: 9, dataTest: 'facet-value-us' } })
    await wrapper.get('[data-test="facet-value-us"]').trigger('click')
    expect(wrapper.emitted('toggle')).toHaveLength(1)
  })

  it('emits `only` (not `toggle`) when the Only button is clicked — @click.stop', async () => {
    const wrapper = mount(FacetValueRow, { props: { label: 'us', count: 9, dataTest: 'facet-value-us' } })
    await wrapper.get('[data-test="facet-value-us-only"]').trigger('click')
    expect(wrapper.emitted('only')).toHaveLength(1)
    // @click.stop must prevent the click from bubbling to the row's toggle handler.
    expect(wrapper.emitted('toggle')).toBeUndefined()
  })

  it('reflects `checked` on aria-pressed', async () => {
    const wrapper = mount(FacetValueRow, { props: { label: 'us', count: 9, checked: true, dataTest: 'r' } })
    expect(wrapper.get('[data-test="r"]').attributes('aria-pressed')).toBe('true')
    await wrapper.setProps({ checked: false })
    expect(wrapper.get('[data-test="r"]').attributes('aria-pressed')).toBe('false')
  })
})

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import LogsFilters from './LogsFilters.vue'
import { api } from '@/lib/core/api'
import { Checkbox } from '@/components/ui/checkbox'

// LogsFilters merges the old prop-driven FilterRail (pinned Services/Severity) and the
// self-fetching FacetRail (Fields catalog) into one adapter over the shared `ui/facet/`
// primitives. It now CONTAINS the catalog (which calls `useFields`/`useQueries`), so every mount
// needs a fresh QueryClient + VueQueryPlugin (`retry: false` keeps a rejected mock from silently
// retrying and making a test look slower/flakier). The pinned-section tests also need
// `api.fields` stubbed to `[]` so the catalog renders empty without touching the network — the
// mount helper does that automatically unless a test has already spied `api.fields` itself.
function mountFilters(props) {
  if (!vi.isMockFunction(api.fields)) vi.spyOn(api, 'fields').mockResolvedValue([])
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(LogsFilters, {
    props: {
      query: '',
      services: ['checkout-api', 'payments-api'],
      serviceCounts: { 'checkout-api': 120, 'payments-api': 30 },
      severityCounts: { info: 100, warn: 10, error: 5 },
      startMs: 0,
      endMs: 100,
      ...props,
    },
    global: { plugins: [[VueQueryPlugin, { queryClient }]] },
  })
}

afterEach(() => {
  vi.restoreAllMocks()
})

// --- Pinned Services/Severity (ported from FilterRail.test.js) --------------------------------
// Checked state derives purely from `facetChecked(query, field, value)` (single-state model, see
// .superpowers/sdd/facet-single-state-model.md); the parent (LogsView) supplies
// `services`/`serviceCounts`/`severityCounts` plus the search `query`.
describe('LogsFilters — pinned Services/Severity', () => {
  it('renders service and severity sections with counts', () => {
    const wrapper = mountFilters()

    expect(wrapper.get('[data-test="fr-service-checkout-api"]').text()).toContain('checkout-api')
    expect(wrapper.get('[data-test="fr-service-checkout-api"]').text()).toContain('120')
    expect(wrapper.get('[data-test="fr-severity-error"]').text()).toContain('5')
  })

  it('checks every service by default (empty query = all-mode, all checked)', () => {
    const wrapper = mountFilters({ query: '' })

    expect(
      wrapper.get('[data-test="fr-service-checkout-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(true)
    expect(
      wrapper.get('[data-test="fr-service-payments-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(true)
  })

  it('unchecks a service excluded via -service:x', () => {
    const wrapper = mountFilters({ query: '-service:checkout-api' })

    expect(
      wrapper.get('[data-test="fr-service-checkout-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(false)
    expect(
      wrapper.get('[data-test="fr-service-payments-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(true)
  })

  it('checks only the included service in include-mode', () => {
    const wrapper = mountFilters({ query: 'service:checkout-api' })

    expect(
      wrapper.get('[data-test="fr-service-checkout-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(true)
    expect(
      wrapper.get('[data-test="fr-service-payments-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(false)
  })

  it('emits toggle-value {field: "service", value} when a service row is clicked', async () => {
    const wrapper = mountFilters()

    await wrapper.get('[data-test="fr-service-checkout-api"]').trigger('click')

    expect(wrapper.emitted('toggle-value')[0]).toEqual([{ field: 'service', value: 'checkout-api' }])
  })

  it('emits only-value {field: "service", value} from the hover Only button', async () => {
    const wrapper = mountFilters()

    await wrapper.get('[data-test="fr-service-checkout-api-only"]').trigger('click')

    expect(wrapper.emitted('only-value')[0]).toEqual([{ field: 'service', value: 'checkout-api' }])
  })

  it('shows Clear for a section only when it has active constraints, and emits clear-field', async () => {
    const wrapper = mountFilters({ query: 'service:checkout-api' })

    expect(wrapper.find('[data-test="fr-clear-service"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="fr-clear-level"]').exists()).toBe(false)

    await wrapper.get('[data-test="fr-clear-service"]').trigger('click')
    expect(wrapper.emitted('clear-field')[0]).toEqual(['service'])
  })

  it('drives severity checked state off grammar field "level"', () => {
    const checkedWrapper = mountFilters({ query: 'level:error' })
    expect(checkedWrapper.get('[data-test="fr-severity-error"]').attributes('aria-pressed')).toBe('true')
    expect(checkedWrapper.get('[data-test="fr-severity-info"]').attributes('aria-pressed')).toBe('false')

    const excludedWrapper = mountFilters({ query: '-level:error' })
    expect(excludedWrapper.get('[data-test="fr-severity-error"]').attributes('aria-pressed')).toBe('false')
    expect(excludedWrapper.get('[data-test="fr-severity-info"]').attributes('aria-pressed')).toBe('true')
  })

  it('emits toggle-value and only-value with field "level" for severity rows', async () => {
    const wrapper = mountFilters()

    await wrapper.get('[data-test="fr-severity-error"]').trigger('click')
    expect(wrapper.emitted('toggle-value')[0]).toEqual([{ field: 'level', value: 'error' }])

    await wrapper.get('[data-test="fr-severity-error-only"]').trigger('click')
    expect(wrapper.emitted('only-value')[0]).toEqual([{ field: 'level', value: 'error' }])
  })

  it('emits clear-field "level" from the severity section Clear', async () => {
    const wrapper = mountFilters({ query: 'level:error' })

    await wrapper.get('[data-test="fr-clear-level"]').trigger('click')
    expect(wrapper.emitted('clear-field')[0]).toEqual(['level'])
  })
})

// --- Fields catalog (ported from FacetRail.test.js) -------------------------------------------
// The catalog is now the shared FacetCatalog primitive: it groups promoted fields up top and
// folds raw attributes into a collapsed "Attributes" group, so an UNCONSTRAINED attribute field
// is reached by expanding that group first (a constrained field auto-opens the group + itself).
describe('LogsFilters — Fields catalog', () => {
  it('lists catalog fields (excluding pinned ones) and emits toggle-value on a value click', async () => {
    vi.spyOn(api, 'fields').mockResolvedValue([
      { name: 'service.name', kind: 'promoted' }, // hidden — already a pinned Services quick-filter
      { name: 'region', kind: 'attribute' },
    ])
    vi.spyOn(api, 'facet').mockResolvedValue({
      values: [
        { value: 'us', count: 9 },
        { value: 'eu', count: 4 },
      ],
      capped: false,
    })
    const wrapper = mountFilters({ query: '', startMs: 0, endMs: 100 })
    await flushPromises()

    // service.name is hidden (surfaced in the Services section); region lives in the collapsed
    // Attributes group — expand it to reach the field.
    expect(wrapper.find('[data-test="facet-field-service.name"]').exists()).toBe(false)
    await wrapper.get('[data-test="facet-group-attributes"]').trigger('click')
    expect(wrapper.find('[data-test="facet-field-region"]').exists()).toBe(true)

    // Expand `region`, then click its top value → emits a { field, value } toggle (the parent
    // turns it into a `-region:us` or `region:us` term via toggleFacetValue).
    await wrapper.get('[data-test="facet-field-region"]').trigger('click')
    await flushPromises()
    await wrapper.get('[data-test="facet-value-us"]').trigger('click')

    expect(wrapper.emitted('toggle-value')[0]).toEqual([{ field: 'region', value: 'us' }])
  })

  it('single-state checkbox: default all-checked, unchecked when the value is excluded', async () => {
    vi.spyOn(api, 'fields').mockResolvedValue([{ name: 'region', kind: 'attribute' }])
    vi.spyOn(api, 'facet').mockResolvedValue({
      values: [
        { value: 'us', count: 9 },
        { value: 'eu', count: 4 },
      ],
      capped: false,
    })
    const wrapper = mountFilters({ query: '-region:eu', startMs: 0, endMs: 100 })
    await flushPromises() // auto-opens `region` (+ its group) since it already has a constraint
    await flushPromises() // lets the resulting useQueries facet fetch resolve

    // Default all-checked: `us` carries no term at all, so facetChecked leaves it checked.
    const usRow = wrapper.get('[data-test="facet-value-us"]')
    expect(usRow.findComponent({ name: 'Checkbox' }).props('modelValue')).toBe(true)

    // Explicitly excluded via `-region:eu` → unchecked, but still visible (no strike-through).
    const euRow = wrapper.get('[data-test="facet-value-eu"]')
    expect(euRow.findComponent({ name: 'Checkbox' }).props('modelValue')).toBe(false)
    expect(euRow.find('span').classes()).not.toContain('line-through')
  })

  it('emits only-value when the hover "Only" button is clicked', async () => {
    vi.spyOn(api, 'fields').mockResolvedValue([{ name: 'host.name', kind: 'attribute' }])
    vi.spyOn(api, 'facet').mockResolvedValue({
      values: [{ value: 'h1', count: 3 }],
      capped: false,
    })
    const wrapper = mountFilters({ query: '', startMs: 0, endMs: 100 })
    await flushPromises()

    await wrapper.get('[data-test="facet-group-attributes"]').trigger('click')
    await wrapper.get('[data-test="facet-field-host.name"]').trigger('click')
    await flushPromises()

    await wrapper.get('[data-test="facet-value-h1-only"]').trigger('click')

    expect(wrapper.emitted('only-value')[0]).toEqual([{ field: 'host.name', value: 'h1' }])
    // The Only button uses @click.stop, so it must not also fire the row's toggle-value.
    expect(wrapper.emitted('toggle-value')).toBeUndefined()
  })

  it('emits clear-field when the per-field Clear button is clicked', async () => {
    vi.spyOn(api, 'fields').mockResolvedValue([{ name: 'region', kind: 'attribute' }])
    vi.spyOn(api, 'facet').mockResolvedValue({
      values: [{ value: 'us', count: 9 }],
      capped: false,
    })
    const wrapper = mountFilters({ query: 'region:us', startMs: 0, endMs: 100 })
    await flushPromises() // auto-opens `region` (+ its group) since it already has a selection
    await flushPromises() // lets the resulting useQueries facet fetch resolve

    // The per-field Clear now carries its own data-test hook on the shared FacetFieldGroup.
    await wrapper.get('[data-test="facet-field-region-clear"]').trigger('click')

    expect(wrapper.emitted('clear-field')[0]).toEqual(['region'])
  })

  it('strips the field own terms with removeFieldAll (both include and exclude) when faceting', async () => {
    vi.spyOn(api, 'fields').mockResolvedValue([{ name: 'region', kind: 'attribute' }])
    const facetSpy = vi.spyOn(api, 'facet').mockResolvedValue({
      values: [{ value: 'us', count: 9 }],
      capped: false,
    })
    // Query carries BOTH a positive include of another field and an exclusion of `region` itself
    // — removeFieldAll must drop the region exclusion but keep the unrelated term.
    const wrapper = mountFilters({ query: 'status:ok -region:eu', startMs: 0, endMs: 100 })
    await flushPromises() // auto-opens `region` since it already has a constraint
    await flushPromises() // lets the resulting useQueries facet fetch resolve

    expect(facetSpy).toHaveBeenCalledWith('region', 'status:ok', '0', '100000000', 50, expect.anything())
  })

  it('pins an excluded value not in the fetched top values, still visible and unchecked', async () => {
    vi.spyOn(api, 'fields').mockResolvedValue([{ name: 'region', kind: 'attribute' }])
    vi.spyOn(api, 'facet').mockResolvedValue({
      values: [{ value: 'us', count: 9 }],
      capped: false,
    })
    const wrapper = mountFilters({ query: '-region:eu', startMs: 0, endMs: 100 })
    await flushPromises() // auto-opens `region` since it already has a constraint
    await flushPromises() // lets the resulting useQueries facet fetch resolve

    const excludedRow = wrapper.get('[data-test="facet-value-eu"]')
    expect(excludedRow.exists()).toBe(true)
    expect(excludedRow.findComponent({ name: 'Checkbox' }).props('modelValue')).toBe(false)

    // Pinned (constrained) value floats to the top, ahead of the fetched 'us'.
    const rows = wrapper.findAll('ul > li > div[data-test^="facet-value-"]')
    expect(rows[0].attributes('data-test')).toBe('facet-value-eu')
  })
})

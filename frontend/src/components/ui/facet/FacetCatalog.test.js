import { describe, it, expect, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import FacetCatalog from './FacetCatalog.vue'

// FacetCatalog fans out per-open-field facet fetches through vue-query (`useQueries`), so every
// mount needs a fresh QueryClient + VueQueryPlugin; `retry: false` keeps a rejected mock from
// silently retrying. Unlike the old rails it does NOT self-fetch its field catalog — the adapter
// passes `fields` and an injected `facetFn` in as props, which is exactly what the tests supply.
function mountCatalog(props, facetFn) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(FacetCatalog, {
    props: {
      queryKeyPrefix: 'facet',
      startNs: '0',
      endNs: '100000000',
      facetFn: facetFn ?? vi.fn().mockResolvedValue({ values: [], capped: false }),
      ...props,
    },
    global: { plugins: [[VueQueryPlugin, { queryClient }]] },
  })
}

const PROMOTED_AND_ATTR = [
  { name: 'http.status_code', kind: 'promoted' },
  { name: 'db.system', kind: 'attribute' },
]

describe('FacetCatalog grouping', () => {
  it('splits promoted fields from the folded Attributes long tail', async () => {
    const wrapper = mountCatalog({ fields: PROMOTED_AND_ATTR, query: '' })
    await flushPromises()

    // Promoted group + its field are shown up top.
    expect(wrapper.find('[data-test="facet-group-promoted"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="facet-field-http.status_code"]').exists()).toBe(true)

    // Attributes group header shows a count; its field stays folded (collapsed by default).
    const attrsHeader = wrapper.find('[data-test="facet-group-attributes"]')
    expect(attrsHeader.exists()).toBe(true)
    expect(attrsHeader.text()).toContain('1')
    expect(attrsHeader.attributes('aria-expanded')).toBe('false')
    expect(wrapper.find('[data-test="facet-field-db.system"]').exists()).toBe(false)
  })

  it('Attributes (N) is collapsed by default and expands on click', async () => {
    const wrapper = mountCatalog({ fields: PROMOTED_AND_ATTR, query: '' })
    await flushPromises()

    expect(wrapper.find('[data-test="facet-field-db.system"]').exists()).toBe(false)
    await wrapper.get('[data-test="facet-group-attributes"]').trigger('click')
    expect(wrapper.find('[data-test="facet-group-attributes"]').attributes('aria-expanded')).toBe('true')
    expect(wrapper.find('[data-test="facet-field-db.system"]').exists()).toBe(true)
  })

  it('a non-empty field search flattens both groups and reaches any field by name', async () => {
    const wrapper = mountCatalog({ fields: PROMOTED_AND_ATTR, query: '' })
    await flushPromises()

    await wrapper.get('input[aria-label="Filter fields"]').setValue('db')
    await flushPromises()

    // No group headers while searching — one flat filtered list.
    expect(wrapper.find('[data-test="facet-group-promoted"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="facet-group-attributes"]').exists()).toBe(false)
    // The attribute field is reachable without expanding its (now-suppressed) group.
    expect(wrapper.find('[data-test="facet-field-db.system"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="facet-field-http.status_code"]').exists()).toBe(false)
  })

  it('auto-opens a constrained attribute field AND forces its group open', async () => {
    const facetFn = vi.fn().mockResolvedValue({ values: [{ value: 'postgres', count: 5 }], capped: false })
    const wrapper = mountCatalog(
      { fields: [{ name: 'db.system', kind: 'attribute' }], query: '-db.system:mysql' },
      facetFn,
    )
    await flushPromises() // auto-open watch runs, then the facet fetch resolves
    await flushPromises()

    // The constrained attribute field is visible with NO manual expand — its Attributes group was
    // forced open — and the field itself is expanded.
    const header = wrapper.get('[data-test="facet-field-db.system"]')
    expect(header.attributes('aria-expanded')).toBe('true')
    // Fetched value + the pinned (excluded, out-of-list) value both render.
    expect(wrapper.find('[data-test="facet-value-postgres"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="facet-value-mysql"]').exists()).toBe(true)
  })

  it('facets each field against the query with its OWN terms stripped (removeFieldAll)', async () => {
    const facetFn = vi.fn().mockResolvedValue({ values: [{ value: 'us', count: 9 }], capped: false })
    // Query carries an unrelated positive include AND an exclusion of `region` itself — the
    // region facet request must drop the region exclusion but keep the unrelated `status:ok`.
    mountCatalog({ fields: [{ name: 'region', kind: 'attribute' }], query: 'status:ok -region:eu' }, facetFn)
    await flushPromises()

    expect(facetFn).toHaveBeenCalledWith('region', 'status:ok', '0', '100000000', 50, expect.anything())
  })
})

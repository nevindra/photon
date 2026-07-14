import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import TracesFilters from './TracesFilters.vue'
import { Checkbox } from '@/components/ui/checkbox'
import { api } from '@/lib/core/api'

// TracesFilters merges the old TracesQuickFilters (self-fetched Service/Status/Kind pinned
// sections) and SpanFacetRail (the fields catalog) into one adapter over the shared ui/facet
// primitives. The pinned sections self-fetch via `useTracesFacet` and the catalog fans out
// `api.tracesFacet` per open field through `useQueries` — so, like SpanFacetRail's suite, every
// mount uses a real QueryClient and mocks at the API layer (not the hook). One `api.tracesFacet`
// mock serves BOTH the pinned sections and the catalog, keyed off the field argument:
// `service.name`/`status_text`/`kind_text` are the pinned columns, anything else is a catalog
// facet. `api.tracesFields` feeds the catalog (empty for the pinned-only cases).
function stubApi({ service = [], status = [], kind = [], fields = [], catalog, hang = {} } = {}) {
  vi.spyOn(api, 'tracesFields').mockResolvedValue(fields)
  vi.spyOn(api, 'tracesFacet').mockImplementation((field) => {
    if (hang[field]) return new Promise(() => {}) // never resolves → the section stays loading
    if (field === 'service.name') return Promise.resolve({ values: service, capped: false })
    if (field === 'status_text') return Promise.resolve({ values: status, capped: false })
    if (field === 'kind_text') return Promise.resolve({ values: kind, capped: false })
    return Promise.resolve(catalog ?? { values: [], capped: false })
  })
}

function mountFilters(props) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(TracesFilters, {
    props: { query: '', startMs: 0, endMs: 100, ...props },
    global: { plugins: [[VueQueryPlugin, { queryClient }]] },
  })
}

beforeEach(() => {
  vi.restoreAllMocks()
})

describe('TracesFilters — pinned Service/Status/Kind', () => {
  it('renders service/status/kind sections with facet values and counts', async () => {
    stubApi({
      service: [{ value: 'checkout-api', count: 120 }],
      status: [
        { value: 'OK', count: 100 },
        { value: 'ERROR', count: 5 },
      ],
      kind: [{ value: 'SERVER', count: 80 }],
    })
    const wrapper = mountFilters()
    await flushPromises()

    expect(wrapper.get('[data-test="qf-service-checkout-api"]').text()).toContain('checkout-api')
    expect(wrapper.get('[data-test="qf-service-checkout-api"]').text()).toContain('120')

    // Status/Kind always render the full fixed keyword domain (there's no "unknown status"),
    // with a zero count for keywords absent from the facet response.
    expect(wrapper.get('[data-test="qf-status-ok"]').text()).toContain('100')
    expect(wrapper.get('[data-test="qf-status-error"]').text()).toContain('5')
    expect(wrapper.get('[data-test="qf-status-unset"]').text()).toContain('0')

    expect(wrapper.get('[data-test="qf-kind-server"]').text()).toContain('80')
    expect(wrapper.get('[data-test="qf-kind-client"]').text()).toContain('0')
    expect(wrapper.get('[data-test="qf-kind-internal"]').text()).toContain('0')
    expect(wrapper.get('[data-test="qf-kind-producer"]').text()).toContain('0')
    expect(wrapper.get('[data-test="qf-kind-consumer"]').text()).toContain('0')
  })

  it('emits toggle-value {field: "service", value} when a service row is clicked', async () => {
    stubApi({ service: [{ value: 'checkout-api', count: 120 }] })
    const wrapper = mountFilters()
    await flushPromises()

    await wrapper.get('[data-test="qf-service-checkout-api"]').trigger('click')

    expect(wrapper.emitted('toggle-value')[0]).toEqual([{ field: 'service', value: 'checkout-api' }])
  })

  it('emits toggle-value with the lower-case grammar keyword for status/kind rows', async () => {
    stubApi({
      status: [{ value: 'ERROR', count: 5 }],
      kind: [{ value: 'CLIENT', count: 20 }],
    })
    const wrapper = mountFilters()
    await flushPromises()

    await wrapper.get('[data-test="qf-status-error"]').trigger('click')
    expect(wrapper.emitted('toggle-value')[0]).toEqual([{ field: 'status', value: 'error' }])

    await wrapper.get('[data-test="qf-kind-client"]').trigger('click')
    expect(wrapper.emitted('toggle-value')[1]).toEqual([{ field: 'kind', value: 'client' }])
  })

  it('derives checkbox state from the query and emits clear-field per section', async () => {
    stubApi({
      service: [{ value: 'checkout-api', count: 5 }],
      status: [{ value: 'ERROR', count: 5 }],
    })
    const wrapper = mountFilters({ query: 'service:checkout-api status:error' })
    await flushPromises()

    expect(
      wrapper.get('[data-test="qf-service-checkout-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(true)
    expect(wrapper.get('[data-test="qf-status-error"]').findComponent(Checkbox).props('modelValue')).toBe(true)
    expect(wrapper.get('[data-test="qf-status-ok"]').findComponent(Checkbox).props('modelValue')).toBe(false)

    // Clear is only shown for sections with an active selection.
    expect(wrapper.find('[data-test="qf-clear-kind"]').exists()).toBe(false)

    await wrapper.get('[data-test="qf-clear-service"]').trigger('click')
    expect(wrapper.emitted('clear-field')[0]).toEqual(['service'])

    await wrapper.get('[data-test="qf-clear-status"]').trigger('click')
    expect(wrapper.emitted('clear-field')[1]).toEqual(['status'])
  })

  it('shows a loading skeleton while a section facet is pending', async () => {
    stubApi({ hang: { 'service.name': true } })
    const wrapper = mountFilters()
    await flushPromises()

    expect(wrapper.find('[data-test="qf-service-skeleton"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="qf-service-checkout-api"]').exists()).toBe(false)
  })

  it('shows an empty state for the service section when the facet has no values', async () => {
    stubApi({ service: [] })
    const wrapper = mountFilters()
    await flushPromises()

    expect(wrapper.text()).toContain('No services')
  })

  it('renders the Status "Error" label red only when it is checked', async () => {
    stubApi({ status: [{ value: 'ERROR', count: 5 }] })

    // `status:error` puts error in the set → checked → red accent.
    const checked = mountFilters({ query: 'status:error' })
    await flushPromises()
    expect(checked.get('[data-test="qf-status-error"] span.flex-1').classes()).toContain('text-sev-error')

    // `-status:error` excludes error → unchecked → no red accent.
    const unchecked = mountFilters({ query: '-status:error' })
    await flushPromises()
    expect(unchecked.get('[data-test="qf-status-error"] span.flex-1').classes()).not.toContain('text-sev-error')
  })

  it('facets Service with its OWN grammar field stripped (other services stay visible)', async () => {
    stubApi({ service: [{ value: 'a', count: 5 }] })
    mountFilters({ query: 'service:a' })
    await flushPromises()

    // The Service section facets `service.name` with the grammar alias `service` stripped, so the
    // rest of the services still come back — `service:a` reduces to an empty facet query.
    expect(api.tracesFacet).toHaveBeenCalledWith('service.name', '', '0', '100000000', 50, expect.anything())
  })

  it('facets Status/Kind with their OWN grammar field stripped too', async () => {
    stubApi({ status: [{ value: 'ERROR', count: 5 }], kind: [{ value: 'SERVER', count: 5 }] })
    mountFilters({ query: 'status:error kind:server' })
    await flushPromises()

    // Each section strips only ITS OWN grammar field — the other section's term is preserved.
    expect(api.tracesFacet).toHaveBeenCalledWith('status_text', 'kind:server', '0', '100000000', 50, expect.anything())
    expect(api.tracesFacet).toHaveBeenCalledWith('kind_text', 'status:error', '0', '100000000', 50, expect.anything())
  })

  it('emits only-value from the hover "Only" action (no exclude action exists)', async () => {
    stubApi({ service: [{ value: 'a', count: 5 }] })
    const wrapper = mountFilters()
    await flushPromises()

    await wrapper.get('[data-test="qf-service-a-only"]').trigger('click')
    expect(wrapper.emitted('only-value')[0]).toEqual([{ field: 'service', value: 'a' }])

    // Single-state model — unchecking a value IS excluding it; there is no second hover action.
    expect(wrapper.find('[data-test="qf-service-a-exclude"]').exists()).toBe(false)
  })

  it('derives the checkbox from facetChecked: all-checked by default, unchecked when excluded', async () => {
    stubApi({ service: [{ value: 'checkout-api', count: 5 }] })

    const wrapper = mountFilters({ query: '-service:checkout-api' })
    await flushPromises()
    expect(
      wrapper.get('[data-test="qf-service-checkout-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(false)

    const defaultWrapper = mountFilters()
    await flushPromises()
    expect(
      defaultWrapper.get('[data-test="qf-service-checkout-api"]').findComponent(Checkbox).props('modelValue'),
    ).toBe(true)
  })

  it('pins an excluded service value (unchecked) even if the facet no longer returns it', async () => {
    stubApi({ service: [{ value: 'checkout-api', count: 5 }] })
    const wrapper = mountFilters({ query: '-service:legacy-api' })
    await flushPromises()

    const row = wrapper.get('[data-test="qf-service-legacy-api"]')
    expect(row.text()).toContain('legacy-api')
    expect(row.findComponent(Checkbox).props('modelValue')).toBe(false)
  })

  it('pins an included service value (checked) even if it is outside the fetched top-N', async () => {
    stubApi({ service: [{ value: 'checkout-api', count: 5 }] })
    const wrapper = mountFilters({ query: 'service:niche-api' })
    await flushPromises()

    // `niche-api` isn't in the fetched top-50, but it's explicitly included in the query — it must
    // still render, checked, not silently vanish.
    const row = wrapper.get('[data-test="qf-service-niche-api"]')
    expect(row.text()).toContain('niche-api')
    expect(row.findComponent(Checkbox).props('modelValue')).toBe(true)
  })

  it('facets Service with removeFieldAll (both the include AND exclude of its own field stripped)', async () => {
    stubApi({ service: [{ value: 'a', count: 5 }] })
    mountFilters({ query: '-service:a status:error' })
    await flushPromises()

    // removeFieldAll drops the field's own `-service:a` exclusion too (removeField would have kept
    // it), so every service — including the excluded one — gets an honest count.
    expect(api.tracesFacet).toHaveBeenCalledWith('service.name', 'status:error', '0', '100000000', 50, expect.anything())
  })
})

describe('TracesFilters — fields catalog', () => {
  it('lists the spans catalog (excluding hidden fixed fields) and emits toggle-value on a value click', async () => {
    stubApi({
      fields: [
        { name: 'service.name', kind: 'promoted' }, // hidden — pinned Service quick-filter
        { name: 'trace_id', kind: 'fixed' }, // hidden — not meaningfully groupable
        { name: 'status_text', kind: 'fixed' }, // NOT hidden — human-readable enum, promoted group
        { name: 'region', kind: 'attribute' }, // folded under the Attributes long tail
      ],
      catalog: {
        values: [
          { value: 'us', count: 9 },
          { value: 'eu', count: 4 },
        ],
        capped: false,
      },
    })
    const wrapper = mountFilters({ query: '' })
    await flushPromises()

    expect(wrapper.find('[data-test="facet-field-service.name"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="facet-field-trace_id"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="facet-field-status_text"]').exists()).toBe(true)

    // `region` is an attribute → folded under the collapsed Attributes group until it's expanded.
    expect(wrapper.find('[data-test="facet-field-region"]').exists()).toBe(false)
    await wrapper.get('[data-test="facet-group-attributes"]').trigger('click')
    expect(wrapper.find('[data-test="facet-field-region"]').exists()).toBe(true)

    // Expand `region`, then click its top value → emits a { field, value } toggle (the parent turns
    // it into a `region:us` term via toggleFieldValue, identically to the logs rail).
    await wrapper.get('[data-test="facet-field-region"]').trigger('click')
    await flushPromises()
    await wrapper.get('[data-test="facet-value-us"]').trigger('click')

    expect(wrapper.emitted('toggle-value')[0]).toEqual([{ field: 'region', value: 'us' }])
  })

  it('emits clear-field when the per-field Clear button is clicked', async () => {
    stubApi({
      fields: [{ name: 'region', kind: 'attribute' }],
      catalog: { values: [{ value: 'us', count: 9 }], capped: false },
    })
    const wrapper = mountFilters({ query: 'region:us' })
    await flushPromises() // auto-opens `region` (constrained) and forces its Attributes group open
    await flushPromises() // lets the resulting useQueries facet fetch resolve

    // The catalog per-field Clear now has an explicit `facet-field-{name}-clear` hook (replacing
    // the old fragile `+ div > button` selector the SpanFacetRail test relied on).
    await wrapper.get('[data-test="facet-field-region-clear"]').trigger('click')

    expect(wrapper.emitted('clear-field')[0]).toEqual(['region'])
  })

  it('emits only-value for a field value (hover "Only" button)', async () => {
    stubApi({
      fields: [{ name: 'host.name', kind: 'attribute' }],
      catalog: { values: [{ value: 'h1', count: 3 }], capped: false },
    })
    const wrapper = mountFilters({ query: '' })
    await flushPromises()

    await wrapper.get('[data-test="facet-group-attributes"]').trigger('click')
    await wrapper.get('[data-test="facet-field-host.name"]').trigger('click')
    await flushPromises()

    await wrapper.get('[data-test="facet-value-h1-only"]').trigger('click')
    expect(wrapper.emitted('only-value')[0]).toEqual([{ field: 'host.name', value: 'h1' }])

    // Single-state model: there is no separate "Exclude" action anymore.
    expect(wrapper.find('[data-test="facet-value-h1-exclude"]').exists()).toBe(false)
  })

  it('checkbox reflects facetChecked: all values checked by default, unchecked when excluded', async () => {
    stubApi({
      fields: [{ name: 'region', kind: 'attribute' }],
      catalog: {
        values: [
          { value: 'us', count: 9 },
          { value: 'eu', count: 4 },
        ],
        capped: false,
      },
    })
    // `-region:eu` gives the field a constraint, so it auto-opens (and forces the Attributes group
    // open) on its own — no manual click needed.
    const wrapper = mountFilters({ query: '-region:eu' })
    await flushPromises()
    await flushPromises()

    // Default all-mode: 'us' has no exclusion, so it stays checked; 'eu' is negated, so unchecked.
    expect(wrapper.get('[data-test="facet-value-us"]').attributes('aria-pressed')).toBe('true')
    expect(wrapper.get('[data-test="facet-value-eu"]').attributes('aria-pressed')).toBe('false')

    // Excluded value stays visible (pinned, not dropped) with no strike-through — de-emphasized via
    // the shared FacetValueRow unchecked tone (`text-foreground/60`) instead.
    const label = wrapper.get('[data-test="facet-value-eu"] span.flex-1')
    expect(label.classes()).not.toContain('line-through')
    expect(label.classes()).toContain('text-foreground/60')
  })

  it('auto-opens a field that only carries a negated (exclude-only) constraint', async () => {
    stubApi({
      fields: [{ name: 'region', kind: 'attribute' }],
      catalog: {
        values: [
          { value: 'us', count: 9 },
          { value: 'eu', count: 4 },
        ],
        capped: false,
      },
    })
    // `-region:eu` has no positive `region:` term at all — fieldValues() alone would report zero
    // constraints and never auto-open; fieldConstraintCount() counts the exclusion too.
    const wrapper = mountFilters({ query: '-region:eu' })
    await flushPromises()
    await flushPromises()

    expect(wrapper.get('[data-test="facet-value-eu"]').attributes('aria-pressed')).toBe('false')
    expect(wrapper.get('[data-test="facet-value-us"]').attributes('aria-pressed')).toBe('true')
  })

  it('strips the catalog facet fetch with removeFieldAll (both positive and negated terms)', async () => {
    stubApi({
      fields: [{ name: 'region', kind: 'attribute' }],
      catalog: { values: [{ value: 'us', count: 9 }], capped: false },
    })
    // A negated `-region:jp` term alongside an unrelated field term auto-opens `region` and fires
    // its facet fetch. `removeField` would leave `-region:jp` behind, skewing region's own
    // breakdown; `removeFieldAll` must drop both signs and keep only the unrelated `other:x`.
    mountFilters({ query: '-region:jp other:x' })
    await flushPromises()
    await flushPromises()

    expect(api.tracesFacet).toHaveBeenCalledWith('region', 'other:x', '0', '100000000', 50, expect.anything())
  })
})

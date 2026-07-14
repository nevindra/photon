import { describe, it, expect, beforeAll, afterAll, beforeEach, vi } from 'vitest'
import { nextTick } from 'vue'
import { mount, flushPromises, DOMWrapper } from '@vue/test-utils'
import TraceTable from './TraceTable.vue'

const copySpy = vi.fn()
vi.mock('@/lib/core/useCopy', () => ({ useCopy: () => ({ copy: copySpy }) }))

// useTableColumns persists to localStorage under a fixed 'photon.cols.traces' key, so state must
// not leak between tests (e.g. an attribute column added in one test would still be there in the
// next mount otherwise).
beforeEach(() => {
  localStorage.clear()
  copySpy.mockClear()
})

// PopoverContent (used by the header's ColumnPicker) teleports to document.body, so once it's
// open we query the body rather than the component wrapper — same helper as ColumnPicker.test.js.
const body = () => new DOMWrapper(document.body)
async function openColumnPicker(wrapper) {
  const trigger = wrapper.findAll('button').find((b) => b.text() === 'Columns')
  await trigger.trigger('click')
  await nextTick()
  await new Promise((resolve) => setTimeout(resolve, 0))
}

// TraceTable now virtualizes its rows with @tanstack/vue-virtual, which sizes its visible range
// from the scroll element's offsetHeight. jsdom reports 0 there, and virtual-core returns an EMPTY
// range for a zero-height viewport — so stub a real height (bigger than the whole fixture) to make
// every row mount, and let the virtualizer settle after the scroll element is measured.
let restoreH, restoreW
beforeAll(() => {
  restoreH = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight')
  restoreW = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth')
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, get: () => 1200 })
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, get: () => 800 })
})
afterAll(() => {
  if (restoreH) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', restoreH)
  if (restoreW) Object.defineProperty(HTMLElement.prototype, 'offsetWidth', restoreW)
})

// Let the virtualizer measure its scroll element and re-render the visible slice.
async function settle() {
  await flushPromises()
  await nextTick()
}

// Reads the inline `width` style off a row's duration-bar fill (the `data-testid="duration-bar-fill"`
// span), scoped to the row for `id` via the existing `data-trace-id` row selector.
function barWidth(wrapper, id) {
  return wrapper.get(`[data-trace-id="${id}"] [data-testid="duration-bar-fill"]`).attributes('style')
}

function trace(id, overrides = {}) {
  return {
    trace_id: id,
    root_service: 'api',
    root_name: `GET /orders/${id}`,
    start_ts: 1_700_000_000_000_000_000n,
    duration_ns: 5_000_000n,
    span_count: 4,
    error_count: 0,
    services: ['api', 'db'],
    ...overrides,
  }
}

const TRACES = [
  trace('t1'),
  trace('t2', { duration_ns: 20_000_000n, error_count: 2, services: ['api'] }),
  trace('t3', { duration_ns: null, root_service: null, root_name: null }),
]

describe('TraceTable', () => {
  it('renders one row per trace', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    expect(w.findAll('[data-testid="trace-row"]').length).toBe(TRACES.length)
    w.unmount()
  })

  it('renders the Errors cell red when error_count > 0, muted otherwise', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    const rows = w.findAll('[data-testid="trace-row"]')
    const errCell1 = rows[0].find('[data-testid="error-count"]')
    const errCell2 = rows[1].find('[data-testid="error-count"]')
    expect(errCell1.classes()).toContain('text-muted-foreground')
    expect(errCell2.classes()).toContain('text-sev-error')
    w.unmount()
  })

  it('emits open-trace with the trace id and start_ts string on row click', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    await w.get('[data-trace-id="t2"]').trigger('click')
    expect(w.emitted('open-trace')[0]).toEqual([
      { traceId: 't2', timeHintNs: '1700000000000000000' },
    ])
    w.unmount()
  })

  it('emits open-trace on Enter/Space keydown for keyboard accessibility', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    const row = w.get('[data-trace-id="t1"]')
    await row.trigger('keydown', { key: 'Enter' })
    await row.trigger('keydown', { key: ' ' })
    expect(w.emitted('open-trace').length).toBe(2)
    expect(w.emitted('open-trace')[0]).toEqual([
      { traceId: 't1', timeHintNs: '1700000000000000000' },
    ])
    w.unmount()
  })

  it('renders skeleton rows while loading and no trace rows', () => {
    const w = mount(TraceTable, { props: { traces: [], loading: true } })
    expect(w.findAll('[data-testid="trace-row-skeleton"]').length).toBeGreaterThan(0)
    expect(w.findAll('[data-testid="trace-row"]').length).toBe(0)
    w.unmount()
  })

  it('renders an empty state when not loading and there are no traces', () => {
    const w = mount(TraceTable, { props: { traces: [], loading: false } })
    expect(w.text()).toContain('No traces match')
    w.unmount()
  })

  it('shows a red left-edge accent on rows with error_count > 0, none otherwise', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    const rows = w.findAll('[data-testid="trace-row"]')
    expect(rows[0].find('[data-testid="trace-error-accent"]').exists()).toBe(false) // t1: error_count 0
    expect(rows[1].find('[data-testid="trace-error-accent"]').exists()).toBe(true) // t2: error_count 2
    w.unmount()
  })

  it('renders a dedicated Service column showing the root service name', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    // t1.root_service is 'api', now shown as its own Service cell (not just a dot on the operation).
    expect(w.get('[data-trace-id="t1"]').text()).toContain('api')
    w.unmount()
  })

  it('adding an attribute column via the ColumnPicker emits columns-changed and renders root_attributes', async () => {
    const traces = [trace('t1', { root_attributes: { 'http.route': '/orders/:id' } })]
    const w = mount(TraceTable, {
      props: { traces, attrCatalog: [{ name: 'http.route', kind: 'attribute' }] },
      attachTo: document.body,
    })
    await settle()

    await openColumnPicker(w)
    await body().find('[data-test="col-toggle-http.route"]').trigger('click')
    await settle()

    expect(w.emitted('columns-changed')).toBeTruthy()
    expect(w.emitted('columns-changed').at(-1)[0]).toEqual(['http.route'])
    expect(w.text()).toContain('/orders/:id')
    w.unmount()
  })

  it('renders a placeholder for an added attribute column missing from root_attributes', async () => {
    const traces = [trace('t1')] // no root_attributes at all
    const w = mount(TraceTable, {
      props: { traces, attrCatalog: [{ name: 'http.route', kind: 'attribute' }] },
      attachTo: document.body,
    })
    await settle()

    await openColumnPicker(w)
    await body().find('[data-test="col-toggle-http.route"]').trigger('click')
    await settle()

    expect(w.text()).toContain('—')
    w.unmount()
  })

  it('hover action "Filter by service" emits toggle-value and does not open the row', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    const row = w.get('[data-trace-id="t2"]')
    await row.get('[data-testid="action-filter-service"]').trigger('click')
    expect(w.emitted('toggle-value')[0]).toEqual([{ field: 'service', value: 'api' }])
    expect(w.emitted('open-trace')).toBeUndefined()
    w.unmount()
  })

  it('hover action "Copy id" calls useCopy with the trace id', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    const row = w.get('[data-trace-id="t1"]')
    await row.get('[data-testid="action-copy-id"]').trigger('click')
    expect(copySpy).toHaveBeenCalledWith('t1', 'trace ID')
    expect(w.emitted('open-trace')).toBeUndefined()
    w.unmount()
  })

  it('hover action "Filter by service" fires on Enter/Space keydown, not the row-open handler', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    const row = w.get('[data-trace-id="t2"]')
    const btn = row.get('[data-testid="action-filter-service"]')
    await btn.trigger('keydown', { key: 'Enter' })
    expect(w.emitted('toggle-value')[0]).toEqual([{ field: 'service', value: 'api' }])
    await btn.trigger('keydown', { key: ' ' })
    expect(w.emitted('toggle-value').length).toBe(2)
    expect(w.emitted('open-trace')).toBeUndefined()
    w.unmount()
  })

  it('hover action "Copy id" fires on Enter/Space keydown, not the row-open handler', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    const row = w.get('[data-trace-id="t1"]')
    const btn = row.get('[data-testid="action-copy-id"]')
    await btn.trigger('keydown', { key: 'Enter' })
    expect(copySpy).toHaveBeenCalledWith('t1', 'trace ID')
    await btn.trigger('keydown', { key: ' ' })
    expect(copySpy).toHaveBeenCalledTimes(2)
    expect(w.emitted('open-trace')).toBeUndefined()
    w.unmount()
  })

  it('emits columns-changed with persisted attribute keys on mount, not just on a later toggle', async () => {
    localStorage.setItem('photon.cols.traces', JSON.stringify({ hidden: [], attrs: ['http.route'] }))
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    expect(w.emitted('columns-changed')).toBeTruthy()
    expect(w.emitted('columns-changed')[0][0]).toEqual(['http.route'])
    w.unmount()
  })

  it('highlights the row whose trace_id equals selectedId, not the others', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES, selectedId: 't2' }, attachTo: document.body })
    await settle()
    expect(w.get('[data-trace-id="t2"]').classes()).toContain('bg-muted')
    expect(w.get('[data-trace-id="t1"]').classes()).not.toContain('bg-muted')
    w.unmount()
  })

  it('does not highlight any row when selectedId is null', async () => {
    const w = mount(TraceTable, { props: { traces: TRACES }, attachTo: document.body })
    await settle()
    for (const row of w.findAll('[data-testid="trace-row"]')) {
      expect(row.classes()).not.toContain('bg-muted')
    }
    w.unmount()
  })

  it('does not re-scale duration bars as more traces append (pinned to the first page)', async () => {
    const shortTrace = trace('short', { duration_ns: 1_000_000n })
    const veryLongTrace = trace('long', { duration_ns: 500_000_000n })
    const w = mount(TraceTable, { props: { traces: [shortTrace] }, attachTo: document.body })
    await settle()
    const w1 = barWidth(w, 'short')
    expect(w1).toBe('width: 100%;') // only bar on the page -> pinned max is its own duration

    await w.setProps({ traces: [shortTrace, veryLongTrace] }) // simulates an appended page
    await settle()
    expect(barWidth(w, 'short')).toBe(w1) // pinned max unchanged -> bar width unchanged
    w.unmount()
  })

  it('re-pins the duration-bar max on a fresh search (different first row), not just on append', async () => {
    const shortTrace = trace('short', { duration_ns: 1_000_000n })
    const veryLongTrace = trace('long', { duration_ns: 500_000_000n })
    const w = mount(TraceTable, { props: { traces: [shortTrace, veryLongTrace] }, attachTo: document.body })
    await settle()
    const w1 = barWidth(w, 'short')
    expect(w1).not.toBe('width: 100%;') // pinned max is the 500ms trace, so the 1ms bar is tiny

    // Simulate the table/query swap on a fresh search: a page whose FIRST row is a different
    // trace (not an append, which always keeps the same first row).
    const freshOnlyTrace = trace('fresh', { duration_ns: 2_000_000n })
    await w.setProps({ traces: [freshOnlyTrace] })
    await settle()
    // Re-pinned to the new page's own max -> the lone bar fills 100%, not the old tiny width.
    expect(barWidth(w, 'fresh')).toBe('width: 100%;')
    w.unmount()
  })
})

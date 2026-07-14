import { describe, it, expect, vi, beforeAll, afterAll, beforeEach } from 'vitest'
import { nextTick } from 'vue'
import { mount, flushPromises, DOMWrapper } from '@vue/test-utils'
import SpanTable from './SpanTable.vue'

// Same virtualizer measurement stub as TraceTable.test.js: jsdom reports 0 for
// offsetHeight/offsetWidth, which makes @tanstack/vue-virtual compute an empty visible
// range for a zero-height viewport. Stub a real size so every row mounts.
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

const toastSpy = vi.fn()
vi.mock('@/components/ui/toast', () => ({ useToast: () => ({ toast: toastSpy }) }))

async function settle() {
  await flushPromises()
  await nextTick()
}

function span(id, overrides = {}) {
  return {
    trace_id: 'abc123def456abc123def456abc123ff',
    span_id: id,
    name: `GET /orders/${id}`,
    service: 'api',
    status_code: 0,
    start_time_nanos: 1_700_000_000_000_000_000n,
    duration_nanos: 5_000_000n,
    attributes: {},
    ...overrides,
  }
}

const SPANS = [
  span('s1'),
  span('s2', { duration_nanos: 20_000_000n, status_code: 2, service: 'checkout' }),
  span('s3', { attributes: { 'http.route': '/orders/:id' } }),
]

// selectedId + pinned-duration-bar fixtures.
const THREE_SPANS = SPANS
const SHORT_SPAN = span('short', { duration_nanos: 1_000_000n })
const VERY_LONG_SPAN = span('very-long', { duration_nanos: 500_000_000n })

function mountTable(props) {
  return mount(SpanTable, {
    props: { loading: false, attrCatalog: [], ...props },
    attachTo: document.body,
  })
}

// Reads the inline width style of a row's duration-bar fill, keyed off `span_id`.
function barWidth(wrapper, spanId) {
  return wrapper.get(`[data-span-id="${spanId}"] [data-testid="span-duration-bar"]`).attributes('style')
}

describe('SpanTable', () => {
  beforeEach(() => {
    localStorage.clear()
    toastSpy.mockClear()
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue() } })
  })

  it('renders one row per span with default columns', async () => {
    const w = mount(SpanTable, {
      props: { spans: SPANS, loading: false, attrCatalog: [] },
      attachTo: document.body,
    })
    await settle()
    const rows = w.findAll('[data-testid="span-row"]')
    expect(rows.length).toBe(SPANS.length)
    expect(w.text()).toContain('GET /orders/s1')
    expect(w.text()).toContain('api')
    expect(w.text()).toContain('checkout')
    w.unmount()
  })

  it('emits open-span with trace + span ids on row click', async () => {
    const w = mount(SpanTable, {
      props: { spans: SPANS, loading: false, attrCatalog: [] },
      attachTo: document.body,
    })
    await settle()
    await w.get('[data-span-id="s2"]').trigger('click')
    expect(w.emitted('open-span')[0][0]).toMatchObject({
      traceId: SPANS[1].trace_id,
      spanId: 's2',
      timeHintNs: SPANS[1].start_time_nanos,
    })
    w.unmount()
  })

  it('emits open-span on Enter/Space keydown for keyboard accessibility', async () => {
    const w = mount(SpanTable, {
      props: { spans: SPANS, loading: false, attrCatalog: [] },
      attachTo: document.body,
    })
    await settle()
    const row = w.get('[data-span-id="s1"]')
    await row.trigger('keydown', { key: 'Enter' })
    await row.trigger('keydown', { key: ' ' })
    expect(w.emitted('open-span').length).toBe(2)
    w.unmount()
  })

  it('renders an added attribute column from row.attributes', async () => {
    localStorage.setItem('photon.cols.spans', JSON.stringify({ hidden: [], attrs: ['http.route'] }))
    const w = mount(SpanTable, {
      props: {
        spans: SPANS,
        loading: false,
        // Raw catalog shape ([{ name, kind }]) — same as TraceTable.vue consumes from
        // `useTracesFields` — not a pre-shaped { key, label, group } picker entry.
        attrCatalog: [{ name: 'http.route', kind: 'attribute' }],
      },
      attachTo: document.body,
    })
    await settle()
    const rows = w.findAll('[data-testid="span-row"]')
    expect(rows[2].text()).toContain('/orders/:id')
    // Rows without the attribute fall back to an em dash placeholder.
    expect(rows[0].text()).toContain('—')
    w.unmount()
  })

  it('adding an attribute column via the ColumnPicker renders it, using the raw {name,kind} catalog shape', async () => {
    const w = mount(SpanTable, {
      props: {
        spans: SPANS,
        loading: false,
        attrCatalog: [
          { name: 'http.route', kind: 'attribute' },
          { name: 'service', kind: 'fixed' }, // collides with a built-in key — must be excluded
        ],
      },
      attachTo: document.body,
    })
    await settle()

    const trigger = w.findAll('button').find((b) => b.text() === 'Columns')
    await trigger.trigger('click')
    await nextTick()
    await new Promise((resolve) => setTimeout(resolve, 0))

    const body = new DOMWrapper(document.body)
    // "service" is already offered once as a built-in; the colliding fixed-kind catalog entry
    // must not add a second, duplicate picker row for the same key.
    expect(body.findAll('[data-test="col-toggle-service"]').length).toBe(1)
    await body.find('[data-test="col-toggle-http.route"]').trigger('click')
    await settle()

    const rows = w.findAll('[data-testid="span-row"]')
    expect(rows[2].text()).toContain('/orders/:id')
    w.unmount()
  })

  it('shows a red left-edge accent + error status on rows with status_code 2', async () => {
    const w = mount(SpanTable, {
      props: { spans: SPANS, loading: false, attrCatalog: [] },
      attachTo: document.body,
    })
    await settle()
    const errorRow = w.get('[data-span-id="s2"]')
    expect(errorRow.find('[data-testid="span-error-accent"]').exists()).toBe(true)
    const okRow = w.get('[data-span-id="s1"]')
    expect(okRow.find('[data-testid="span-error-accent"]').exists()).toBe(false)
    w.unmount()
  })

  it('hover action "Filter by service" emits toggle-value and does not bubble a row-open', async () => {
    const w = mount(SpanTable, {
      props: { spans: SPANS, loading: false, attrCatalog: [] },
      attachTo: document.body,
    })
    await settle()
    const row = w.get('[data-span-id="s2"]')
    await row.get('[data-testid="filter-by-service"]').trigger('click')
    expect(w.emitted('toggle-value')[0][0]).toEqual({ field: 'service', value: 'checkout' })
    expect(w.emitted('open-span')).toBeFalsy()
    w.unmount()
  })

  it('hover action "Copy id" copies the span id via useCopy', async () => {
    const w = mount(SpanTable, {
      props: { spans: SPANS, loading: false, attrCatalog: [] },
      attachTo: document.body,
    })
    await settle()
    const row = w.get('[data-span-id="s1"]')
    await row.get('[data-testid="copy-span-id"]').trigger('click')
    await flushPromises()
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('s1')
    expect(w.emitted('open-span')).toBeFalsy()
    w.unmount()
  })

  it('hover action "Filter by service" fires on Enter/Space keydown, not the row-open handler', async () => {
    const w = mount(SpanTable, {
      props: { spans: SPANS, loading: false, attrCatalog: [] },
      attachTo: document.body,
    })
    await settle()
    const row = w.get('[data-span-id="s2"]')
    const btn = row.get('[data-testid="filter-by-service"]')
    await btn.trigger('keydown', { key: 'Enter' })
    expect(w.emitted('toggle-value')[0][0]).toEqual({ field: 'service', value: 'checkout' })
    await btn.trigger('keydown', { key: ' ' })
    expect(w.emitted('toggle-value').length).toBe(2)
    expect(w.emitted('open-span')).toBeFalsy()
    w.unmount()
  })

  it('hover action "Copy id" fires on Enter/Space keydown, not the row-open handler', async () => {
    const w = mount(SpanTable, {
      props: { spans: SPANS, loading: false, attrCatalog: [] },
      attachTo: document.body,
    })
    await settle()
    const row = w.get('[data-span-id="s1"]')
    const btn = row.get('[data-testid="copy-span-id"]')
    await btn.trigger('keydown', { key: 'Enter' })
    await flushPromises()
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('s1')
    await btn.trigger('keydown', { key: ' ' })
    await flushPromises()
    expect(navigator.clipboard.writeText).toHaveBeenCalledTimes(2)
    expect(w.emitted('open-span')).toBeFalsy()
    w.unmount()
  })

  it('renders skeleton rows while loading and no span rows', () => {
    const w = mount(SpanTable, { props: { spans: [], loading: true, attrCatalog: [] } })
    expect(w.findAll('[data-testid="span-row-skeleton"]').length).toBeGreaterThan(0)
    expect(w.findAll('[data-testid="span-row"]').length).toBe(0)
    w.unmount()
  })

  it('renders an empty state when not loading and there are no spans', () => {
    const w = mount(SpanTable, { props: { spans: [], loading: false, attrCatalog: [] } })
    expect(w.text()).toContain('No spans match')
    w.unmount()
  })

  it('highlights the row whose span_id equals selectedId', async () => {
    const w = mountTable({ spans: THREE_SPANS, selectedId: THREE_SPANS[1].span_id })
    await settle()
    expect(w.get(`[data-span-id="${THREE_SPANS[1].span_id}"]`).classes()).toContain('bg-muted')
    // A row that isn't selected must not pick up the highlight.
    expect(w.get(`[data-span-id="${THREE_SPANS[0].span_id}"]`).classes()).not.toContain('bg-muted')
    w.unmount()
  })

  it('does not re-scale duration bars as more spans append', async () => {
    const w = mountTable({ spans: [SHORT_SPAN] })
    await settle()
    const w1 = barWidth(w, SHORT_SPAN.span_id)
    await w.setProps({ spans: [SHORT_SPAN, VERY_LONG_SPAN] })
    await settle()
    expect(barWidth(w, SHORT_SPAN.span_id)).toBe(w1)
    w.unmount()
  })

  it('re-pins the duration-bar max on a fresh search (different first row), not just on append', async () => {
    const w = mountTable({ spans: [SHORT_SPAN, VERY_LONG_SPAN] })
    await settle()
    const w1 = barWidth(w, SHORT_SPAN.span_id)
    expect(w1).not.toBe('width: 100%;') // pinned max is the very-long span, so the short bar is tiny

    // Fresh search: a page whose FIRST row is a different span (not an append, which always
    // keeps the same first row).
    const freshSpan = span('fresh', { duration_nanos: 2_000_000n })
    await w.setProps({ spans: [freshSpan] })
    await settle()
    // Re-pinned to the new page's own max -> the lone bar fills 100%, not the old tiny width.
    expect(barWidth(w, 'fresh')).toBe('width: 100%;')
    w.unmount()
  })
})

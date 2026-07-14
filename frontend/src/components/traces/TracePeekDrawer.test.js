import { describe, it, expect, vi, beforeEach, afterEach, beforeAll, afterAll } from 'vitest'
import { computed, nextTick, ref, toValue } from 'vue'
import { mount } from '@vue/test-utils'
import TracePeekDrawer from './TracePeekDrawer.vue'
import { Sheet } from '@/components/ui/sheet'
import { useTrace } from '@/lib/traces/tracesQueries'

// TracePeekDrawer owns its data-fetching (like TraceDetailView) — mock the HOOK directly so each
// test can hand back a canned { data, isFetching } ref pair with no QueryClient setup, matching
// the pattern in TracesQuickFilters.test.js.
vi.mock('@/lib/traces/tracesQueries', () => ({
  useTrace: vi.fn(),
}))

const copySpy = vi.fn()
vi.mock('@/lib/core/useCopy', () => ({
  useCopy: () => ({ copy: copySpy }),
}))

function span(id, parent, start, end, extra = {}) {
  return {
    span_id: id,
    parent_span_id: parent,
    start_time_nanos: BigInt(start),
    end_time_nanos: BigInt(end),
    duration_nanos: end - start,
    name: extra.name ?? `op-${id}`,
    service: extra.service ?? 'api',
    status_code: extra.status_code ?? 0,
    ...extra,
  }
}

// root 1000ms / slow 500ms / fast 50ms / fast2 10ms (same service as fast, so spanCount !=
// serviceCount) / err 50ms but ERROR — must sort to the top despite being far from the longest.
const SPANS = [
  span('root', null, 0, 1_000_000_000, { service: 'api', name: 'checkout' }),
  span('fast', 'root', 100_000_000, 150_000_000, { service: 'cache', name: 'get' }),
  span('fast2', 'fast', 110_000_000, 120_000_000, { service: 'cache', name: 'get2' }),
  span('slow', 'root', 200_000_000, 700_000_000, { service: 'db', name: 'query' }),
  span('err', 'root', 750_000_000, 800_000_000, {
    service: 'payments',
    name: 'charge',
    status_code: 2,
    status_text: 'ERROR',
    status_message: 'card declined',
  }),
]

// A healthy variant (drops the errored span) for asserting the error UI is absent when clean.
const HEALTHY_SPANS = SPANS.filter((s) => s.status_code !== 2)

// A large, otherwise-uninteresting trace (a root + N-1 children) purely to exercise the
// virtualizer's windowing — content doesn't matter here, only that it produces N distinct nodes.
function makeSpans(n) {
  const spans = [span('root', null, 0, 1_000_000_000, { service: 'api', name: 'root' })]
  for (let i = 1; i < n; i++) {
    spans.push(span(`s${i}`, 'root', i * 1000, i * 1000 + 500, { service: 'svc', name: `op-${i}` }))
  }
  return spans
}

// Reactive to the `traceId` arg the component passes in (a computed/ref, unwrapped via
// `toValue`) rather than a one-shot `mockReturnValue` — so tests can actually exercise what
// happens when that arg changes (or, per the identity-latch fix, doesn't change) after mount,
// the same way the real `useTrace`'s `enabled`/query-key reactivity would.
function mockTrace({ spans = SPANS, loading = false } = {}) {
  useTrace.mockImplementation((traceIdArg) => ({
    data: computed(() => {
      if (loading) return undefined
      const id = toValue(traceIdArg)
      return id ? { trace_id: id, spans } : undefined
    }),
    isFetching: ref(loading),
    error: ref(null),
  }))
}

async function flush() {
  await new Promise((r) => setTimeout(r, 0))
}

function mountDrawer(props) {
  return mount(TracePeekDrawer, {
    props: { traceId: 'trace-abc', spanId: null, timeHintNs: undefined, open: true, ...props },
    attachTo: document.body,
  })
}

// The span list virtualizes with @tanstack/vue-virtual, which sizes its visible range from the
// scroll element's offsetHeight. jsdom reports 0 there, and virtual-core returns an EMPTY range
// for a zero-height viewport — so stub a real height, same idiom as TraceTable.test.js. Kept
// comfortably bigger than the 5-span SPANS fixture (5 * 40px row estimate = 200px) so every
// existing test still finds all its rows, but far smaller than a large trace's total content so
// the windowing test actually windows.
let restoreH, restoreW
beforeAll(() => {
  restoreH = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight')
  restoreW = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth')
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, get: () => 400 })
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, get: () => 480 })
})
afterAll(() => {
  if (restoreH) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', restoreH)
  if (restoreW) Object.defineProperty(HTMLElement.prototype, 'offsetWidth', restoreW)
})

beforeEach(() => {
  useTrace.mockReset()
  copySpy.mockClear()
  Element.prototype.scrollIntoView = vi.fn()
})

// Sheet content is teleported to document.body, outside the wrapper's own root element — clear
// it defensively so a failed assertion in one test (which skips that test's own w.unmount()) can't
// leak stale DOM into the next test's document.body queries.
afterEach(() => {
  document.body.innerHTML = ''
})

describe('TracePeekDrawer', () => {
  it('renders a condensed summary from the built trace', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    const root = document.body.querySelector('[data-testid="peek-stat-root"]')
    expect(root.textContent).toContain('api')
    expect(root.textContent).toContain('checkout')
    expect(document.body.querySelector('[data-testid="peek-stat-spans"]').textContent).toContain('5')
    expect(document.body.querySelector('[data-testid="peek-stat-services"]').textContent).toContain('4')
    expect(document.body.querySelector('[data-testid="peek-stat-errors"]').textContent).toContain('1')
    expect(document.body.querySelector('[data-testid="peek-stat-duration"]')).toBeTruthy()
    expect(document.body.querySelector('[data-testid="peek-stat-started"]')).toBeTruthy()
    w.unmount()
  })

  it('orders the span list errored-first, then by duration desc', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    const rows = [...document.body.querySelectorAll('[data-testid="peek-span-row"]')]
    const ids = rows.map((r) => r.getAttribute('data-span-id'))
    // err (error, wins regardless of duration) -> root (1000ms) -> slow (500ms) -> fast (50ms) -> fast2 (10ms)
    expect(ids).toEqual(['err', 'root', 'slow', 'fast', 'fast2'])
    w.unmount()
  })

  // Regression guard for the virtualizer wiring: a large trace must NOT mount one DOM row per
  // span — only the on-screen slice (+ overscan) mounts. Mirrors the jsdom offsetHeight/offsetWidth
  // stub the table/waterfall tests use so the scroll element has a nonzero, measurable size.
  it('windows the span list for a large trace', async () => {
    mockTrace({ spans: makeSpans(500) })
    const w = mountDrawer()
    await flush()
    await nextTick()
    const rendered = document.body.querySelectorAll('[data-testid="peek-span-row"]')
    expect(rendered.length).toBeGreaterThan(0)
    expect(rendered.length).toBeLessThan(500)
    w.unmount()
  })

  it('pre-focuses and scrolls the row matching spanId', async () => {
    mockTrace()
    const w = mountDrawer({ spanId: 'slow' })
    await flush()
    const row = document.body.querySelector('[data-span-id="slow"]')
    expect(row.getAttribute('data-selected')).toBe('true')
    const other = document.body.querySelector('[data-span-id="root"]')
    expect(other.getAttribute('data-selected')).toBe('false')
    expect(Element.prototype.scrollIntoView).toHaveBeenCalled()
    w.unmount()
  })

  it('emits open-full with traceId and spanId', async () => {
    mockTrace()
    const w = mountDrawer({ spanId: 'slow' })
    await flush()
    const btn = document.body.querySelector('[data-testid="open-full"]')
    expect(btn).toBeTruthy()
    btn.click()
    await flush()
    expect(w.emitted('open-full')[0][0]).toEqual({ traceId: 'trace-abc', spanId: 'slow' })
    w.unmount()
  })

  it('copies the trace ID via useCopy from the header', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    const btn = document.body.querySelector('[data-testid="copy-trace-id"]')
    expect(btn).toBeTruthy()
    btn.click()
    await flush()
    expect(copySpy).toHaveBeenCalledWith('trace-abc', 'trace ID')
    w.unmount()
  })

  it('emits close when the sheet requests to close (X / Esc / overlay)', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    await w.findComponent(Sheet).vm.$emit('update:open', false)
    expect(w.emitted('close')).toBeTruthy()
    w.unmount()
  })

  it('shows a loading state while the trace is fetching', async () => {
    mockTrace({ loading: true })
    const w = mountDrawer()
    await flush()
    expect(document.body.querySelector('[data-testid="peek-skeleton"]')).toBeTruthy()
    w.unmount()
  })

  it('shows an empty state when the trace has no spans', async () => {
    mockTrace({ spans: [] })
    const w = mountDrawer()
    await flush()
    expect(document.body.querySelector('[data-testid="peek-stat-root"]')).toBeFalsy()
    expect(document.body.textContent).toContain('Trace not found')
    w.unmount()
  })

  // Regression: the fetch id used to be `props.open ? props.traceId : ''`, so the moment `open`
  // flipped false (as it does the instant the caller starts closing — see the planned
  // `@close="drawer = null"` wiring in TracesExplorer), useTrace's key collapsed to a
  // disabled/empty query and this drawer swapped its still-fading-out content to the "Trace not
  // found" empty state. The drawer now latches its identity internally, so closing must NOT swap
  // the content away — assert right after the props change (a single reactive flush), before
  // Reka's own close-animation bookkeeping (which jsdom, having no real CSS animations, resolves
  // by fully unmounting the drawer a tick later) removes the whole subtree for an unrelated
  // reason and would otherwise mask what this test is checking.
  it('keeps showing the trace content while closing, instead of flashing empty state', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    expect(document.body.querySelector('[data-testid="peek-span-list"]')).toBeTruthy()

    await w.setProps({ open: false })

    // Still latched on the last-shown trace — no flip to skeleton/empty state mid-close.
    expect(document.body.querySelector('[data-testid="peek-span-list"]')).toBeTruthy()
    expect(document.body.querySelector('[data-testid="peek-summary"]')).toBeTruthy()
    expect(document.body.textContent).not.toContain('Trace not found')
    w.unmount()
  })

  it('re-highlights and scrolls when spanId changes via setProps after mount', async () => {
    mockTrace()
    const w = mountDrawer({ spanId: 'root' })
    await flush()
    expect(document.body.querySelector('[data-span-id="root"]').getAttribute('data-selected')).toBe('true')

    Element.prototype.scrollIntoView.mockClear()
    await w.setProps({ spanId: 'slow' })
    await flush()

    const nowSelected = document.body.querySelector('[data-span-id="slow"]')
    const noLongerSelected = document.body.querySelector('[data-span-id="root"]')
    expect(nowSelected.getAttribute('data-selected')).toBe('true')
    expect(noLongerSelected.getAttribute('data-selected')).toBe('false')
    expect(Element.prototype.scrollIntoView).toHaveBeenCalled()
    w.unmount()
  })

  it('headline leads with duration + an errors pill', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    const duration = document.body.querySelector('[data-testid="peek-stat-duration"]')
    expect(duration).toBeTruthy()
    expect(duration.textContent.trim()).not.toBe('')
    const pill = document.body.querySelector('[data-testid="peek-stat-errors"]')
    expect(pill).toBeTruthy()
    expect(pill.textContent).toContain('1')
    expect(pill.textContent).toContain('error')
    w.unmount()
  })

  it('surfaces the primary error span text in a callout', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    const callout = document.body.querySelector('[data-testid="peek-error-callout"]')
    expect(callout).toBeTruthy()
    // service · name + the actual status message, not just a count.
    expect(callout.textContent).toContain('payments')
    expect(callout.textContent).toContain('charge')
    expect(callout.textContent).toContain('card declined')
    w.unmount()
  })

  it('hides the error callout and errors pill on a healthy trace', async () => {
    mockTrace({ spans: HEALTHY_SPANS })
    const w = mountDrawer()
    await flush()
    expect(document.body.querySelector('[data-testid="peek-error-callout"]')).toBeFalsy()
    expect(document.body.querySelector('[data-testid="peek-stat-errors"]')).toBeFalsy()
    w.unmount()
  })

  it('renders a status_message sub-line on errored span rows only', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    const errRow = document.body.querySelector('[data-span-id="err"]')
    expect(errRow.querySelector('[data-testid="peek-span-error-msg"]')).toBeTruthy()
    expect(errRow.textContent).toContain('card declined')
    // A healthy row stays single-line — no error sub-line.
    const rootRow = document.body.querySelector('[data-span-id="root"]')
    expect(rootRow.querySelector('[data-testid="peek-span-error-msg"]')).toBeFalsy()
    w.unmount()
  })

  // Regression guard for the virtualizer's row-height estimate: an error row with a
  // status_message renders a second line (the assertion above), so a flat single-line estimate
  // undersizes its virtual slot and lets the next row overlap it. `err` (errored, has
  // status_message) must get the taller reserved height; `root` (healthy, single-line) keeps the
  // base height.
  it('reserves a taller virtual row height for an error row with a status_message', async () => {
    mockTrace()
    const w = mountDrawer()
    await flush()
    const errRow = document.body.querySelector('[data-span-id="err"]')
    const rootRow = document.body.querySelector('[data-span-id="root"]')
    expect(errRow.style.height).toBe('56px')
    expect(rootRow.style.height).toBe('40px')
    w.unmount()
  })

  it('emits open-full on "o" and view-logs on "l" keyboard shortcuts', async () => {
    mockTrace()
    const w = mountDrawer({ timeHintNs: '42' })
    await flush()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'o' }))
    await flush()
    expect(w.emitted('open-full')).toBeTruthy()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'l' }))
    await flush()
    const viewLogs = w.emitted('view-logs')
    expect(viewLogs).toBeTruthy()
    expect(viewLogs[0][0]).toMatchObject({ traceId: 'trace-abc', timeHintNs: '42' })
    w.unmount()
  })
})

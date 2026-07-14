import { describe, it, expect, vi, beforeAll, afterAll } from 'vitest'
import { nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import TraceWaterfall from './TraceWaterfall.vue'

// jsdom doesn't implement Element.scrollTo (virtual-core's default scrollToFn), so a real
// scrollToIndex() call never actually moves scrollTop / re-renders the visible slice here. Wrap
// the (single, stable-for-the-instance's-lifetime) virtualizer's own `scrollToIndex` with a spy —
// delegating to the real implementation — so tests can assert it was invoked with the right index
// without depending on jsdom scroll simulation it doesn't have.
const scrollSpyHolder = vi.hoisted(() => ({ current: null }))
vi.mock('@tanstack/vue-virtual', async (importOriginal) => {
  const actual = await importOriginal()
  return {
    ...actual,
    useVirtualizer: (options) => {
      const state = actual.useVirtualizer(options)
      const inst = state.value
      const orig = inst.scrollToIndex
      const spy = vi.fn((...args) => orig(...args))
      inst.scrollToIndex = spy
      scrollSpyHolder.current = spy
      return state
    },
  }
})

// The span-rows list is virtualized with @tanstack/vue-virtual. virtual-core sizes its visible
// range from the scroll element's offsetHeight and returns an EMPTY range for a zero-height
// viewport — which is what jsdom reports — so stub a real height (bigger than the whole fixture)
// and let the virtualizer settle after the scroll element is measured.
let restoreH, restoreW, restoreCtx
// jsdom has no ResizeObserver (used by the minimap's useResizeObserver / the rows' useElementSize)
// and no canvas 2D context — stub both so mounting past the minimap threshold doesn't throw.
if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
}
beforeAll(() => {
  restoreH = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight')
  restoreW = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth')
  restoreCtx = HTMLCanvasElement.prototype.getContext
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, get: () => 1200 })
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, get: () => 800 })
  HTMLCanvasElement.prototype.getContext = () => null
})
afterAll(() => {
  if (restoreH) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', restoreH)
  if (restoreW) Object.defineProperty(HTMLElement.prototype, 'offsetWidth', restoreW)
  HTMLCanvasElement.prototype.getContext = restoreCtx
})

async function settle() {
  await flushPromises()
  await nextTick()
}

function span(id, parent, start, end, extra = {}) {
  return {
    span_id: id,
    parent_span_id: parent,
    start_time_nanos: BigInt(start),
    end_time_nanos: BigInt(end),
    duration_nanos: end - start,
    name: `op-${id}`,
    service: extra.service ?? 'api',
    status_code: extra.status_code ?? 0,
    ...extra,
  }
}

const SPANS = [
  span('root', null, 0, 1_000_000_000),
  span('child', 'root', 100_000_000, 400_000_000, { service: 'db' }),
  span('bad', 'root', 500_000_000, 700_000_000, { status_code: 2 }),
]

// Two 'db' matches, one of them (a1) nested under another (a), so a collapsed subtree still
// counts a match — pre-order flat is [root, a, a1, b].
const MATCH_SPANS = [
  span('root', null, 0, 1_000_000_000, { service: 'api' }),
  span('a', 'root', 0, 500_000_000, { service: 'db' }),
  span('a1', 'a', 0, 200_000_000, { service: 'db' }),
  span('b', 'root', 500_000_000, 900_000_000, { service: 'cache' }),
]

// A wide fixture (> the 50-row minimap threshold) so the minimap column renders.
const BIG_SPANS = [span('root', null, 0, 1_000_000_000)]
for (let i = 0; i < 60; i++) {
  BIG_SPANS.push(span('s' + i, 'root', i * 1_000_000, i * 1_000_000 + 500_000))
}

describe('TraceWaterfall', () => {
  it('renders one row per span', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    expect(w.findAll('[data-span-row]').length).toBe(3)
    w.unmount()
  })

  it('precomputes geometry onto each visible row', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    const rows = w.vm.openRows
    expect(rows.length).toBeGreaterThan(0)
    for (const r of rows) {
      expect(typeof r.barLeftPct).toBe('number')
      expect(typeof r.barWidthPct).toBe('number')
      expect(Array.isArray(r.selfInsets)).toBe(true)
      expect(Array.isArray(r.eventMarkers)).toBe(true)
      // `matches` was relocated off the geometry row (openRows) into the separate `isRowMatch`
      // helper (backed by `matchIds`) so filter typing doesn't recompute this geometry pass.
      expect(typeof w.vm.isRowMatch(r.id)).toBe('boolean')
      expect(typeof r.descendantCount).toBe('number')
    }
    w.unmount()
  })

  it('emits select-span with the span id on row click', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    await w.find('[data-span-row][data-span-id="child"]').trigger('click')
    expect(w.emitted('select-span')[0]).toEqual(['child'])
    w.unmount()
  })

  it('marks the error span row', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    const badBar = w.find('[data-span-id="bad"] [data-span-bar]')
    expect(badBar.classes()).toContain('bg-sev-error')
    w.unmount()
  })

  it('collapse-healthy hides healthy leaf branches but keeps error + critical paths', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS, collapseHealthy: true }, attachTo: document.body })
    await settle()
    const ids = w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))
    expect(ids).toContain('root')
    expect(ids).toContain('bad') // error span kept
    expect(ids).not.toContain('child') // healthy, off the critical path → hidden
    w.unmount()
  })

  it('renders an empty state when there are no spans', () => {
    const w = mount(TraceWaterfall, { props: { spans: [] } })
    expect(w.text()).toContain('No spans')
    w.unmount()
  })

  it('filter dims non-matching rows and updates the N of M count', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    await w.find('input[placeholder="Filter spans…"]').setValue('db')
    await settle()
    const childRow = w.find('[data-span-row][data-span-id="child"]')
    const rootRow = w.find('[data-span-row][data-span-id="root"]')
    expect(childRow.classes()).not.toContain('opacity-40') // matches (service "db")
    expect(rootRow.classes()).toContain('opacity-40') // no match → dimmed, not removed
    expect(w.text()).toContain('1 of 3 spans')
    w.unmount()
  })

  it('per-node collapse hides descendants of the collapsed span', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    await w
      .find('[data-span-row][data-span-id="root"] [data-testid="span-collapse-toggle"]')
      .trigger('click')
    await settle()
    const ids = w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))
    expect(ids).toEqual(['root'])
    w.unmount()
  })

  it('clicking the collapse chevron does not emit select-span', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    await w
      .find('[data-span-row][data-span-id="root"] [data-testid="span-collapse-toggle"]')
      .trigger('click')
    expect(w.emitted('select-span')).toBeUndefined()
    w.unmount()
  })

  it('j/k keyboard moves selection through visible rows', async () => {
    const w = mount(TraceWaterfall, {
      props: { spans: SPANS, selectedSpanId: 'root' },
      attachTo: document.body,
    })
    await settle()
    const rowsEl = w.find('[data-testid="waterfall-rows"]')
    await rowsEl.trigger('keydown', { key: 'j' })
    expect(w.emitted('select-span')[0]).toEqual(['child'])

    // TraceWaterfall is a controlled component — selectedSpanId only advances when the parent
    // feeds the emitted id back in, exactly as TraceDetailView's `@select-span` handler does.
    await w.setProps({ selectedSpanId: 'child' })
    await rowsEl.trigger('keydown', { key: 'j' })
    expect(w.emitted('select-span')[1]).toEqual(['bad'])

    await w.setProps({ selectedSpanId: 'bad' })
    await rowsEl.trigger('keydown', { key: 'k' })
    expect(w.emitted('select-span')[2]).toEqual(['child'])
    w.unmount()
  })

  it('match total counts whole-tree matches, including inside a collapsed subtree', async () => {
    const w = mount(TraceWaterfall, { props: { spans: MATCH_SPANS }, attachTo: document.body })
    await settle()
    // Collapse 'a' so its child 'a1' is hidden from the visible rows…
    await w
      .find('[data-span-row][data-span-id="a"] [data-testid="span-collapse-toggle"]')
      .trigger('click')
    await settle()
    expect(w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))).not.toContain('a1')
    // …yet the match total still counts a1 (whole-tree scan), and the first match is auto-selected.
    await w.find('input[placeholder="Filter spans…"]').setValue('db')
    await settle()
    expect(w.find('[data-testid="match-nav-count"]').text()).toBe('1 / 2')
    w.unmount()
  })

  it('n / N cycle through matches and wrap', async () => {
    const w = mount(TraceWaterfall, { props: { spans: MATCH_SPANS }, attachTo: document.body })
    await settle()
    await w.find('input[placeholder="Filter spans…"]').setValue('db')
    await settle()
    const rowsEl = w.find('[data-testid="waterfall-rows"]')
    const last = () => {
      const e = w.emitted('select-span')
      return e[e.length - 1]
    }
    expect(last()).toEqual(['a']) // filter auto-selects the first match

    await rowsEl.trigger('keydown', { key: 'n' })
    expect(last()).toEqual(['a1'])
    expect(w.find('[data-testid="match-nav-count"]').text()).toBe('2 / 2')

    await rowsEl.trigger('keydown', { key: 'n' }) // wraps back to the first
    expect(last()).toEqual(['a'])
    expect(w.find('[data-testid="match-nav-count"]').text()).toBe('1 / 2')

    await rowsEl.trigger('keydown', { key: 'N', shiftKey: true }) // previous wraps to the last
    expect(last()).toEqual(['a1'])
    w.unmount()
  })

  it('jumping into a collapsed subtree clears its ancestors collapse', async () => {
    const w = mount(TraceWaterfall, { props: { spans: MATCH_SPANS }, attachTo: document.body })
    await settle()
    await w.find('input[placeholder="Filter spans…"]').setValue('db')
    await settle()
    // Collapse 'a' — its matched child 'a1' is now hidden.
    await w
      .find('[data-span-row][data-span-id="a"] [data-testid="span-collapse-toggle"]')
      .trigger('click')
    await settle()
    expect(w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))).not.toContain('a1')
    // Jump to the next match (a1): revealing it must re-expand its ancestor 'a'.
    await w.find('[data-testid="waterfall-rows"]').trigger('keydown', { key: 'n' })
    await settle()
    const ids = w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))
    expect(ids).toContain('a1')
    w.unmount()
  })

  it('keeps a healthy match visible under collapse-healthy when a filter is active', async () => {
    const w = mount(TraceWaterfall, {
      props: { spans: SPANS, collapseHealthy: true },
      attachTo: document.body,
    })
    await settle()
    // Baseline: the healthy, off-critical-path 'child' is hidden by collapse-healthy.
    expect(w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))).not.toContain('child')
    // Filtering to it keeps it (and its ancestor chain) despite collapse-healthy.
    await w.find('input[placeholder="Filter spans…"]').setValue('db')
    await settle()
    const ids = w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))
    expect(ids).toContain('child')
    expect(ids).toContain('root')
    w.unmount()
  })

  it('renders the minimap only past the row threshold', async () => {
    const small = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    expect(small.find('[data-testid="trace-minimap"]').exists()).toBe(false)
    small.unmount()

    const big = mount(TraceWaterfall, { props: { spans: BIG_SPANS }, attachTo: document.body })
    await settle()
    expect(big.find('[data-testid="trace-minimap"]').exists()).toBe(true)
    big.unmount()
  })

  // --- initialSpanId (deep-link pre-selection) ---

  it('initialSpanId selects the row and scrolls to its open-row index on mount', async () => {
    // root(0) + s0..s59(1..60) — 's59' is open-row index 60.
    const w = mount(TraceWaterfall, {
      props: { spans: BIG_SPANS, initialSpanId: 's59' },
      attachTo: document.body,
    })
    await settle()
    expect(w.emitted('select-span')[0]).toEqual(['s59'])
    expect(scrollSpyHolder.current).toHaveBeenCalledWith(60, { align: 'center' })
    w.unmount()
  })

  it('reveals the ancestor chain and selects it when initialSpanId changes to a collapsed target', async () => {
    const w = mount(TraceWaterfall, { props: { spans: MATCH_SPANS }, attachTo: document.body })
    await settle()
    // Collapse 'a' — its child 'a1' is hidden.
    await w
      .find('[data-span-row][data-span-id="a"] [data-testid="span-collapse-toggle"]')
      .trigger('click')
    await settle()
    expect(w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))).not.toContain('a1')

    await w.setProps({ initialSpanId: 'a1' })
    await settle()

    const ids = w.findAll('[data-span-row]').map((r) => r.attributes('data-span-id'))
    expect(ids).toContain('a1') // ancestor 'a' un-collapsed to reveal it
    expect(w.emitted('select-span').at(-1)).toEqual(['a1'])
    w.unmount()
  })

  it('no-ops when initialSpanId is absent or unknown', async () => {
    const w = mount(TraceWaterfall, { props: { spans: SPANS }, attachTo: document.body })
    await settle()
    expect(w.emitted('select-span')).toBeUndefined()

    await w.setProps({ initialSpanId: 'does-not-exist' })
    await settle()
    expect(w.emitted('select-span')).toBeUndefined()
    w.unmount()
  })
})

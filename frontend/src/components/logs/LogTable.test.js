// LogTable is now backed by @tanstack/vue-table (row model) + @tanstack/vue-virtual (row
// windowing). It is pure-props — no vue-query — so no QueryClient/plugin is needed here.
//
// jsdom reports a 0x0 scroll viewport, so the virtualizer's outer size is 0 and it windows down to
// roughly `overscan` rows regardless of how many rows are passed. That's exactly what lets the
// "renders a SUBSET" assertion below be a meaningful, deterministic proxy for "windowing is on".
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import LogTable from './LogTable.vue'
import SeverityTag from './SeverityTag.vue'

// jsdom reports every element as 0x0, and the virtualizer renders NOTHING when its viewport height
// is 0 (it early-returns a null range). Give the scroll viewport a real height so the virtualizer
// computes a genuine visible window — a subset of the rows — which is what these tests assert on.
// Rows are left unmeasured (no measureElement ref), so they keep the 31px estimate; only the
// viewport size matters here.
const rectDescriptors = {}
beforeAll(() => {
  for (const [prop, value] of [
    ['offsetHeight', 300],
    ['offsetWidth', 800],
  ]) {
    rectDescriptors[prop] = Object.getOwnPropertyDescriptor(window.HTMLElement.prototype, prop)
    Object.defineProperty(window.HTMLElement.prototype, prop, { configurable: true, get: () => value })
  }
})
afterAll(() => {
  for (const prop of Object.keys(rectDescriptors)) {
    if (rectDescriptors[prop]) Object.defineProperty(window.HTMLElement.prototype, prop, rectDescriptors[prop])
    else delete window.HTMLElement.prototype[prop]
  }
})

const BASE_NS = 1_700_000_000_000_000_000n

function makeRow(i) {
  return {
    id: i,
    timestamp: BASE_NS + BigInt(i) * 1_000_000n,
    severity: i % 5 === 0 ? 'error' : 'info',
    service: `svc-${i % 3}`,
    body: `log message ${i}`,
    attributes: { 'http.method': i % 2 === 0 ? 'POST' : 'GET' },
  }
}

const ROWS = Array.from({ length: 200 }, (_, i) => makeRow(i))

async function settle() {
  await nextTick()
  await flushPromises()
  await nextTick()
}

describe('LogTable (virtualized)', () => {
  it('windows the rows — only a subset is in the DOM, not all 200', async () => {
    const w = mount(LogTable, { props: { rows: ROWS }, attachTo: document.body })
    await settle()
    const rendered = w.findAll('[role="option"]')
    expect(rendered.length).toBeGreaterThan(0)
    expect(rendered.length).toBeLessThan(ROWS.length)
    w.unmount()
  })

  it('renders a fixed header regardless of the windowed rows', async () => {
    const w = mount(LogTable, { props: { rows: ROWS } })
    await settle()
    expect(w.text()).toContain('Time')
    expect(w.text()).toContain('Level')
    expect(w.text()).toContain('Service')
    expect(w.text()).toContain('Message')
    w.unmount()
  })

  it('moves selection with j/k/arrow keys and emits select', async () => {
    const w = mount(LogTable, { props: { rows: ROWS } })
    await settle()
    const list = w.find('[role="listbox"]')

    // No selection yet → ArrowDown selects the first row.
    await list.trigger('keydown', { key: 'ArrowDown' })
    expect(w.emitted('select')[0]).toEqual([ROWS[0].id])

    // With row 0 selected, `j` advances to row 1.
    await w.setProps({ selectedId: ROWS[0].id })
    await list.trigger('keydown', { key: 'j' })
    expect(w.emitted('select')[1]).toEqual([ROWS[1].id])

    // `k` / ArrowUp steps back to row 0.
    await w.setProps({ selectedId: ROWS[1].id })
    await list.trigger('keydown', { key: 'k' })
    expect(w.emitted('select')[2]).toEqual([ROWS[0].id])
    w.unmount()
  })

  it('emits select for the current selection on Enter', async () => {
    const w = mount(LogTable, { props: { rows: ROWS, selectedId: ROWS[3].id } })
    await settle()
    await w.find('[role="listbox"]').trigger('keydown', { key: 'Enter' })
    expect(w.emitted('select')).toBeTruthy()
    expect(w.emitted('select')[0]).toEqual([ROWS[3].id])
    w.unmount()
  })

  it('marks the selected row with aria-selected and emits select on row click', async () => {
    const w = mount(LogTable, { props: { rows: ROWS, selectedId: ROWS[0].id } })
    await settle()
    const first = w.find('[data-row-id="0"]')
    expect(first.attributes('aria-selected')).toBe('true')
    await first.trigger('click')
    expect(w.emitted('select')[0]).toEqual([0])
    w.unmount()
  })

  it('emits filter-severity (not select) when the severity tag is clicked', async () => {
    const w = mount(LogTable, { props: { rows: ROWS } })
    await settle()
    // Row 0 has severity 'error'. The SeverityTag wrapper carries @click.stop.
    const sevWrapper = w.find('[data-row-id="0"] span.cursor-pointer')
    await sevWrapper.trigger('click')
    expect(w.emitted('filter-severity')[0]).toEqual(['error'])
    // @click.stop keeps the row-select from firing off the same click.
    expect(w.emitted('select')).toBeFalsy()
    w.unmount()
  })

  it('renders one grid track + cell per configured attribute column', async () => {
    const w = mount(LogTable, {
      props: { rows: ROWS.slice(0, 5), columns: ['http.method'] },
    })
    await settle()
    // The attribute name appears as a header track…
    expect(w.text()).toContain('http.method')
    // …and its value renders in the row cells (row 0's http.method is POST).
    expect(w.text()).toContain('POST')
    w.unmount()
  })

  it('renders SeverityTag for each windowed row', async () => {
    const w = mount(LogTable, { props: { rows: ROWS } })
    await settle()
    const tags = w.findAllComponents(SeverityTag)
    const options = w.findAll('[role="option"]')
    expect(tags.length).toBe(options.length)
    expect(options.length).toBeGreaterThan(0)
    w.unmount()
  })

  it('shows the empty state when there are no rows and not loading', async () => {
    const w = mount(LogTable, { props: { rows: [], loading: false } })
    await settle()
    expect(w.text()).toContain('No logs match')
    w.unmount()
  })
})

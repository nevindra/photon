import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import LogDetailDrawer from './LogDetailDrawer.vue'

async function flush() {
  await new Promise((r) => setTimeout(r, 0))
}

const rowWithTrace = {
  id: 1,
  timestamp: 1_700_000_000_000_000_000n,
  service: 'api',
  severity: 'error',
  body: 'boom',
  trace_id: 'abc123',
  span_id: 's1',
  attributes: { region: 'us-east-1' },
}

// The drawer teleports its content into document.body (Reka Sheet), so tests query the body.
beforeEach(() => {
  Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue() } })
})

describe('LogDetailDrawer — view trace', () => {
  it('shows a View trace action and emits view-trace with a time hint', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    const btn = document.body.querySelector('[data-testid="view-trace"]')
    expect(btn).toBeTruthy()
    btn.click()
    await flush()
    expect(w.emitted('view-trace')[0][0]).toEqual({
      traceId: 'abc123',
      timeHintNs: '1700000000000000000',
    })
    w.unmount()
  })

  it('omits the View trace action when the row has no trace_id', async () => {
    const row = { ...rowWithTrace, trace_id: null }
    const w = mount(LogDetailDrawer, { props: { row, open: true }, attachTo: document.body })
    await flush()
    expect(document.body.querySelector('[data-testid="view-trace"]')).toBeFalsy()
    w.unmount()
  })

  it('emits view-trace from the trace_id field jump', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    const jump = document.body.querySelector('[data-testid="field-jump-trace_id"]')
    expect(jump).toBeTruthy()
    jump.click()
    await flush()
    expect(w.emitted('view-trace')[0][0]).toEqual({
      traceId: 'abc123',
      timeHintNs: '1700000000000000000',
    })
    w.unmount()
  })
})

describe('LogDetailDrawer — message hero', () => {
  it('renders the message body as the hero', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    const hero = document.body.querySelector('[data-testid="log-message"]')
    expect(hero).toBeTruthy()
    expect(hero.textContent).toContain('boom')
    w.unmount()
  })

  it('the copy-message button copies the body', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    document.body.querySelector('[data-testid="copy-message"]').click()
    await flush()
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('boom')
    w.unmount()
  })

  it('the `c` shortcut (forwarded by PeekDrawer) copies the message', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    // PeekDrawer binds a window keydown listener while open and forwards non-nav keys via @shortcut.
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'c' }))
    await flush()
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('boom')
    w.unmount()
  })
})

describe('LogDetailDrawer — field actions', () => {
  it('filter-in (+) emits filter-value with negate:false, mapping service.name → service', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    document.body.querySelector('[data-testid="filter-in-service.name"]').click()
    await flush()
    expect(w.emitted('filter-value')[0][0]).toEqual({ field: 'service', value: 'api', negate: false })
    w.unmount()
  })

  it('filter-out (−) emits filter-value with negate:true', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    document.body.querySelector('[data-testid="filter-out-service.name"]').click()
    await flush()
    expect(w.emitted('filter-value')[0][0]).toEqual({ field: 'service', value: 'api', negate: true })
    w.unmount()
  })

  it('an attribute row filters by its own name', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    document.body.querySelector('[data-testid="filter-in-region"]').click()
    await flush()
    expect(w.emitted('filter-value')[0][0]).toEqual({
      field: 'region',
      value: 'us-east-1',
      negate: false,
    })
    w.unmount()
  })

  it('the per-field copy button copies that value', async () => {
    const w = mount(LogDetailDrawer, { props: { row: rowWithTrace, open: true }, attachTo: document.body })
    await flush()
    document.body.querySelector('[data-testid="copy-value-region"]').click()
    await flush()
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('us-east-1')
    w.unmount()
  })
})

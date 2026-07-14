import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import SpanDetailPanel from './SpanDetailPanel.vue'

const { copySpy } = vi.hoisted(() => ({ copySpy: vi.fn() }))
vi.mock('@/lib/core/useCopy', () => ({ useCopy: () => ({ copy: copySpy }) }))

const SPAN = {
  trace_id: 't1',
  span_id: 's5',
  parent_span_id: 's2',
  name: 'charge.card',
  kind: 3,
  kind_text: 'CLIENT',
  start_time_nanos: 1_700_000_000_000_000_000n,
  end_time_nanos: 1_700_000_000_085_000_000n,
  duration_nanos: 85_000_000,
  status_code: 2,
  status_text: 'ERROR',
  status_message: 'card declined',
  scope_name: 'payments',
  service: 'payments',
  events: [{ name: 'retry', time_unix_nano: '1700000000040000000', attributes: {} }],
  links: null,
  attributes: { 'http.method': 'POST', 'net.peer.name': 'gw.example' },
}

async function clickTab(w, label) {
  const tabs = w.findAll('[role="tab"]')
  const tab = tabs.find((t) => t.text() === label)
  await tab.trigger('click')
}

describe('SpanDetailPanel', () => {
  it('renders nothing when span is null', () => {
    const w = mount(SpanDetailPanel, { props: { span: null, traceId: 't1' } })
    expect(w.find('[data-testid="span-panel"]').exists()).toBe(false)
  })

  it('shows the span name, service, duration by default (Overview tab)', () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    expect(w.text()).toContain('charge.card')
    expect(w.text()).toContain('payments')
    expect(w.text()).toContain('85.0ms')
  })

  it('shows the error status callout regardless of active tab', () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    expect(w.text()).toContain('card declined')
  })

  it('shows attributes only after switching to the Attributes tab', async () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    expect(w.text()).not.toContain('http.method')
    await clickTab(w, 'Attributes')
    expect(w.text()).toContain('http.method')
  })

  it('shows the raw JSON (BigInt-safe) after switching to the Raw tab', async () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await clickTab(w, 'Raw')
    expect(w.text()).toContain('span_id')
    expect(w.text()).toContain('s5')
  })

  it('renders the self-time row on Overview when a node is provided', () => {
    const node = {
      span: SPAN,
      selfTimeNs: 40_000_000n,
      startNs: SPAN.start_time_nanos,
      endNs: SPAN.end_time_nanos,
      durationNs: SPAN.end_time_nanos - SPAN.start_time_nanos,
      children: [
        {
          span: { span_id: 'c1', name: 'db.query', service: 'payments-db' },
          startNs: SPAN.start_time_nanos + 10_000_000n,
          endNs: SPAN.start_time_nanos + 45_000_000n,
          durationNs: 35_000_000n,
          isError: false,
        },
      ],
    }
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1', node } })
    expect(w.text()).toContain('40.0ms')
  })

  it('emits view-logs with a trace_id + span_id query', async () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await w.find('[data-testid="view-logs"]').trigger('click')
    expect(w.emitted('view-logs')[0]).toEqual([{ query: 'trace_id:t1 span_id:s5' }])
  })

  it('emits close', async () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await w.find('[data-testid="close-panel"]').trigger('click')
    expect(w.emitted('close')).toBeTruthy()
  })

  it('copies the span ID via useCopy', async () => {
    copySpy.mockClear()
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    const btn = w.findAll('button').find((b) => b.text().includes('Copy span ID'))
    await btn.trigger('click')
    expect(copySpy).toHaveBeenCalledWith('s5', 'span ID')
  })

  it('copies the raw span JSON via useCopy', async () => {
    copySpy.mockClear()
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await clickTab(w, 'Raw')
    const btn = w.findAll('button').find((b) => b.text().includes('Copy JSON'))
    await btn.trigger('click')
    expect(copySpy).toHaveBeenCalledWith(expect.stringContaining('"span_id": "s5"'), 'span JSON')
  })

  it('resets to the Overview tab when the selected span changes', async () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await clickTab(w, 'Raw')
    expect(w.find('[data-test="tabpanel-raw"]').exists()).toBe(true)
    await w.setProps({ span: { ...SPAN, span_id: 's9' } })
    expect(w.find('[data-test="tabpanel-overview"]').exists()).toBe(true)
  })

  it('moves between tabs with arrow keys (roving tabindex)', async () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await w.find('[data-test="tab-overview"]').trigger('keydown', { key: 'ArrowRight' })
    expect(w.find('[data-test="tab-attributes"]').attributes('tabindex')).toBe('0')
    expect(w.find('[data-test="tab-overview"]').attributes('tabindex')).toBe('-1')
  })

  it('wraps around and jumps to the ends with Home/End', async () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await w.find('[data-test="tab-overview"]').trigger('keydown', { key: 'ArrowLeft' })
    expect(w.find('[data-test="tab-raw"]').attributes('tabindex')).toBe('0')
    await w.find('[data-test="tab-raw"]').trigger('keydown', { key: 'Home' })
    expect(w.find('[data-test="tab-overview"]').attributes('tabindex')).toBe('0')
    await w.find('[data-test="tab-overview"]').trigger('keydown', { key: 'End' })
    expect(w.find('[data-test="tab-raw"]').attributes('tabindex')).toBe('0')
  })

  it('copies an attribute value from its per-row copy button', async () => {
    copySpy.mockClear()
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await clickTab(w, 'Attributes')
    await w.find('[data-test="attr-copy-http.method"]').trigger('click')
    expect(copySpy).toHaveBeenCalledWith('POST', 'http.method')
  })

  it('toggles an attribute value between truncated and expanded on click', async () => {
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await clickTab(w, 'Attributes')
    const valueBtn = w.findAll('button').find((b) => b.text() === 'POST')
    expect(valueBtn.classes()).toContain('truncate')
    await valueBtn.trigger('click')
    expect(valueBtn.classes()).toContain('whitespace-pre-wrap')
    await valueBtn.trigger('click')
    expect(valueBtn.classes()).toContain('truncate')
  })

  it('copies an identity value (e.g. span_id) from its per-row copy button', async () => {
    copySpy.mockClear()
    const w = mount(SpanDetailPanel, { props: { span: SPAN, traceId: 't1' } })
    await w.find('[data-test="attr-copy-span_id"]').trigger('click')
    expect(copySpy).toHaveBeenCalledWith('s5', 'span_id')
  })
})

import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricQuickStarts from './MetricQuickStarts.vue'

describe('MetricQuickStarts', () => {
  it('renders a card for each present curated metric and emits apply on click', async () => {
    const w = mount(MetricQuickStarts, { props: { catalog: [{ name: 'http_requests_total' }] } })
    const cards = w.findAll('[data-testid="quickstart-card"]')
    expect(cards.length).toBeGreaterThanOrEqual(1)
    await cards[0].trigger('click')
    const payload = w.emitted('apply')?.[0]?.[0] as { metric: string; agg: string }
    expect(payload.metric).toBe('http_requests_total')
    expect(payload.agg).toBe('rate')
  })
  it('renders no cards when no curated metric is present', () => {
    const w = mount(MetricQuickStarts, { props: { catalog: [{ name: 'custom.only' }] } })
    expect(w.findAll('[data-testid="quickstart-card"]').length).toBe(0)
  })
})

import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricCatalog from './MetricCatalog.vue'

const ENTRIES = [
  { name: 'http.server.requests', type: 'sum', unit: '1', series_count: 860, last_seen: '0' },
  { name: 'process.cpu.utilization', type: 'gauge', unit: '1', series_count: 48, last_seen: '0' },
  { name: 'http.server.duration', type: 'histogram', unit: 'ms', series_count: 1200, last_seen: '0' },
]

describe('MetricCatalog', () => {
  it('lists all entries and filters by the search box', async () => {
    const w = mount(MetricCatalog, { props: { entries: ENTRIES } })
    expect(w.findAll('[data-testid="catalog-row"]')).toHaveLength(3)
    await w.get('[data-testid="catalog-search"]').setValue('http')
    expect(w.findAll('[data-testid="catalog-row"]')).toHaveLength(2)
  })
  it('emits open with the metric name on row click', async () => {
    const w = mount(MetricCatalog, { props: { entries: ENTRIES } })
    await w.findAll('[data-testid="catalog-row"]')[0].trigger('click')
    expect(w.emitted('open')[0]).toEqual(['http.server.requests'])
  })
})

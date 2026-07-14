import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricMetaPanel from './MetricMetaPanel.vue'

const META = {
  name: 'http.server.duration', type: 'histogram', temporality: 'cumulative', unit: 'ms',
  is_monotonic: null, series_count: 1204, last_seen: '0', attribute_keys: ['service', 'http.route'],
}

describe('MetricMetaPanel', () => {
  it('renders name, type/temporality, cardinality and attribute chips', () => {
    const w = mount(MetricMetaPanel, { props: { metadata: META } })
    expect(w.text()).toContain('http.server.duration')
    expect(w.text()).toContain('histogram')
    expect(w.text()).toContain('cumulative')
    expect(w.text()).toContain('1,204')
    expect(w.findAll('[data-testid="attr-chip"]')).toHaveLength(2)
  })
  it('emits view-exemplars on the CTA', async () => {
    const w = mount(MetricMetaPanel, { props: { metadata: META } })
    await w.get('[data-testid="view-exemplars"]').trigger('click')
    expect(w.emitted('view-exemplars')).toBeTruthy()
  })
  it('shows nothing meaningful when metadata is null', () => {
    const w = mount(MetricMetaPanel, { props: { metadata: null } })
    expect(w.find('[data-testid="meta-empty"]').exists()).toBe(true)
  })
})

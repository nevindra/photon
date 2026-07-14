import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ChartPanel from '../ChartPanel.vue'

describe('ChartPanel', () => {
  it('renders the title and default body slot', () => {
    const w = mount(ChartPanel, { props: { title: 'Latency' }, slots: { default: '<p>body</p>' } })
    expect(w.text()).toContain('Latency')
    expect(w.html()).toContain('<p>body</p>')
  })

  it('renders the subtitle when provided', () => {
    const w = mount(ChartPanel, { props: { title: 'Volume', subtitle: 'drag to zoom' } })
    expect(w.text()).toContain('drag to zoom')
  })

  it('renders the summary slot when provided', () => {
    const w = mount(ChartPanel, { props: { title: 'Volume' }, slots: { summary: '<span>1,820</span>' } })
    expect(w.html()).toContain('<span>1,820</span>')
  })

  it('omits the summary region when no summary slot is passed', () => {
    const w = mount(ChartPanel, { props: { title: 'Volume' } })
    expect(w.find('[data-test="chart-panel-summary"]').exists()).toBe(false)
  })
})

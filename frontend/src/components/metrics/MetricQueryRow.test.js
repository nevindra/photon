import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { TooltipProvider } from '@/components/ui/tooltip'
import MetricQueryRow from './MetricQueryRow.vue'

const CATALOG = [{ name: 'http.server.requests', type: 'sum', unit: '1', series_count: 1, last_seen: '0' }]

function mountRow(props = {}) {
  return mount(
    { components: { TooltipProvider, MetricQueryRow }, template: '<TooltipProvider><MetricQueryRow v-bind="$attrs" /></TooltipProvider>' },
    { attrs: {
      metric: 'http.server.requests', catalog: CATALOG, agg: null, defaultAgg: 'rate',
      groupBy: [], filter: '', metricType: 'sum', isMonotonic: true,
      attributeKeys: ['service', 'http.route'], services: ['checkout'], ...props,
    },
    // Reka's DropdownMenu teleports its content to document.body, so the agg dropdown test
    // needs the wrapper attached there (mirrors NavRail.test.js / RelatedMenu.test.ts).
    attachTo: document.body },
  )
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

describe('MetricQueryRow', () => {
  it('shows the query badge A and the auto badge when agg is null', () => {
    const w = mountRow()
    expect(w.get('[data-testid="query-badge"]').text()).toBe('A')
    expect(w.find('[data-testid="agg-auto-badge"]').exists()).toBe(true)
    expect(w.text()).toContain('rate') // the default agg value is displayed
  })
  it('renders disabled power-tool footer placeholders', () => {
    const w = mountRow()
    expect(w.get('[data-testid="add-query"]').attributes('disabled')).toBeDefined()
    expect(w.get('[data-testid="raw-sql"]').attributes('disabled')).toBeDefined()
  })
  it('emits update:groupBy when a group-by option is chosen', async () => {
    const w = mountRow()
    // Group-by is a native <select> (jsdom does not fire change events from clicking <option>
    // elements, so we drive it via setValue rather than clicking the option per plan Step 1's note).
    const select = w.get('[data-testid="groupby-trigger"] select')
    await select.setValue('service')
    const row = w.findComponent(MetricQueryRow)
    expect(row.emitted('update:groupBy')[0]).toEqual([['service']])
  })
  it('disables group-by for summary metrics', () => {
    const w = mountRow({ metricType: 'summary' })
    const select = w.get('[data-testid="groupby-trigger"] select')
    expect(select.attributes('disabled')).toBeDefined()
  })
  it('offers p50/p90/p99 for histogram metrics', async () => {
    const w = mountRow({ metricType: 'histogram' })
    // Open the agg dropdown; Reka teleports the menu content to document.body, so query there
    // (not `w.find`, which only searches the wrapper's own subtree) once it has rendered.
    await w.get('[data-testid="agg-trigger"]').trigger('click')
    await flush()
    for (const id of ['p99', 'p90', 'p50', 'count', 'sum', 'avg']) {
      expect(document.body.querySelector(`[data-testid="agg-option-${id}"]`)).toBeTruthy()
    }
    w.unmount()
  })
})

describe('MetricQueryRow viz + presets', () => {
  const base = {
    metric: 'http.server.duration', metricType: 'histogram', isMonotonic: null,
    catalog: [], agg: null, defaultAgg: 'p99', groupBy: [], filter: '',
    attributeKeys: [], services: [], viz: 'line', seriesCount: 1,
  }
  it('emits update:viz from the switcher', async () => {
    const w = mountRow(base)
    await w.find('[data-testid="viz-opt-bar"]').trigger('click')
    const row = w.findComponent(MetricQueryRow)
    expect(row.emitted('update:viz')?.[0]?.[0]).toBe('bar')
  })
  it('applies a preset agg via update:agg', async () => {
    const w = mountRow(base)
    await w.findAll('[data-testid="preset-chip"]')[0].trigger('click')
    const row = w.findComponent(MetricQueryRow)
    expect(row.emitted('update:agg')?.[0]?.[0]).toBe('p99')
  })
})

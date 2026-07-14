import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricVizSwitcher from './MetricVizSwitcher.vue'
import { TooltipProvider } from '@/components/ui/tooltip'

const allViz = [
  { id: 'line', label: 'Line' }, { id: 'bar', label: 'Bar' }, { id: 'stat', label: 'Stat' },
]

// Reka's Tooltip requires a TooltipProvider ancestor (see MetricQueryRow.test.js for the
// same pattern) — the component itself stays exactly as specified in the plan.
function mountSwitcher(props: Record<string, unknown>) {
  return mount(
    { components: { TooltipProvider, MetricVizSwitcher }, template: '<TooltipProvider><MetricVizSwitcher v-bind="$attrs" /></TooltipProvider>' },
    { attrs: props },
  )
}

describe('MetricVizSwitcher', () => {
  it('emits update:modelValue when an available viz is clicked', async () => {
    const w = mountSwitcher({ modelValue: 'line', available: ['line', 'bar', 'stat'], allViz })
    await w.find('[data-testid="viz-opt-bar"]').trigger('click')
    const switcher = w.findComponent(MetricVizSwitcher)
    expect(switcher.emitted('update:modelValue')?.[0]?.[0]).toBe('bar')
  })
  it('disables an option that is not available and does not emit for it', async () => {
    const w = mountSwitcher({ modelValue: 'line', available: ['line', 'bar'], allViz })
    const stat = w.find('[data-testid="viz-opt-stat"]')
    expect(stat.attributes('disabled')).toBeDefined()
    await stat.trigger('click')
    const switcher = w.findComponent(MetricVizSwitcher)
    expect(switcher.emitted('update:modelValue')).toBeFalsy()
  })
})

import { describe, it, expect, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { TooltipProvider } from '@/components/ui/tooltip'
import ContextBar from './ContextBar.vue'
import { scope, timeRange } from '@/lib/core/context'

const wrap = (props = {}) =>
  mount(
    {
      components: { ContextBar, TooltipProvider },
      template: '<TooltipProvider><ContextBar v-bind="$attrs" /></TooltipProvider>',
    },
    { attrs: props, attachTo: document.body },
  )

// Variant that fills the middle `search` slot, mirroring how AppShell forwards a view's SearchBar.
const wrapWithSearch = (props = {}) =>
  mount(
    {
      components: { ContextBar, TooltipProvider },
      template:
        '<TooltipProvider><ContextBar v-bind="$attrs"><template #search><div data-testid="search-slot">search here</div></template></ContextBar></TooltipProvider>',
    },
    { attrs: props, attachTo: document.body },
  )

beforeEach(() => {
  scope.value = null
  timeRange.value = '30m'
})

describe('ContextBar', () => {
  it('shows the scope chip only when a scope is set, and clears it on ✕', async () => {
    scope.value = { type: 'service', id: 'checkout', label: 'checkout' }
    const w = wrap({ crumb: 'Backend' })
    const chip = w.get('[data-testid="scope-chip"]')
    expect(chip.text()).toContain('checkout')
    await w.get('[data-testid="scope-clear"]').trigger('click')
    expect(scope.value).toBeNull()
  })

  it('hides the scope chip when scope is null', () => {
    const w = wrap({ crumb: 'Home · Overview' })
    expect(w.find('[data-testid="scope-chip"]').exists()).toBe(false)
    expect(w.text()).toContain('Home · Overview')
  })

  it('renders the middle `search` slot when a searchable view forwards one', () => {
    const w = wrapWithSearch({ crumb: 'Logs' })
    expect(w.find('[data-testid="search-slot"]').exists()).toBe(true)
    expect(w.text()).toContain('Logs')
  })

  it('renders without a search slot (non-searchable views leave the middle empty)', () => {
    const w = wrap({ crumb: 'Metrics' })
    expect(w.find('[data-testid="search-slot"]').exists()).toBe(false)
    expect(w.text()).toContain('Metrics')
  })
})

import { describe, it, expect, afterEach } from 'vitest'
import { mount, flushPromises, DOMWrapper } from '@vue/test-utils'
import MetricPicker from './MetricPicker.vue'

const catalog = [
  { name: 'http.server.duration', type: 'histogram', unit: 'ms' },
  { name: 'http.server.requests', type: 'sum' },
  { name: 'cpu.usage', type: 'gauge', unit: '%' },
]

// PopoverContent is teleported to document.body (see ui/popover/PopoverContent.vue, which wraps
// Reka's PopoverContent in a PopoverPortal) and only renders once the popover is open, so tests
// open the trigger first, then query the body rather than the component wrapper — same pattern as
// TimeRangePicker.test.js.
const body = () => new DOMWrapper(document.body)

let wrapper

afterEach(() => {
  wrapper?.unmount()
  wrapper = undefined
})

async function openPicker(props = {}) {
  wrapper = mount(MetricPicker, {
    props: { modelValue: '', catalog, favorites: [], recent: [], ...props },
    attachTo: document.body,
  })
  await wrapper.find('[data-testid="metric-picker-trigger"]').trigger('click')
  await flushPromises()
  return wrapper
}

describe('MetricPicker', () => {
  it('renders Favorites and Recent sections from props', async () => {
    await openPicker({ favorites: ['cpu.usage'], recent: ['http.server.requests'] })
    expect(body().html()).toContain('cpu.usage')
    expect(body().html()).toContain('http.server.requests')
  })
  it('emits update:modelValue with a real catalog metric name when chosen', async () => {
    await openPicker()
    await body().findAll('[data-testid="metric-option"]')[0].trigger('click')
    const emitted = wrapper.emitted('update:modelValue')
    expect(emitted).toBeTruthy()
    expect(catalog.map((c) => c.name)).toContain(emitted[0][0])
  })
  it('narrows the option list to metrics matching the search query', async () => {
    await openPicker()
    await body().find('[data-testid="metric-picker-search"]').setValue('cpu')
    await flushPromises()
    const names = body().findAll('[data-testid="metric-option"]').map((o) => o.text())
    expect(names.some((t) => t.includes('cpu.usage'))).toBe(true)
    expect(names.some((t) => t.includes('http.server.duration'))).toBe(false)
  })
  it('emits toggle-favorite when the star is clicked', async () => {
    await openPicker()
    await body().find('[data-testid="metric-star"]').trigger('click')
    expect(wrapper.emitted('toggle-favorite')).toBeTruthy()
  })
})

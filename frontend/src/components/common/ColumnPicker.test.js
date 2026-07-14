import { describe, it, expect, afterEach } from 'vitest'
import { mount, DOMWrapper } from '@vue/test-utils'
import { nextTick } from 'vue'
import ColumnPicker from './ColumnPicker.vue'

// PopoverContent is teleported to document.body (see ui/popover/PopoverContent.vue),
// so once the popover is open we query the body rather than the component wrapper.
const body = () => new DOMWrapper(document.body)

async function open(wrapper) {
  await wrapper.find('button').trigger('click')
  await nextTick()
  // Popper positioning settles on a microtask; flush once more so the
  // teleported content is present in document.body before we query it.
  await new Promise((resolve) => setTimeout(resolve, 0))
}

const builtins = [
  { key: 'status', label: 'Status', group: 'Built-in' },
  { key: 'duration', label: 'Duration', group: 'Built-in' },
]

describe('ColumnPicker', () => {
  let wrapper
  afterEach(() => {
    wrapper?.unmount()
    wrapper = undefined
    document.body.innerHTML = ''
  })

  it('renders every available item as a toggle, grouped by group', async () => {
    wrapper = mount(ColumnPicker, {
      props: {
        available: [...builtins, { key: 'http.route', label: 'http.route', group: 'Attributes' }],
        selected: new Set(['status']),
      },
      attachTo: document.body,
    })
    await open(wrapper)

    expect(body().find('[data-test="col-toggle-status"]').exists()).toBe(true)
    expect(body().find('[data-test="col-toggle-duration"]').exists()).toBe(true)
    expect(body().find('[data-test="col-toggle-http.route"]').exists()).toBe(true)
    // Group headers present.
    expect(body().text()).toContain('Built-in')
    expect(body().text()).toContain('Attributes')
  })

  it('emits toggle(key) when an item is clicked', async () => {
    wrapper = mount(ColumnPicker, {
      props: { available: builtins, selected: new Set() },
      attachTo: document.body,
    })
    await open(wrapper)

    await body().find('[data-test="col-toggle-duration"]').trigger('click')
    expect(wrapper.emitted('toggle')).toEqual([['duration']])
  })

  it('shows a filter input only when there are more than 8 items, and narrows the list', async () => {
    const many = Array.from({ length: 12 }, (_, i) => ({
      key: `attr${i}`,
      label: `attr${i}`,
      group: 'Attributes',
    }))
    wrapper = mount(ColumnPicker, {
      props: { available: many, selected: new Set() },
      attachTo: document.body,
    })
    await open(wrapper)

    const input = body().find('[data-test="col-filter"]')
    expect(input.exists()).toBe(true)

    await input.setValue('attr1')
    await nextTick()
    // attr1, attr10, attr11 match "attr1"; attr2 does not.
    expect(body().find('[data-test="col-toggle-attr1"]').exists()).toBe(true)
    expect(body().find('[data-test="col-toggle-attr10"]').exists()).toBe(true)
    expect(body().find('[data-test="col-toggle-attr2"]').exists()).toBe(false)
  })

  it('omits the filter input when there are 8 or fewer items', async () => {
    wrapper = mount(ColumnPicker, {
      props: { available: builtins, selected: new Set() },
      attachTo: document.body,
    })
    await open(wrapper)
    expect(body().find('[data-test="col-filter"]').exists()).toBe(false)
  })
})

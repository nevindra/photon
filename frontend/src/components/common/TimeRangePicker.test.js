import { describe, it, expect, afterEach } from 'vitest'
import { mount, DOMWrapper } from '@vue/test-utils'
import { nextTick } from 'vue'
import TimeRangePicker from './TimeRangePicker.vue'

// PopoverContent is teleported to document.body (see ui/popover/PopoverContent.vue),
// so once the popover is open we query the body rather than the component wrapper.
const body = () => new DOMWrapper(document.body)

async function openPopover(wrapper) {
  await wrapper.find('button').trigger('click')
  await nextTick()
  // Popper positioning settles on a microtask; flush once more so the
  // teleported content is present in document.body before we query it.
  await new Promise((resolve) => setTimeout(resolve, 0))
}

function findByText(selector, text) {
  return body()
    .findAll(selector)
    .find((el) => el.text() === text)
}

describe('TimeRangePicker', () => {
  let wrapper

  afterEach(() => {
    wrapper?.unmount()
    wrapper = undefined
  })

  it('renders a trigger with "Last <preset>" when a preset is active', () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '15m', customRange: null },
      attachTo: document.body,
    })
    expect(wrapper.text()).toContain('Last 15m')
    expect(wrapper.text()).not.toContain('Custom')
  })

  it('renders a "Custom" trigger label when a custom range is active', () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '', customRange: { startMs: 1000, endMs: 2000 } },
      attachTo: document.body,
    })
    expect(wrapper.text()).toContain('Custom')
    expect(wrapper.text()).not.toContain('Last')
  })

  it('the trigger label reflects applied props, not the pending in-popover selection', async () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '30m', customRange: null },
      attachTo: document.body,
    })
    await openPopover(wrapper)

    // Selecting a different preset in the popover is only pending — the
    // trigger (still showing the applied value) must not change yet.
    const presetButton = findByText('button', '1h')
    await presetButton.trigger('click')

    expect(wrapper.find('button').text()).toContain('Last 30m')
  })

  it('clicking a quick-range preset does not emit anything by itself', async () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '30m', customRange: null },
      attachTo: document.body,
    })
    await openPopover(wrapper)

    const presetButton = findByText('button', '1h')
    expect(presetButton).toBeTruthy()
    await presetButton.trigger('click')

    expect(wrapper.emitted('update:modelValue')).toBeUndefined()
    expect(wrapper.emitted('update:customRange')).toBeUndefined()
  })

  it('selecting a preset then clicking Apply emits update:modelValue with that preset only', async () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '30m', customRange: null },
      attachTo: document.body,
    })
    await openPopover(wrapper)

    const presetButton = findByText('button', '1h')
    await presetButton.trigger('click')

    const applyButton = findByText('button', 'Apply')
    expect(applyButton.attributes('disabled')).toBeUndefined()
    await applyButton.trigger('click')

    expect(wrapper.emitted('update:modelValue')).toEqual([['1h']])
    expect(wrapper.emitted('update:customRange')).toBeUndefined()
  })

  it('the preset grid highlights the pending selection, not the applied one', async () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '30m', customRange: null },
      attachTo: document.body,
    })
    await openPopover(wrapper)

    const presetButton = findByText('button', '1h')
    await presetButton.trigger('click')

    expect(presetButton.classes()).toContain('bg-neutral-200')
    const previouslyApplied = findByText('button', '30m')
    expect(previouslyApplied.classes()).not.toContain('bg-neutral-200')
  })

  it('offers exactly the spec-defined quick ranges', async () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '30m', customRange: null },
      attachTo: document.body,
    })
    await openPopover(wrapper)

    const labels = ['5m', '15m', '30m', '1h', '3h', '6h', '12h', '24h', '7d']
    for (const label of labels) {
      expect(findByText('button', label)).toBeTruthy()
    }
  })

  it('disables Apply until a preset is selected or a valid custom range is entered', async () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '', customRange: null },
      attachTo: document.body,
    })
    await openPopover(wrapper)

    const applyButton = findByText('button', 'Apply')
    expect(applyButton.attributes('disabled')).toBeDefined()

    const [fromInput, toInput] = body().findAll('input[type="datetime-local"]')
    await fromInput.setValue('2026-07-01T10:00')
    expect(applyButton.attributes('disabled')).toBeDefined()

    // To before From — still invalid.
    await toInput.setValue('2026-07-01T09:00')
    expect(applyButton.attributes('disabled')).toBeDefined()

    await toInput.setValue('2026-07-01T12:00')
    expect(applyButton.attributes('disabled')).toBeUndefined()
  })

  it('editing From/To clears a pending preset selection', async () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '30m', customRange: null },
      attachTo: document.body,
    })
    await openPopover(wrapper)

    const presetButton = findByText('button', '1h')
    await presetButton.trigger('click')
    expect(presetButton.classes()).toContain('bg-neutral-200')

    const [fromInput] = body().findAll('input[type="datetime-local"]')
    await fromInput.setValue('2026-07-01T10:00')

    expect(presetButton.classes()).not.toContain('bg-neutral-200')
  })

  it('Apply with a valid From/To emits update:customRange with numeric epoch-ms', async () => {
    wrapper = mount(TimeRangePicker, {
      props: { modelValue: '30m', customRange: null },
      attachTo: document.body,
    })
    await openPopover(wrapper)

    const [fromInput, toInput] = body().findAll('input[type="datetime-local"]')
    await fromInput.setValue('2026-07-01T10:00')
    await toInput.setValue('2026-07-01T12:00')

    const applyButton = findByText('button', 'Apply')
    expect(applyButton.attributes('disabled')).toBeUndefined()
    await applyButton.trigger('click')

    const events = wrapper.emitted('update:customRange')
    expect(events).toHaveLength(1)
    const [{ startMs, endMs }] = events[0]
    expect(typeof startMs).toBe('number')
    expect(typeof endMs).toBe('number')
    expect(startMs).toBe(new Date('2026-07-01T10:00').getTime())
    expect(endMs).toBe(new Date('2026-07-01T12:00').getTime())
    expect(startMs).toBeLessThan(endMs)
    expect(wrapper.emitted('update:modelValue')).toBeUndefined()
  })
})

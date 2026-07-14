import { describe, it, expect, afterEach } from 'vitest'
import { mount, DOMWrapper } from '@vue/test-utils'
import { nextTick } from 'vue'
import DatePicker from './DatePicker.vue'

// PopoverContent is teleported to document.body (see ui/popover/PopoverContent.vue),
// so once the popover is open we query the body rather than the component wrapper.
const body = () => new DOMWrapper(document.body)

async function openPopover(wrapper: ReturnType<typeof mount>) {
  await wrapper.find('button').trigger('click')
  await nextTick()
  // Popper positioning settles on a microtask; flush once more so the
  // teleported content is present in document.body before we query it.
  await new Promise((resolve) => setTimeout(resolve, 0))
}

function findByText(selector: string, text: string) {
  return body()
    .findAll(selector)
    .find((el) => el.text() === text)
}

const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/

describe('DatePicker', () => {
  let wrapper: ReturnType<typeof mount> | undefined

  afterEach(() => {
    wrapper?.unmount()
    wrapper = undefined
  })

  it('shows the placeholder in the trigger when modelValue is empty', () => {
    wrapper = mount(DatePicker, {
      props: { modelValue: '' },
      attachTo: document.body,
    })
    expect(wrapper.text()).toContain('Pick a date')
  })

  it('shows the formatted applied date in the trigger when modelValue is set', () => {
    wrapper = mount(DatePicker, {
      props: { modelValue: '2025-06-12' },
      attachTo: document.body,
    })
    expect(wrapper.text()).toContain('Jun 12, 2025')
    expect(wrapper.text()).not.toContain('Pick a date')
  })

  it('offers the spec-defined relative presets', async () => {
    wrapper = mount(DatePicker, { props: { modelValue: '' }, attachTo: document.body })
    await openPopover(wrapper)

    for (const label of ['7d', '30d', '90d', '6mo', '1y', '2y']) {
      expect(findByText('button', label)).toBeTruthy()
    }
  })

  it('selecting a preset does not emit until Apply, then emits an ISO date', async () => {
    wrapper = mount(DatePicker, { props: { modelValue: '' }, attachTo: document.body })
    await openPopover(wrapper)

    const presetButton = findByText('button', '30d')
    expect(presetButton).toBeTruthy()
    await presetButton!.trigger('click')

    // Pending only — trigger label unchanged, nothing emitted yet.
    expect(wrapper.emitted('update:modelValue')).toBeUndefined()
    expect(wrapper.find('button').text()).toContain('Pick a date')

    const applyButton = findByText('button', 'Apply')
    expect(applyButton!.attributes('disabled')).toBeUndefined()
    await applyButton!.trigger('click')

    const events = wrapper.emitted('update:modelValue')
    expect(events).toHaveLength(1)
    const [value] = events![0] as [string]
    expect(value).toMatch(ISO_DATE)
  })

  it('highlights the pending preset', async () => {
    wrapper = mount(DatePicker, { props: { modelValue: '' }, attachTo: document.body })
    await openPopover(wrapper)

    const presetButton = findByText('button', '90d')
    await presetButton!.trigger('click')
    expect(presetButton!.classes()).toContain('bg-neutral-200')
  })

  it('entering a specific date clears an active preset and Apply emits that date', async () => {
    wrapper = mount(DatePicker, { props: { modelValue: '' }, attachTo: document.body })
    await openPopover(wrapper)

    // Pick a preset first...
    const presetButton = findByText('button', '30d')
    await presetButton!.trigger('click')
    expect(presetButton!.classes()).toContain('bg-neutral-200')

    // ...then a specific date takes over (mutual exclusion).
    const dateInput = body().find('input[type="date"]')
    await dateInput.setValue('2025-01-15')
    expect(presetButton!.classes()).not.toContain('bg-neutral-200')

    const applyButton = findByText('button', 'Apply')
    expect(applyButton!.attributes('disabled')).toBeUndefined()
    await applyButton!.trigger('click')

    expect(wrapper.emitted('update:modelValue')).toEqual([['2025-01-15']])
  })

  it('disables Apply until a preset or specific date is pending', async () => {
    wrapper = mount(DatePicker, { props: { modelValue: '' }, attachTo: document.body })
    await openPopover(wrapper)

    const applyButton = findByText('button', 'Apply')
    expect(applyButton!.attributes('disabled')).toBeDefined()

    const dateInput = body().find('input[type="date"]')
    await dateInput.setValue('2025-01-15')
    expect(applyButton!.attributes('disabled')).toBeUndefined()
  })
})

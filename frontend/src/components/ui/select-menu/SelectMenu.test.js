import { describe, it, expect } from 'vitest'
import { nextTick } from 'vue'
import { mount, DOMWrapper } from '@vue/test-utils'
import { SelectMenu } from './index.js'

const OPTIONS = [
  { value: 'recent', label: 'Recent' },
  { value: 'slowest', label: 'Slowest' },
  { value: 'errors', label: 'Errors' },
]

function mountMenu(props = {}) {
  return mount(SelectMenu, {
    props: { options: OPTIONS, modelValue: 'recent', ...props },
    attachTo: document.body,
  })
}

// PopoverContent teleports to document.body; open it, then query the body — the same pattern
// ColumnPicker/TraceTable tests use.
async function open(w) {
  await w.get('[aria-label]').trigger('click')
  await nextTick()
  await new Promise((r) => setTimeout(r, 0))
  return new DOMWrapper(document.body)
}

describe('SelectMenu', () => {
  it('shows the current option label (and prefix) on the trigger', () => {
    const w = mountMenu({ prefix: 'Sort:' })
    expect(w.text()).toContain('Sort:')
    expect(w.text()).toContain('Recent')
    w.unmount()
  })

  it('opens on trigger click and lists every option', async () => {
    const w = mountMenu()
    const body = await open(w)
    for (const o of OPTIONS) {
      expect(body.find(`[data-testid="select-option-${o.value}"]`).exists()).toBe(true)
    }
    w.unmount()
  })

  it('emits update:modelValue with the chosen value', async () => {
    const w = mountMenu()
    const body = await open(w)
    await body.find('[data-testid="select-option-slowest"]').trigger('click')
    expect(w.emitted('update:modelValue')[0]).toEqual(['slowest'])
    w.unmount()
  })

  it('does not re-emit when the already-selected value is chosen', async () => {
    const w = mountMenu({ modelValue: 'recent' })
    const body = await open(w)
    await body.find('[data-testid="select-option-recent"]').trigger('click')
    expect(w.emitted('update:modelValue')).toBeUndefined()
    w.unmount()
  })

  it('marks the selected option (aria-selected) for the check indicator', async () => {
    const w = mountMenu({ modelValue: 'slowest' })
    const body = await open(w)
    expect(body.find('[data-testid="select-option-slowest"]').attributes('aria-selected')).toBe('true')
    expect(body.find('[data-testid="select-option-recent"]').attributes('aria-selected')).toBe('false')
    w.unmount()
  })
})

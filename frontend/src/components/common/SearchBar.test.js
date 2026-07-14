import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, h, reactive } from 'vue'
import SearchBar from '@/components/common/SearchBar.vue'
import { SPAN_FIELDS, SPAN_EXAMPLE_QUERIES } from '@/lib/traces/spanFields'

// jsdom doesn't implement ResizeObserver (used by SearchBar to resync the overlay's scroll
// offset after a layout resize). Stub it so mounting the component doesn't throw here.
if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
}

// The component is controlled (`modelValue` prop + `update:modelValue` emit). Wrap it in a
// tiny v-model harness so typing round-trips through the prop exactly like it does in
// LogsView — otherwise the lexer-driven autocomplete would keep seeing the stale prop value.
function mountBar(initial = {}) {
  const state = reactive({
    value: initial.modelValue ?? '',
    services: initial.services ?? [],
    error: initial.error ?? null,
  })
  const Harness = defineComponent({
    setup() {
      return () =>
        h(SearchBar, {
          modelValue: state.value,
          services: state.services,
          error: state.error,
          // Only forward catalog/exampleQueries when a test passes them in, so the default
          // (undefined) path exercises SearchBar's own prop defaults — the logs catalog.
          ...(initial.catalog ? { catalog: initial.catalog } : {}),
          ...(initial.exampleQueries ? { exampleQueries: initial.exampleQueries } : {}),
          'onUpdate:modelValue': (v) => {
            state.value = v
          },
        })
    },
  })
  const wrapper = mount(Harness, { attachTo: document.body })
  return { wrapper, state, bar: wrapper.findComponent(SearchBar), input: wrapper.get('input') }
}

// Simulate typing: set the raw value + caret, then fire the input event the component listens
// on (which emits update:modelValue and records the caret from selectionStart).
async function type(input, value, caret = value.length) {
  input.element.value = value
  input.element.setSelectionRange(caret, caret)
  await input.trigger('input')
}

function optionTexts(wrapper) {
  return wrapper.findAll('[role="option"]').map((o) => o.text())
}

describe('SearchBar — overlay highlighting', () => {
  it('renders styled spans per token role (field pill, negated strikethrough, green free-text)', () => {
    const wrapper = mount(SearchBar, {
      props: { modelValue: 'service:api -level:debug "hello"' },
    })
    const spans = wrapper.findAll('span')

    const field = spans.find((s) => s.text() === 'service')
    expect(field).toBeTruthy()
    expect(field.classes()).toContain('font-semibold')
    expect(field.classes()).toContain('text-foreground')

    // Negated field key: red + strikethrough.
    const negField = spans.find((s) => s.text() === 'level')
    expect(negField).toBeTruthy()
    expect(negField.classes()).toContain('line-through')
    expect(negField.classes()).toContain('text-sev-error')

    // Quoted free-text: faint green, no pill.
    const quoted = spans.find((s) => s.text() === '"hello"')
    expect(quoted).toBeTruthy()
    expect(quoted.classes()).toContain('text-green-700')

    wrapper.unmount()
  })

  it('falls back to plain text and never throws on odd input', () => {
    const wrapper = mount(SearchBar, { props: { modelValue: '   ' } })
    expect(wrapper.find('input').exists()).toBe(true)
    wrapper.unmount()
  })
})

describe('SearchBar — error state', () => {
  it('renders the message + 1-based column and underlines the offending span', () => {
    const wrapper = mount(SearchBar, {
      props: { modelValue: 'level:foo', error: { message: 'bad field', offset: 0 } },
    })
    expect(wrapper.text()).toContain('bad field')
    expect(wrapper.text()).toContain('(column 1)')

    const target = wrapper.findAll('span').find((s) => s.text() === 'level')
    expect(target.classes()).toContain('decoration-wavy')

    wrapper.unmount()
  })

  it('suppresses the error visual while the user is actively editing', async () => {
    const { wrapper, input } = mountBar({
      modelValue: 'level:foo',
      error: { message: 'bad field', offset: 0 },
    })
    expect(wrapper.text()).toContain('bad field')

    await type(input, 'level:foob')
    expect(wrapper.text()).not.toContain('bad field')

    wrapper.unmount()
  })
})

describe('SearchBar — autocomplete', () => {
  it('opens on empty focus with field suggestions and examples', async () => {
    const { wrapper, input } = mountBar({ services: ['checkout-api'] })
    await input.trigger('focus')

    const texts = optionTexts(wrapper)
    expect(texts.some((t) => t.includes('service'))).toBe(true)
    expect(texts.some((t) => t.includes('level'))).toBe(true)
    // Examples group present when the box is empty.
    expect(texts.some((t) => t.includes('status_code>=500'))).toBe(true)

    wrapper.unmount()
  })

  it('filters field suggestions by the typed prefix', async () => {
    const { wrapper, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'ser')

    const texts = optionTexts(wrapper)
    expect(texts.some((t) => t.includes('service'))).toBe(true)
    expect(texts.some((t) => t.includes('level'))).toBe(false)
    // No examples once a prefix is present.
    expect(texts.some((t) => t.includes('status_code>=500'))).toBe(false)

    wrapper.unmount()
  })

  it('offers service values from the services prop after "service:"', async () => {
    const { wrapper, input } = mountBar({ services: ['checkout-api', 'payments-api'] })
    await input.trigger('focus')
    await type(input, 'service:')

    const texts = optionTexts(wrapper)
    expect(texts.some((t) => t.includes('checkout-api'))).toBe(true)
    expect(texts.some((t) => t.includes('payments-api'))).toBe(true)

    wrapper.unmount()
  })

  it('offers the fixed level enum after "level:"', async () => {
    const { wrapper, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'level:')

    const texts = optionTexts(wrapper)
    for (const lvl of ['debug', 'info', 'warn', 'error', 'fatal']) {
      expect(texts.some((t) => t.includes(lvl))).toBe(true)
    }

    wrapper.unmount()
  })

  it('shows no value list for a field without values', async () => {
    const { wrapper, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'trace_id:')
    expect(wrapper.findAll('[role="option"]').length).toBe(0)
    wrapper.unmount()
  })

  it('Enter inserts a field as "<name>:" leaving the caret after the colon', async () => {
    const { wrapper, bar, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'ser')
    await input.trigger('keydown', { key: 'Enter' })

    const emitted = bar.emitted('update:modelValue')
    expect(emitted.at(-1)[0]).toBe('service:')

    wrapper.unmount()
  })

  it('Tab inserts a value with a trailing space', async () => {
    const { wrapper, bar, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'level:')
    await input.trigger('keydown', { key: 'Tab' })

    const emitted = bar.emitted('update:modelValue')
    expect(emitted.at(-1)[0]).toBe('level:debug ')

    wrapper.unmount()
  })

  it('clicking a row inserts it and keeps focus (mousedown is prevented)', async () => {
    const { wrapper, bar, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'ser')

    const opt = wrapper.findAll('[role="option"]')[0]
    await opt.trigger('mousedown')
    await opt.trigger('click')

    const emitted = bar.emitted('update:modelValue')
    expect(emitted.at(-1)[0]).toBe('service:')

    wrapper.unmount()
  })

  it('preserves a leading "-" when inserting a field', async () => {
    const { wrapper, bar, input } = mountBar()
    await input.trigger('focus')
    await type(input, '-lev')
    await input.trigger('keydown', { key: 'Enter' })

    const emitted = bar.emitted('update:modelValue')
    expect(emitted.at(-1)[0]).toBe('-level:')

    wrapper.unmount()
  })

  it('replaces the whole query when an example is chosen', async () => {
    const { wrapper, bar, input } = mountBar()
    await input.trigger('focus')
    // Nothing is selected by default now, so the FIRST ArrowDown lands on item 0; reaching the
    // first example (past the 5 fields, index 5) takes 6 presses.
    for (let i = 0; i < 6; i++) await input.trigger('keydown', { key: 'ArrowDown' })
    await input.trigger('keydown', { key: 'Enter' })

    const emitted = bar.emitted('update:modelValue')
    expect(emitted.at(-1)[0]).toBe('service:checkout-api status_code>=500')

    wrapper.unmount()
  })

  it('highlights no row by default on the browse list, and only after an explicit arrow', async () => {
    const { wrapper, input } = mountBar()
    await input.trigger('focus')

    const selectedCount = () =>
      wrapper.findAll('[role="option"]').filter((o) => o.attributes('aria-selected') === 'true').length

    // Empty box → list is open but nothing reads as selected.
    expect(wrapper.findAll('[role="option"]').length).toBeGreaterThan(0)
    expect(selectedCount()).toBe(0)
    expect(input.attributes('aria-activedescendant')).toBeUndefined()

    // First ArrowDown selects the first row (not the second).
    await input.trigger('keydown', { key: 'ArrowDown' })
    const options = wrapper.findAll('[role="option"]')
    expect(options[0].attributes('aria-selected')).toBe('true')
    expect(selectedCount()).toBe(1)

    wrapper.unmount()
  })

  it('highlights the top row while completing a typed prefix (Enter target is visible)', async () => {
    const { wrapper, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'ser')
    // A typed prefix is a completion, so the top match is the highlighted Enter target.
    expect(wrapper.findAll('[role="option"]')[0].attributes('aria-selected')).toBe('true')
    wrapper.unmount()
  })

  it('Enter on the empty browse list applies the query as-is instead of grafting the first row', async () => {
    const { wrapper, bar, input } = mountBar()
    await input.trigger('focus')
    expect(wrapper.findAll('[role="option"]').length).toBeGreaterThan(0)

    await input.trigger('keydown', { key: 'Enter' })

    // Nothing was inserted (no emit at all), and the browse list closed.
    expect(bar.emitted('update:modelValue')).toBeFalsy()
    expect(wrapper.findAll('[role="option"]').length).toBe(0)

    wrapper.unmount()
  })

  it('Enter after a completed term + trailing space does not append a field/example', async () => {
    const { wrapper, bar, input } = mountBar({ services: ['checkout-api'] })
    await input.trigger('focus')
    await type(input, 'service:checkout-api ')

    await input.trigger('keydown', { key: 'Enter' })

    // The last value is exactly what was typed — Enter added nothing.
    expect(bar.emitted('update:modelValue').at(-1)[0]).toBe('service:checkout-api ')

    wrapper.unmount()
  })

  it('hovering a browse-list row then Enter still selects it (explicit pick re-enables accept)', async () => {
    const { wrapper, bar, input } = mountBar()
    await input.trigger('focus')
    await wrapper.findAll('[role="option"]')[0].trigger('mouseenter')
    await input.trigger('keydown', { key: 'Enter' })

    const emitted = bar.emitted('update:modelValue')
    expect(emitted).toBeTruthy()
    expect(emitted.at(-1)[0]).toMatch(/:$/) // a field was inserted as "<name>:"

    wrapper.unmount()
  })

  it('Esc closes the dropdown without clearing the query', async () => {
    const { wrapper, bar, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'ser')
    expect(wrapper.findAll('[role="option"]').length).toBeGreaterThan(0)

    await input.trigger('keydown', { key: 'Escape' })
    expect(wrapper.findAll('[role="option"]').length).toBe(0)
    // Query is untouched (last emit is still the typed value, not empty).
    expect(bar.emitted('update:modelValue').at(-1)[0]).toBe('ser')

    wrapper.unmount()
  })

  it('shows no dropdown inside a quoted phrase (freetext context)', async () => {
    const { wrapper, input } = mountBar()
    await input.trigger('focus')
    await type(input, '"tim')
    expect(wrapper.findAll('[role="option"]').length).toBe(0)
    wrapper.unmount()
  })

  it('ArrowDown is inert when the dropdown is closed (native caret movement is not suppressed)', () => {
    const { wrapper, input } = mountBar()
    // Not focused → dropdown is closed; ArrowDown must pass through untouched.
    const evt = new KeyboardEvent('keydown', { key: 'ArrowDown', cancelable: true, bubbles: true })
    expect(() => input.element.dispatchEvent(evt)).not.toThrow()
    expect(evt.defaultPrevented).toBe(false)
    expect(wrapper.findAll('[role="option"]').length).toBe(0)
    wrapper.unmount()
  })

  it('ArrowDown is inert when the dropdown is open but offers no items', async () => {
    const { wrapper, input } = mountBar()
    await input.trigger('focus')
    await type(input, 'trace_id:') // a field with no values → open would have 0 items
    expect(wrapper.findAll('[role="option"]').length).toBe(0)

    const evt = new KeyboardEvent('keydown', { key: 'ArrowDown', cancelable: true, bubbles: true })
    expect(() => input.element.dispatchEvent(evt)).not.toThrow()
    expect(evt.defaultPrevented).toBe(false)

    wrapper.unmount()
  })
})

describe('SearchBar — catalog prop (spans mode)', () => {
  it('offers the passed-in catalog fields and example queries instead of the logs defaults', async () => {
    const { wrapper, input } = mountBar({ catalog: SPAN_FIELDS, exampleQueries: SPAN_EXAMPLE_QUERIES })
    await input.trigger('focus')

    const texts = optionTexts(wrapper)
    expect(texts.some((t) => t.includes('operation'))).toBe(true)
    expect(texts.some((t) => t.includes('duration'))).toBe(true)
    expect(texts.some((t) => t.includes('level'))).toBe(false)
    expect(texts.some((t) => t.includes('duration>=500ms'))).toBe(true)
    expect(texts.some((t) => t.includes('status_code>=500'))).toBe(false)

    wrapper.unmount()
  })

  it('offers the fixed status enum after "status:" using the passed-in catalog', async () => {
    const { wrapper, input } = mountBar({ catalog: SPAN_FIELDS, exampleQueries: SPAN_EXAMPLE_QUERIES })
    await input.trigger('focus')
    await type(input, 'status:')

    const texts = optionTexts(wrapper)
    for (const v of ['ok', 'error', 'unset']) {
      expect(texts.some((t) => t.includes(v))).toBe(true)
    }

    wrapper.unmount()
  })

  it('with no catalog prop, still behaves exactly like the logs default (backward compatible)', async () => {
    const { wrapper, input } = mountBar({ services: ['checkout-api'] })
    await input.trigger('focus')

    const texts = optionTexts(wrapper)
    expect(texts.some((t) => t.includes('service'))).toBe(true)
    expect(texts.some((t) => t.includes('status_code>=500'))).toBe(true)

    wrapper.unmount()
  })
})

describe('SearchBar — keyboard hint footer', () => {
  it('shows navigate/select/dismiss hints while the dropdown is open', async () => {
    const { wrapper, input } = mountBar()
    await input.trigger('focus')
    expect(wrapper.findAll('[role="option"]').length).toBeGreaterThan(0)
    const text = wrapper.text()
    expect(text).toContain('navigate')
    expect(text).toContain('select')
    expect(text).toContain('dismiss')
    wrapper.unmount()
  })

  it('hides the hints when the dropdown is closed', () => {
    const { wrapper } = mountBar()
    expect(wrapper.text()).not.toContain('navigate')
    wrapper.unmount()
  })
})

describe('SearchBar — accessibility', () => {
  it('exposes an aria-label on the combobox input', () => {
    const wrapper = mount(SearchBar)
    expect(wrapper.get('input').attributes('aria-label')).toBe('Search logs')
    wrapper.unmount()
  })
})

describe('SearchBar — no Cmd/Ctrl+K global handler', () => {
  it('adds no global keydown listener', () => {
    const spy = vi.spyOn(window, 'addEventListener')
    const { wrapper } = mountBar()
    const keydownCalls = spy.mock.calls.filter((c) => c[0] === 'keydown')
    expect(keydownCalls.length).toBe(0)

    // And dispatching Ctrl+K does nothing observable.
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', ctrlKey: true }))

    spy.mockRestore()
    wrapper.unmount()
  })
})

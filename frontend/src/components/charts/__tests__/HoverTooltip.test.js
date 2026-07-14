import { describe, it, expect, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import HoverTooltip from '../HoverTooltip.vue'

// Teleports to `document.body`, so assertions read from the body rather than wrapper.html().
describe('HoverTooltip', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('renders nothing in the body when hidden', () => {
    mount(HoverTooltip, { props: { visible: false }, attachTo: document.body })
    expect(document.body.querySelector('[role="tooltip"]')).toBeNull()
  })

  it('teleports the tooltip into the body with title/subtitle and positions it via x/y', () => {
    mount(HoverTooltip, {
      props: { visible: true, x: 120, y: 45, title: '42 lines', subtitle: '10:00–10:05' },
      attachTo: document.body,
    })

    const tooltip = document.body.querySelector('[role="tooltip"]')
    expect(tooltip).not.toBeNull()
    expect(tooltip.textContent).toContain('42 lines')
    expect(tooltip.textContent).toContain('10:00–10:05')
    expect(tooltip.style.left).toBe('120px')
    expect(tooltip.style.top).toBe('45px')
  })
})

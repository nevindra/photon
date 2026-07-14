import { mount, DOMWrapper } from '@vue/test-utils'
import { nextTick } from 'vue'
import { describe, it, expect } from 'vitest'
import LiveControl from './LiveControl.vue'

// SelectMenu's options render only while open, teleported to document.body — open the trigger,
// then query the body (same pattern as SelectMenu.test.js).
async function openSelect(wrapper, ariaLabel) {
  await wrapper.get(`[aria-label="${ariaLabel}"]`).trigger('click')
  await nextTick()
  await new Promise((r) => setTimeout(r, 0))
  return new DOMWrapper(document.body)
}

describe('LiveControl', () => {
  it('emits update:mode when chosen', async () => {
    const w = mount(LiveControl, {
      props: { mode: 'manual', status: 'idle' },
      attachTo: document.body,
    })
    const body = await openSelect(w, 'Refresh mode')
    await body.find('[data-testid="select-option-30s"]').trigger('click')
    expect(w.emitted('update:mode')[0]).toEqual(['30s'])
    w.unmount()
  })

  it('emits refresh when the refresh button is clicked', async () => {
    const w = mount(LiveControl, { props: { mode: 'manual', status: 'idle' } })
    await w.get('[data-testid="live-refresh"]').trigger('click')
    expect(w.emitted('refresh')).toHaveLength(1)
  })

  it('shows the rate only while live', () => {
    const w = mount(LiveControl, { props: { mode: 'live', status: 'live', rate: 1234 } })
    expect(w.text()).toContain('1.2k/s')
  })

  it('hides the Live option when streamable is false', async () => {
    const w = mount(LiveControl, {
      props: { mode: 'manual', status: 'idle', streamable: false },
      attachTo: document.body,
    })
    const body = await openSelect(w, 'Refresh mode')
    expect(body.find('[data-testid="select-option-live"]').exists()).toBe(false)
    w.unmount()
  })
})

import { describe, it, expect, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { PeekDrawer } from './index.js'

async function flush() {
  await new Promise((r) => setTimeout(r, 0))
}

function mountDrawer(props, slots) {
  return mount(PeekDrawer, {
    props: { open: true, hasContent: true, index: -1, total: 0, ...props },
    slots,
    attachTo: document.body,
  })
}

// Sheet content is teleported to document.body, outside the wrapper's own root element — clear
// it defensively so a failed assertion in one test can't leak stale DOM into the next test's
// document.body queries (mirrors TracePeekDrawer.test.js's convention).
afterEach(() => {
  document.body.innerHTML = ''
})

describe('PeekDrawer — nav strip', () => {
  it('renders index+1 / total when open and total > 1', async () => {
    const w = mountDrawer({ index: 2, total: 128 })
    await flush()
    expect(document.body.textContent).toContain('3 / 128')
    w.unmount()
  })

  it('emits prev when the ‹ button is clicked, and next when the › button is clicked', async () => {
    const w = mountDrawer({ index: 2, total: 128 })
    await flush()
    document.body.querySelector('[data-testid="peek-drawer-prev"]').click()
    document.body.querySelector('[data-testid="peek-drawer-next"]').click()
    await flush()
    expect(w.emitted('prev')).toBeTruthy()
    expect(w.emitted('next')).toBeTruthy()
    w.unmount()
  })

  it('disables ‹ at the start of the list and › at the end of the list', async () => {
    const wStart = mountDrawer({ index: 0, total: 5 })
    await flush()
    expect(document.body.querySelector('[data-testid="peek-drawer-prev"]').disabled).toBe(true)
    expect(document.body.querySelector('[data-testid="peek-drawer-next"]').disabled).toBe(false)
    wStart.unmount()
    document.body.innerHTML = ''

    const wEnd = mountDrawer({ index: 4, total: 5 })
    await flush()
    expect(document.body.querySelector('[data-testid="peek-drawer-next"]').disabled).toBe(true)
    expect(document.body.querySelector('[data-testid="peek-drawer-prev"]').disabled).toBe(false)
    wEnd.unmount()
  })

  it('omits the nav strip when total <= 1', async () => {
    const w = mountDrawer({ index: 0, total: 1 })
    await flush()
    expect(document.body.querySelector('[data-testid="peek-drawer-prev"]')).toBeFalsy()
    expect(document.body.querySelector('[data-testid="peek-drawer-next"]')).toBeFalsy()
    w.unmount()
  })
})

describe('PeekDrawer — keyboard nav', () => {
  it('emits next on j / ArrowDown and prev on k / ArrowUp while open', async () => {
    const w = mountDrawer({ index: 2, total: 5 })
    await flush()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'j' }))
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown' }))
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'k' }))
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp' }))
    await flush()

    expect(w.emitted('next').length).toBe(2)
    expect(w.emitted('prev').length).toBe(2)
    w.unmount()
  })

  it('emits shortcut with the event for a non-nav key', async () => {
    const w = mountDrawer({ index: 2, total: 5 })
    await flush()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'c' }))
    await flush()

    expect(w.emitted('shortcut')).toBeTruthy()
    expect(w.emitted('shortcut')[0][0]).toBeInstanceOf(KeyboardEvent)
    expect(w.emitted('shortcut')[0][0].key).toBe('c')
    w.unmount()
  })

  it('ignores keydown when the target is an input', async () => {
    const w = mountDrawer(
      { index: 2, total: 5 },
      { default: '<div><input data-testid="peek-input" /></div>' },
    )
    await flush()

    const input = document.body.querySelector('[data-testid="peek-input"]')
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'j', bubbles: true }))
    await flush()

    expect(w.emitted('next')).toBeFalsy()
    w.unmount()
  })

  it('ignores keydown when a modifier key is held', async () => {
    const w = mountDrawer({ index: 2, total: 5 })
    await flush()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'j', metaKey: true }))
    await flush()

    expect(w.emitted('next')).toBeFalsy()
    w.unmount()
  })

  it('removes the window listener once open flips to false', async () => {
    const w = mountDrawer({ index: 2, total: 5 })
    await flush()

    await w.setProps({ open: false })
    await flush()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'j' }))
    await flush()

    expect(w.emitted('next')).toBeFalsy()
    w.unmount()
  })
})

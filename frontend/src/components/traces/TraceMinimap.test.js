import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { mount } from '@vue/test-utils'
import TraceMinimap from './TraceMinimap.vue'

// The minimap draws to a canvas (visual-only) and reads its own pixel height for the drag math.
// jsdom has neither a real 2D context nor layout, so stub getContext → null (draw becomes a
// no-op, as the component guards) and stub offsetHeight so the pointer→scrollTop math has a
// height to divide by. ResizeObserver is absent in jsdom too; useResizeObserver no-ops without
// it, but stub it so nothing throws.
let restoreH, restoreCtx
if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
}
beforeAll(() => {
  restoreH = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight')
  restoreCtx = HTMLCanvasElement.prototype.getContext
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, get: () => 1000 })
  HTMLCanvasElement.prototype.getContext = () => null
})
afterAll(() => {
  if (restoreH) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', restoreH)
  HTMLCanvasElement.prototype.getContext = restoreCtx
})

const ROWS = [
  { id: 'a', offsetNs: 0, durationNs: 100, isError: false },
  { id: 'b', offsetNs: 100, durationNs: 400, isError: true },
  { id: 'c', offsetNs: 200, durationNs: 800, isError: false },
]

function mountMinimap(props = {}) {
  return mount(TraceMinimap, {
    props: {
      rows: ROWS,
      traceDurationNs: 1000,
      scrollTop: 0,
      viewportHeight: 0,
      totalHeight: 1000,
      ...props,
    },
    attachTo: document.body,
  })
}

describe('TraceMinimap', () => {
  it('positions the viewport rectangle from the scroll props', () => {
    const w = mountMinimap({ scrollTop: 100, viewportHeight: 200, totalHeight: 1000 })
    const style = w.find('[data-testid="trace-minimap-viewport"]').attributes('style')
    expect(style).toContain('top: 10%')
    expect(style).toContain('height: 20%')
    w.unmount()
  })

  it('clamps the viewport rectangle to the visible range', () => {
    const w = mountMinimap({ scrollTop: 5000, viewportHeight: 4000, totalHeight: 1000 })
    const style = w.find('[data-testid="trace-minimap-viewport"]').attributes('style')
    expect(style).toContain('top: 100%')
    expect(style).toContain('height: 100%')
    w.unmount()
  })

  // clientX/clientY are read-only accessors on MouseEvent, so vue-test-utils' `trigger` can't set
  // them post-construction — dispatch a native event that carries the coordinate in its init dict.
  function pointerdownAt(wrapper, clientY) {
    wrapper
      .find('[data-testid="trace-minimap"]')
      .element.dispatchEvent(new MouseEvent('pointerdown', { clientY, bubbles: true }))
  }

  it('emits scroll-to (centred, clamped) on pointerdown at a y-fraction', () => {
    // offsetHeight stubbed to 1000; a pointer at clientY=500 is the vertical midpoint →
    // raw = 0.5 * totalHeight = 1000; centred on a 200px viewport → 900; clamp keeps it.
    const w = mountMinimap({ totalHeight: 2000, viewportHeight: 200 })
    pointerdownAt(w, 500)
    const emitted = w.emitted('scroll-to')
    expect(emitted).toHaveLength(1)
    expect(emitted[0][0]).toBeCloseTo(900, 0)
    w.unmount()
  })

  it('clamps the emitted scroll-to into [0, totalHeight - viewportHeight]', () => {
    const w = mountMinimap({ totalHeight: 1000, viewportHeight: 300 })
    // Pointer near the very bottom would overshoot; expect a clamp to totalHeight - viewport.
    pointerdownAt(w, 1000)
    const emitted = w.emitted('scroll-to')
    expect(emitted[0][0]).toBeCloseTo(700, 0)
    w.unmount()
  })

  it('renders without throwing when the canvas 2D context is null', () => {
    // getContext is stubbed to null above; mounting (which calls draw) must not throw.
    expect(() => {
      const w = mountMinimap({ matches: new Set(['b']) })
      expect(w.find('[data-testid="trace-minimap"]').exists()).toBe(true)
      expect(w.find('canvas').exists()).toBe(true)
      w.unmount()
    }).not.toThrow()
  })
})

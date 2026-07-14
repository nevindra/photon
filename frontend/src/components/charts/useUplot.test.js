import { defineComponent, ref, toRef } from 'vue'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { useUplot } from './useUplot.js'

// A fake uPlot recording its lifecycle calls, shared with the (hoisted) module mocks below.
const { calls, FakeUplot } = vi.hoisted(() => {
  const calls = { construct: [], setData: [], setSize: [], destroy: 0 }
  class FakeUplot {
    constructor(opts, data, el) {
      calls.construct.push({ opts, data, el })
    }
    setData(d) {
      calls.setData.push(d)
    }
    setSize(s) {
      calls.setSize.push(s)
    }
    destroy() {
      calls.destroy += 1
    }
    redraw() {}
  }
  return { calls, FakeUplot }
})
vi.mock('uplot', () => ({ default: FakeUplot }))
vi.mock('uplot/dist/uPlot.min.css', () => ({}))

// Host component: exposes the useUplot handles and drives build()/size from reactive props.
const Harness = defineComponent({
  props: { data: { type: Array, default: () => [[], []] }, w: { type: Number, default: 0 }, h: { type: Number, default: 0 } },
  setup(props) {
    const el = ref(null)
    const size = { width: toRef(props, 'w'), height: toRef(props, 'h') }
    const build = () => ({ opts: { series: [] }, data: props.data })
    const api = useUplot(el, build, size)
    return { el, ...api }
  },
  template: '<div ref="el" />',
})

beforeEach(() => {
  calls.construct.length = 0
  calls.setData.length = 0
  calls.setSize.length = 0
  calls.destroy = 0
})
afterEach(() => {
  vi.restoreAllMocks()
})

describe('useUplot', () => {
  it('no-ops without a canvas 2D context (jsdom) and tears down cleanly', async () => {
    // jsdom's getContext returns null → canRender() is false → never construct.
    const wrapper = mount(Harness, { props: { w: 400, h: 200, data: [[1], [2]] } })
    await flushPromises()

    expect(wrapper.vm.uplot).toBeNull()
    expect(calls.construct).toHaveLength(0)

    // handles are safe no-ops in the headless path
    expect(() => {
      wrapper.vm.redraw()
      wrapper.vm.rebuild()
    }).not.toThrow()

    expect(() => wrapper.unmount()).not.toThrow()
    expect(calls.destroy).toBe(0) // nothing was ever created
  })

  it('drives the full lifecycle once a 2D context is available', async () => {
    // Give jsdom a (fake) 2D context so canRender() passes and the mocked engine is exercised.
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({})

    const wrapper = mount(Harness, { props: { w: 400, h: 200, data: [[1], [2]] } })
    await flushPromises()

    // created once, sized from the reactive size
    expect(calls.construct).toHaveLength(1)
    expect(calls.construct[0].opts.width).toBe(400)
    expect(calls.construct[0].opts.height).toBe(200)
    expect(wrapper.vm.uplot).not.toBeNull()

    // data change → setData (not a re-construct)
    await wrapper.setProps({ data: [[9], [9]] })
    await flushPromises()
    expect(calls.setData).toHaveLength(1)
    expect(calls.setData[0]).toEqual([[9], [9]])
    expect(calls.construct).toHaveLength(1)

    // size change → setSize
    await wrapper.setProps({ w: 500 })
    await flushPromises()
    expect(calls.setSize.at(-1)).toEqual({ width: 500, height: 200 })

    // rebuild → destroy + recreate
    wrapper.vm.rebuild()
    expect(calls.destroy).toBe(1)
    expect(calls.construct).toHaveLength(2)

    // unmount → final destroy
    wrapper.unmount()
    expect(calls.destroy).toBe(2)
  })

  it('never constructs before it has a real size', async () => {
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({})

    // 0×0 container: engine loads but creation is blocked until a size arrives.
    const wrapper = mount(Harness, { props: { w: 0, h: 0, data: [[1], [2]] } })
    await flushPromises()
    expect(calls.construct).toHaveLength(0)

    await wrapper.setProps({ w: 320, h: 160 })
    await flushPromises()
    expect(calls.construct).toHaveLength(1)

    wrapper.unmount()
  })
})

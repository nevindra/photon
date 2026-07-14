import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import LcpAttributionBar from './LcpAttributionBar.vue'

describe('LcpAttributionBar', () => {
  it('renders one segment per present sub-part, sized by its share of the total', () => {
    const w = mount(LcpAttributionBar, {
      props: { ttfb: 120, resourceLoadDelay: 30, resourceLoadTime: 900, elementRenderDelay: 50, element: '<img class=hero>' },
    })
    const segs = w.findAll('[data-testid="lcp-segment"]')
    expect(segs).toHaveLength(4)
    // resource load time = 900 / (120+30+900+50=1100) ≈ 81.8%.
    const rlt = w.get('[data-part="resourceLoadTime"]')
    expect(rlt.attributes('style')).toContain('width: 81.8181')
    expect(rlt.text()).toContain('900 ms')
  })

  it('names the dominant segment + the element in the insight line', () => {
    const w = mount(LcpAttributionBar, {
      props: { ttfb: 120, resourceLoadDelay: 30, resourceLoadTime: 900, elementRenderDelay: 50, element: '<img class=hero>' },
    })
    const insight = w.get('[data-testid="lcp-insight"]').text()
    expect(insight).toContain('Resource load time')
    expect(insight).toContain('82%') // 81.8% rounded
    expect(insight).toContain('<img class=hero>')
  })

  it('omits missing/null sub-parts (fewer segments, no legend entry)', () => {
    const w = mount(LcpAttributionBar, { props: { ttfb: 100, resourceLoadTime: 300, element: '#hero' } })
    expect(w.findAll('[data-testid="lcp-segment"]')).toHaveLength(2)
    expect(w.find('[data-part="resourceLoadDelay"]').exists()).toBe(false)
  })

  it('renders nothing when no sub-parts are present', () => {
    const w = mount(LcpAttributionBar, { props: { element: '#hero' } })
    expect(w.find('[data-testid="lcp-segment"]').exists()).toBe(false)
    expect(w.find('[data-testid="lcp-insight"]').exists()).toBe(false)
  })
})

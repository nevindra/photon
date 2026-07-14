import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import RumFleetKpis from './RumFleetKpis.vue'

const kpis = [
  { key: 'pv', label: 'Pageviews', value: '128.4k', accent: 'info' as const },
  {
    key: 'cwv',
    label: 'Core Web Vitals · good',
    value: '74%',
    accent: 'warning' as const,
    dist: { good: 74, needs: 18, poor: 8 },
    sub: 'good · needs · poor',
  },
  {
    key: 'slow',
    label: 'Slowest app · LCP',
    value: '4.3s',
    accent: 'error' as const,
    valueTone: 'poor' as const,
    sub: 'web-admin',
    to: '/rum/web-admin',
  },
]

describe('RumFleetKpis', () => {
  it('renders every tile with its value and sub-content', () => {
    const w = mount(RumFleetKpis, { props: { kpis } })
    expect(w.text()).toContain('128.4k')
    expect(w.text()).toContain('74%')
    expect(w.text()).toContain('4.3s')
    // The slowest-app value is toned by its rating.
    expect(w.find('.text-sev-error').exists()).toBe(true)
    // A tile with a dist renders the distribution bar.
    expect(w.find('[data-testid="dist-good"]').exists()).toBe(true)
  })

  it('emits navigate only for tiles that carry a `to`', async () => {
    const w = mount(RumFleetKpis, { props: { kpis } })
    const buttons = w.findAll('button')
    expect(buttons).toHaveLength(1) // only the "slow" tile is clickable
    await buttons[0].trigger('click')
    expect(w.emitted('navigate')?.[0]).toEqual(['/rum/web-admin'])
  })
})

import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ChartTooltipCard from '../ChartTooltipCard.vue'

describe('ChartTooltipCard', () => {
  it('renders header, total, and breakdown rows with a Tailwind-class swatch', () => {
    const w = mount(ChartTooltipCard, {
      props: {
        title: '14:22:07 – 14:22:37',
        total: '142 spans',
        rows: [
          { key: 'ok', label: 'Ok', value: '128', swatchClass: 'bg-muted-foreground/70' },
          { key: 'error', label: 'Error', value: '12', swatchClass: 'bg-sev-error' },
        ],
      },
    })
    expect(w.text()).toContain('14:22:07 – 14:22:37')
    expect(w.text()).toContain('142 spans')
    expect(w.text()).toContain('Ok')
    expect(w.text()).toContain('128')
    expect(w.text()).toContain('Error')
    // The swatch renders the caller's Tailwind background class.
    expect(w.html()).toContain('bg-sev-error')
    // Canonical light-bordered surface.
    expect(w.get('div').classes()).toEqual(expect.arrayContaining(['bg-popover', 'border', 'shadow-lg']))
  })

  it('supports a raw CSS-colour swatch (line-series strokes)', () => {
    const w = mount(ChartTooltipCard, {
      props: { title: '14:22', rows: [{ key: 'a', label: 'svc-a', value: '9', swatchColor: 'rgb(1, 2, 3)' }] },
    })
    expect(w.html()).toContain('rgb(1, 2, 3)')
    expect(w.text()).toContain('svc-a')
  })

  it('omits the total and rows when not provided (header-only)', () => {
    const w = mount(ChartTooltipCard, { props: { title: 'only header' } })
    expect(w.text()).toContain('only header')
    expect(w.text().trim()).toBe('only header')
  })
})

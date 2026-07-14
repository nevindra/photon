import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import RumSlowestRoutes from './RumSlowestRoutes.vue'
import type { RouteRow } from '@/lib/rum/rumSummary'

const routes: RouteRow[] = [
  { app: 'web-storefront', route: '/checkout', lcp_p75: 4900, rating: 'poor' },
  { app: 'admin-dashboard', route: '/reports', lcp_p75: 2200, rating: 'good' },
]

describe('RumSlowestRoutes', () => {
  it('renders each route with its formatted LCP', () => {
    const w = mount(RumSlowestRoutes, { props: { routes } })
    const rows = w.findAll('[data-testid="rum-route"]')
    expect(rows).toHaveLength(2)
    expect(rows[0].text()).toContain('/checkout')
    expect(rows[0].text()).toContain('4.9s')
  })

  it('emits open with the app + route on click', async () => {
    const w = mount(RumSlowestRoutes, { props: { routes } })
    await w.findAll('[data-testid="rum-route"]')[0].trigger('click')
    expect(w.emitted('open')?.[0]).toEqual([{ app: 'web-storefront', route: '/checkout' }])
  })
})

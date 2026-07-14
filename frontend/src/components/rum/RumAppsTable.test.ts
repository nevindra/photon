import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import RumAppsTable from './RumAppsTable.vue'
import type { AppRow } from '@/lib/rum/rumSummary'

const rows: AppRow[] = [
  {
    app: 'web-storefront',
    pageviews: 96800,
    lcp: { p75: 4300, rating: 'poor' },
    inp: { p75: 180, rating: 'good' },
    cls: { p75: 0.18, rating: 'needs' },
    errors: 42,
    sessions: 19,
    status: 'poor',
    _poor: 1,
    _needs: 1,
  },
]

describe('RumAppsTable', () => {
  it('renders a row per app with formatted, rating-coloured vitals', () => {
    const w = mount(RumAppsTable, { props: { rows } })
    const row = w.get('[data-testid="rum-app-row"]')
    expect(row.text()).toContain('web-storefront')
    expect(row.text()).toContain('4.3s') // LCP formatted
    expect(row.text()).toContain('0.18') // CLS unitless
    expect(row.text()).toContain('96,800') // pageviews
    // The poor LCP cell is coloured; the error count is coloured because errors > 0.
    expect(row.findAll('.text-sev-error').length).toBeGreaterThan(0)
  })

  it('emits open with the app name on row click', async () => {
    const w = mount(RumAppsTable, { props: { rows } })
    await w.get('[data-testid="rum-app-row"]').trigger('click')
    expect(w.emitted('open')?.[0]).toEqual(['web-storefront'])
  })
})

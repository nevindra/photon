import { describe, it, expect } from 'vitest'
import * as q from '@/lib/rum/rumQueries'

// Minimal existence/shape test (mirrors servicesQueries.test.js) — the composables are exercised
// end-to-end by the RUM views (Task E3+); this just guards the module's public surface.
describe('rumQueries', () => {
  it('exports the RUM composables', () => {
    for (const n of [
      'useRumApps',
      'useRumVitals',
      'useRumBreakdown',
      'useRumPages',
      'useRumPageDetail',
      'useRumErrors',
      'useRumErrorFacets',
      'useRumErrorDetail',
    ]) {
      expect(typeof q[n]).toBe('function')
    }
  })
})

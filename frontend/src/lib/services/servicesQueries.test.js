import { describe, it, expect } from 'vitest'
import * as q from '@/lib/services/servicesQueries'

// Minimal existence/shape test (mirrors the brief) — the composables are exercised end-to-end by
// the Services list/detail views (Task 11/12); this just guards the module's public surface.
describe('servicesQueries', () => {
  it('exports the service composables', () => {
    for (const n of [
      'useServicesList',
      'useServiceTimeseries',
      'useServiceDependencies',
      'useServiceSettings',
      'useSetServiceSettings',
      'useResetServiceSettings',
    ]) {
      expect(typeof q[n]).toBe('function')
    }
  })
})

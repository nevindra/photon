import { describe, it, expect } from 'vitest'
import * as q from '@/lib/infra/infraQueries'

// Minimal existence/shape test (mirrors rumQueries.test.js / servicesQueries.test.js) — the
// composables are exercised end-to-end by the Infra views (InfraHostsView/InfraHostDetailView);
// this guards the module's public surface and pins the host-list query-key shape.
describe('infraQueries', () => {
  it('exports the infra composables', () => {
    for (const n of ['useInfraHosts', 'useInfraHost', 'useInfraHostSeries', 'infraHostsKey']) {
      expect(typeof (q as any)[n]).toBe('function')
    }
  })

  it('builds a stable host-list query key', () => {
    expect(q.infraHostsKey('1', '2')).toEqual(['infra', 'hosts', '1', '2'])
  })
})

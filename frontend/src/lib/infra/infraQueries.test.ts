import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// Mock useQuery so we can assert the plain options object each wrapper builds (mirrors
// logsQueries.test.js).
const useQueryMock = vi.fn((_opts: any) => ({}))
vi.mock('@tanstack/vue-query', async (orig) => ({
  ...(await orig()),
  useQuery: (opts: unknown) => useQueryMock(opts),
}))

import * as q from '@/lib/infra/infraQueries'
import { api } from '@/lib/core/api'
import { setTenant, clearTenant } from '@/lib/core/context'

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

// The Infra vertical filters by the global tenant scope like logs/traces/metrics/services: the
// active tenant's grammar term rides along as `q` and re-keys the cache.
describe('infraQueries tenant filter', () => {
  beforeEach(() => {
    useQueryMock.mockClear()
    clearTenant()
  })
  afterEach(() => {
    clearTenant()
    vi.restoreAllMocks()
  })

  it('useInfraHosts passes the tenant term as q and re-keys on it', async () => {
    const spy = vi.spyOn(api, 'infraHosts').mockResolvedValue({ hosts: [] } as any)
    setTenant('divtik')
    q.useInfraHosts('0', '1')
    const opts = useQueryMock.mock.calls.at(-1)![0] as any

    expect(opts.queryKey.value).toContain('tenant:divtik')
    await opts.queryFn({ signal: undefined })
    expect(spy).toHaveBeenCalledWith('0', '1', 'tenant:divtik', expect.anything())
  })

  it('adds no term when no tenant is set', async () => {
    const spy = vi.spyOn(api, 'infraHosts').mockResolvedValue({ hosts: [] } as any)
    q.useInfraHosts('0', '1')
    const opts = useQueryMock.mock.calls.at(-1)![0] as any
    expect(opts.queryKey.value.at(-1)).toBeNull()
    await opts.queryFn({ signal: undefined })
    expect(spy).toHaveBeenCalledWith('0', '1', undefined, expect.anything())
  })
})

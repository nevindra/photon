import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// Mock useQuery so we can assert the plain options object each wrapper builds (mirrors the
// caching-options block in tracesQueries.test.js).
const useQueryMock = vi.fn(() => ({}))
vi.mock('@tanstack/vue-query', async (orig) => ({
  ...(await orig()),
  useQuery: (opts) => useQueryMock(opts),
}))

import { useFacet, useHistogram, useSearchLogs } from '@/lib/logs/logsQueries'
import { keepPreviousData } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import { setScope, clearScope } from '@/lib/core/context'

describe('logsQueries caching options', () => {
  beforeEach(() => useQueryMock.mockClear())
  it.each([
    ['facet', () => useFacet('service.name', '', '0', '1')],
    ['histogram', () => useHistogram('', '0', '1')],
  ])('%s sets staleTime + gcTime + keepPreviousData', (_name, call) => {
    call()
    const opts = useQueryMock.mock.calls.at(-1)[0]
    expect(opts.staleTime).toBe(30_000)
    expect(opts.gcTime).toBe(5 * 60_000)
    expect(opts.placeholderData).toBe(keepPreviousData)
  })
})

// Task 12: `tenant` is the first ScopeType that actually filters — every composable here appends
// the active scope's grammar term to its request (and re-keys on it), so a tenant scope narrows
// logs without the caller (LogsView) having to know about scope at all.
describe('logsQueries tenant scope', () => {
  beforeEach(() => {
    useQueryMock.mockClear()
    clearScope()
  })
  afterEach(() => clearScope())

  it('useSearchLogs appends the tenant term to the request query and the cache key', async () => {
    const spy = vi.spyOn(api, 'search').mockResolvedValue({ rows: [], matched_count: 0, elapsed_ms: 0 })
    setScope({ type: 'tenant', id: 'divtik', label: 'divtik' })
    useSearchLogs('key', () => ({ start_ts_nanos: '0', end_ts_nanos: '1', query: 'level:error', limit: 10 }))
    const opts = useQueryMock.mock.calls.at(-1)[0]

    expect(opts.queryKey.value).toContain('tenant:divtik')
    await opts.queryFn({ signal: undefined })
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ query: 'level:error tenant:divtik' }),
      expect.anything(),
    )
  })

  it('adds no term when scope is absent or non-tenant', () => {
    useSearchLogs('key', () => ({ start_ts_nanos: '0', end_ts_nanos: '1', query: 'level:error', limit: 10 }))
    expect(useQueryMock.mock.calls.at(-1)[0].queryKey.value.at(-1)).toBeNull()

    setScope({ type: 'service', id: 'checkout', label: 'checkout' })
    useSearchLogs('key', () => ({ start_ts_nanos: '0', end_ts_nanos: '1', query: '', limit: 10 }))
    expect(useQueryMock.mock.calls.at(-1)[0].queryKey.value.at(-1)).toBeNull()
  })
})

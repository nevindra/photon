import { describe, it, expect, vi, beforeEach } from 'vitest'

// Mock useQuery so we can assert the plain options object each wrapper builds (mirrors the
// caching-options block in tracesQueries.test.js).
const useQueryMock = vi.fn(() => ({}))
vi.mock('@tanstack/vue-query', async (orig) => ({
  ...(await orig()),
  useQuery: (opts) => useQueryMock(opts),
}))

import { useFacet, useHistogram } from '@/lib/logs/logsQueries'
import { keepPreviousData } from '@tanstack/vue-query'

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

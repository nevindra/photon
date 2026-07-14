import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest'
import { defineComponent, ref } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import { useSearchTraces, useSearchSpans } from '@/lib/traces/tracesQueries'

// Neither `useSearchTraces` nor `useSearchSpans` had a dedicated test file before this one —
// like `metricsQueries.test.js`/`dataQueries.test.js`, `useQuery`/`useInfiniteQuery` require an
// active Vue injection context (`QueryClient` via `VueQueryPlugin`), so we reuse the codebase's
// established harness pattern: a tiny inline component + mount + flushPromises.
afterEach(() => vi.restoreAllMocks())

function mountHarness(setupFn) {
  const Harness = defineComponent({
    setup() {
      return { result: setupFn() }
    },
    render: () => null,
  })
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const wrapper = mount(Harness, { global: { plugins: [[VueQueryPlugin, { queryClient }]] } })
  return { wrapper, queryClient, query: wrapper.vm.result }
}

describe('tracesQueries', () => {
  describe('useSearchTraces', () => {
    it('keys off the relative descriptor and calls api.searchTraces via buildRequest(cursor)', async () => {
      const spy = vi
        .spyOn(api, 'searchTraces')
        .mockResolvedValue({ traces: [{ trace_id: 't1' }], matched_count: 1, elapsed_ms: 2, next_cursor: null })
      const descriptor = ref('q=foo|1h')
      const buildRequest = (cursor) => ({ query: 'foo', start: '0', end: '10', cursor })

      const { query, queryClient } = mountHarness(() => useSearchTraces(descriptor, buildRequest))
      await flushPromises()

      expect(spy).toHaveBeenCalledWith(
        expect.objectContaining({ query: 'foo', cursor: undefined }),
        expect.objectContaining({ signal: expect.anything() }),
      )
      const keys = queryClient.getQueryCache().getAll().map((q) => q.queryKey)
      expect(keys).toContainEqual(['search-traces', 'q=foo|1h'])
      expect(query.data.value.pages[0].traces).toEqual([{ trace_id: 't1' }])
    })

    it('paginates via next_cursor from the previous page', async () => {
      const spy = vi
        .spyOn(api, 'searchTraces')
        .mockResolvedValueOnce({ traces: [{ trace_id: 't1' }], matched_count: 2, elapsed_ms: 1, next_cursor: 'cursor-2' })
        .mockResolvedValueOnce({ traces: [{ trace_id: 't2' }], matched_count: 2, elapsed_ms: 1, next_cursor: null })
      const descriptor = ref('q=|1h')
      const buildRequest = (cursor) => ({ query: '', start: '0', end: '10', cursor })

      const { query } = mountHarness(() => useSearchTraces(descriptor, buildRequest))
      await flushPromises()
      expect(query.hasNextPage.value).toBe(true)

      await query.fetchNextPage()
      await flushPromises()

      expect(spy).toHaveBeenLastCalledWith(
        expect.objectContaining({ cursor: 'cursor-2' }),
        expect.objectContaining({ signal: expect.anything() }),
      )
      expect(query.data.value.pages).toHaveLength(2)
      expect(query.hasNextPage.value).toBe(false)
    })
  })

  describe('useSearchSpans', () => {
    it('accepts (descriptor, buildRequest, opts), keys off the descriptor under "spans-search", and calls api.searchSpans', async () => {
      const spy = vi
        .spyOn(api, 'searchSpans')
        .mockResolvedValue({ rows: [{ span_id: 's1' }], matched_count: 1, elapsed_ms: 3, next_cursor: null })
      const descriptor = ref('q=bar|30m')
      const buildRequest = (cursor) => ({ query: 'bar', start: '0', end: '10', cursor })

      const { query, queryClient } = mountHarness(() => useSearchSpans(descriptor, buildRequest, { refetchInterval: false }))
      await flushPromises()

      expect(spy).toHaveBeenCalledWith(
        expect.objectContaining({ query: 'bar', cursor: undefined }),
        expect.objectContaining({ signal: expect.anything() }),
      )
      const keys = queryClient.getQueryCache().getAll().map((q) => q.queryKey)
      expect(keys).toContainEqual(['spans-search', 'q=bar|30m'])
      // Result pages expose `rows` (not `traces`).
      expect(query.data.value.pages[0].rows).toEqual([{ span_id: 's1' }])
      expect(query.data.value.pages[0].matched_count).toBe(1)
      expect(query.data.value.pages[0].elapsed_ms).toBe(3)
    })

    it('paginates via next_cursor, mirroring useSearchTraces', async () => {
      const spy = vi
        .spyOn(api, 'searchSpans')
        .mockResolvedValueOnce({ rows: [{ span_id: 's1' }], matched_count: 2, elapsed_ms: 1, next_cursor: 'cursor-2' })
        .mockResolvedValueOnce({ rows: [{ span_id: 's2' }], matched_count: 2, elapsed_ms: 1, next_cursor: null })
      const descriptor = ref('q=|30m')
      const buildRequest = (cursor) => ({ query: '', start: '0', end: '10', cursor })

      const { query } = mountHarness(() => useSearchSpans(descriptor, buildRequest))
      await flushPromises()
      expect(query.hasNextPage.value).toBe(true)

      await query.fetchNextPage()
      await flushPromises()

      expect(spy).toHaveBeenLastCalledWith(
        expect.objectContaining({ cursor: 'cursor-2' }),
        expect.objectContaining({ signal: expect.anything() }),
      )
      expect(query.data.value.pages.map((p) => p.rows[0].span_id)).toEqual(['s1', 's2'])
      expect(query.hasNextPage.value).toBe(false)
    })

    it('exposes the same surface as useSearchTraces (isFetching, isFetchingNextPage, error, errorUpdatedAt, dataUpdatedAt)', async () => {
      vi.spyOn(api, 'searchSpans').mockResolvedValue({ rows: [], matched_count: 0, elapsed_ms: 0, next_cursor: null })
      const { query } = mountHarness(() => useSearchSpans(ref('q=|30m'), () => ({ query: '', start: '0', end: '10' })))
      await flushPromises()

      for (const key of ['isFetching', 'isFetchingNextPage', 'error', 'errorUpdatedAt', 'dataUpdatedAt', 'hasNextPage', 'fetchNextPage']) {
        expect(query).toHaveProperty(key)
      }
    })
  })
})

// These wrappers build a plain options object we can assert on by mocking useQuery.
const useQueryMock = vi.fn(() => ({}))
vi.mock('@tanstack/vue-query', async (orig) => ({
  ...(await orig()),
  useQuery: (opts) => useQueryMock(opts),
}))

import { useTracesFacet, useTracesHistogram, useTracesLatency } from '@/lib/traces/tracesQueries'
import { keepPreviousData } from '@tanstack/vue-query'

describe('facet/histogram/latency caching options', () => {
  beforeEach(() => useQueryMock.mockClear())
  it.each([
    ['facet', () => useTracesFacet('service.name', '', '0', '1')],
    ['histogram', () => useTracesHistogram('', '0', '1')],
    ['latency', () => useTracesLatency('', '0', '1')],
  ])('%s sets staleTime + keepPreviousData', (_name, call) => {
    call()
    const opts = useQueryMock.mock.calls.at(-1)[0]
    expect(opts.staleTime).toBe(30_000)
    expect(opts.gcTime).toBe(5 * 60_000)
    expect(opts.placeholderData).toBe(keepPreviousData)
  })
})

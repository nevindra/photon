import { describe, it, expect, vi, afterEach } from 'vitest'
import { defineComponent, ref } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import { storageQueryKey, retentionQueryKey, useStorage, useRetention, useSetRetention, usePurge, useUsageSeries } from '@/lib/data/dataQueries'

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

// `usersQueries.js` (the sibling module this file mirrors) has no dedicated test file of its own
// in this codebase either — every query composable is only ever exercised inside a mounted
// component, because `useQuery`/`useMutation` require an active Vue injection context. So, like
// `metricsQueries.test.js`, this is a smoke-import assertion plus a check that the query keys are
// stable — the composables get full behavioral coverage once the Data & Retention view (Task 13)
// composes them.
describe('dataQueries', () => {
  it('exports the documented composables', () => {
    expect(typeof useStorage).toBe('function')
    expect(typeof useRetention).toBe('function')
    expect(typeof useSetRetention).toBe('function')
    expect(typeof usePurge).toBe('function')
    expect(typeof useUsageSeries).toBe('function')
  })

  it('query keys are stable', () => {
    expect(storageQueryKey()).toEqual(['storage'])
    expect(retentionQueryKey()).toEqual(['retention'])
  })

  it('useUsageSeries calls api.getUsageSeries with the window and keys on it', async () => {
    const spy = vi
      .spyOn(api, 'getUsageSeries')
      .mockResolvedValue({ window: '7d', bucket_ms: 1800000, series: { logs: [], traces: [], metrics: [] } })
    const { query, queryClient } = mountHarness(() => useUsageSeries(ref('7d')))
    await flushPromises()

    expect(spy).toHaveBeenCalledWith({ window: '7d' }, expect.objectContaining({ signal: expect.anything() }))
    const keys = queryClient.getQueryCache().getAll().map((q) => q.queryKey)
    expect(keys).toContainEqual(['usage-series', '7d'])
    expect(query.data.value.window).toBe('7d')
  })
})

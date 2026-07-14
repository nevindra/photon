import { describe, it, expect, vi, afterEach } from 'vitest'
import { defineComponent, ref } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import { useMetricCatalog, useMetricMetadata, useMetricLabels, useMetricSeries } from '@/lib/metrics/metricsQueries'

// `tracesQueries.js` (the sibling module this file mirrors) has no dedicated test file of its
// own in this codebase — every query composable here is only ever exercised inside a mounted
// component (see LatencyHistogram.test.js, SpanFacetRail.test.js, MetricsView.test.js, etc.),
// because `useQuery` requires an active Vue injection context (`QueryClient` provided via
// `VueQueryPlugin`) and throws when called directly from a plain function. So there is no
// established "call the composable outside setup" harness to mirror; instead we reuse the
// codebase's real, load-bearing pattern — a tiny inline harness component + VueQueryPlugin — to
// exercise `useMetricSeries`'s relative-key/buildRequest wiring, plus a smoke-import assertion
// for the other three composables (kept untested-in-detail here; they're simple `useQuery`
// wrappers with no branching logic, and get full behavioral coverage once the view (Task 12)
// composes them).
afterEach(() => vi.restoreAllMocks())

describe('metricsQueries', () => {
  it('exports the documented composables', () => {
    expect(typeof useMetricCatalog).toBe('function')
    expect(typeof useMetricMetadata).toBe('function')
    expect(typeof useMetricLabels).toBe('function')
    expect(typeof useMetricSeries).toBe('function')
  })

  it('useMetricSeries builds a relative key and calls api.metricQuery via buildRequest', async () => {
    const spy = vi
      .spyOn(api, 'metricQuery')
      .mockResolvedValue({ results: [], step: '1', capped: false, elapsed_ms: 0 })
    const buildRequest = () => ({
      queries: [{ id: 'a', metric: 'm', group_by: [], filter: '' }],
      start: '0',
      end: '10',
    })
    const Harness = defineComponent({
      setup() {
        useMetricSeries(ref('m|avg||30m'), buildRequest)
        return () => null
      },
    })
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    mount(Harness, { global: { plugins: [[VueQueryPlugin, { queryClient }]] } })
    await flushPromises()

    // queryFn is wired to buildRequest() (resolved at fetch time, not baked into the key).
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ start: '0', end: '10' }),
      expect.objectContaining({ signal: expect.anything() }),
    )
    // The cache key registered with TanStack carries only the RELATIVE descriptor —
    // never the absolute start/end ns — so live-tail refetches don't churn the cache key.
    const keys = queryClient.getQueryCache().getAll().map((q) => q.queryKey)
    expect(keys).toContainEqual(['metric-series', 'm|avg||30m'])
    expect(JSON.stringify(keys)).not.toContain('"10"')
  })
})

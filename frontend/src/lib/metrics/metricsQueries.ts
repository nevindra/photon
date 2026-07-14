// Thin TanStack Query wrappers over the `api.metric*` methods (OTLP Metrics Explorer, Phase 3).
// Same contract as `tracesQueries.js`: reactive inputs (refs OR getters, normalized with
// `toValue`), a reactive `queryKey` for refetch-on-change + dedup, and a threaded AbortSignal.
import { computed, toValue } from 'vue'
import type { MaybeRefOrGetter } from 'vue'
import { useQuery, keepPreviousData } from '@tanstack/vue-query'
import type { UseQueryOptions } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import type { MetricQueryRequest, MetricQueryResponse } from '@/lib/core/api'

export function useMetricCatalog(startNs: MaybeRefOrGetter<string>, endNs: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => ['metric-catalog', String(toValue(startNs)), String(toValue(endNs))]),
    queryFn: ({ signal }) => api.metricCatalog(toValue(startNs), toValue(endNs), {}, { signal }),
    staleTime: 30 * 1000,
  })
}

export function useMetricMetadata(
  name: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => ['metric-metadata', toValue(name), String(toValue(startNs)), String(toValue(endNs))]),
    queryFn: ({ signal }) => api.metricMetadata(toValue(name), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(name)),
    staleTime: 30 * 1000,
  })
}

export function useMetricLabels(
  metric: MaybeRefOrGetter<string>,
  key: MaybeRefOrGetter<string | null | undefined>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => ['metric-labels', toValue(metric), toValue(key) ?? null, String(toValue(startNs)), String(toValue(endNs))]),
    queryFn: ({ signal }) => api.metricLabels(toValue(metric), toValue(key) ?? null, toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(metric) && !!toValue(key)),
  })
}

// Live-capable series query. `seriesKey` is the RELATIVE descriptor (metric|agg|groupBy|filter|range);
// absolute start/end/step are resolved fresh inside buildRequest() on every fetch (incl. live-tail
// refetch), so the window advances without churning the cache key.
export function useMetricSeries(
  seriesKey: MaybeRefOrGetter<string>,
  buildRequest: () => MetricQueryRequest,
  options: Partial<UseQueryOptions<MetricQueryResponse>> = {},
) {
  return useQuery({
    queryKey: computed(() => ['metric-series', toValue(seriesKey)]),
    queryFn: ({ signal }) => api.metricQuery(buildRequest(), { signal }),
    placeholderData: keepPreviousData,
    ...options,
  })
}

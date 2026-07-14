// Thin TanStack Query wrappers over the `api.*` trace/span methods. Same contract as
// `logsQueries.js`: reactive inputs (refs OR getters, normalized with `toValue`), a reactive
// `queryKey` for refetch-on-change + dedup, and a threaded AbortSignal for cancellation. Trace
// and span search are cursor-paginated, so they use `useInfiniteQuery`. No business logic here —
// views compose these and read `.data`/`.isFetching`/`.fetchNextPage`.
import { computed, toValue } from 'vue'
import type { MaybeRefOrGetter } from 'vue'
import { useQuery, useInfiniteQuery, keepPreviousData } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import type {
  FieldInfo,
  FacetResult,
  TraceSearchRequest,
  TraceSearchResult,
  SpanSearchRequest,
  SpanSearchResult,
  TracesHistogramBucket,
  LatencyResult,
  RedGroup,
  RedRow,
} from '@/lib/core/api'

// Extra reactive query options threaded through to `useInfiniteQuery`/`useQuery` (e.g.
// `enabled`, `refetchInterval`, `placeholderData`) — vue-query's option surface is deep-generic
// over TQueryFnData/TPageParam/etc, so a precise passthrough type isn't worth modeling here;
// `any` is a targeted escape hatch for this spread only, callers still get typed params/returns.
type ExtraQueryOptions = Record<string, any>

// Trace field catalog for a time window.
export function useTracesFields(startNs: MaybeRefOrGetter<string>, endNs: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => ['traces-fields', String(toValue(startNs)), String(toValue(endNs))]),
    queryFn: ({ signal }): Promise<FieldInfo[]> => api.tracesFields(toValue(startNs), toValue(endNs), { signal }),
  })
}

// Trace search — cursor-paginated (`useInfiniteQuery`). Split into a RELATIVE key and a
// FETCH-TIME request builder, exactly like `logsQueries.useSearchLogs`:
//
//   - `searchKey` (ref/getter) is the cache key: the *relative* descriptor of the search (query
//     text, time range, custom range, sort, limit). It must NOT contain the now-anchored absolute
//     window, which would churn the key every millisecond and defeat dedupe/caching.
//   - `buildRequest(cursor)` is called at FETCH time — for every page and on every refetch,
//     including each `refetchInterval` live-tail poll — and returns the full request envelope with
//     absolute start/end ns resolved against the current clock, plus the page `cursor` (undefined
//     = first page). Resolving the window here is what advances it to "now" without touching the
//     key; the view refreshes its `nowTick` on the first page so all pages of one cycle agree.
//   - `options` threads extra reactive query options (e.g. `refetchInterval`, `placeholderData`);
//     refs/computeds are supported (vue-query deep-unwraps them).
export function useSearchTraces(
  searchKey: MaybeRefOrGetter<unknown>,
  buildRequest: (cursor: string | undefined) => TraceSearchRequest,
  options: ExtraQueryOptions = {},
) {
  return useInfiniteQuery({
    queryKey: computed(() => ['search-traces', toValue(searchKey)]),
    queryFn: ({ pageParam, signal }): Promise<TraceSearchResult> =>
      api.searchTraces(buildRequest(pageParam), { signal }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last: TraceSearchResult) => last.next_cursor ?? undefined,
    ...options,
  })
}

// Span search — infinite, mirrors useSearchTraces exactly (same relative-key / fetch-time
// buildRequest split; see the comment above useSearchTraces). Result pages expose `rows` (not
// `traces`).
export function useSearchSpans(
  searchKey: MaybeRefOrGetter<unknown>,
  buildRequest: (cursor: string | undefined) => SpanSearchRequest,
  options: ExtraQueryOptions = {},
) {
  return useInfiniteQuery({
    queryKey: computed(() => ['spans-search', toValue(searchKey)]),
    queryFn: ({ pageParam, signal }): Promise<SpanSearchResult> =>
      api.searchSpans(buildRequest(pageParam), { signal }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last: SpanSearchResult) => last.next_cursor ?? undefined,
    ...options,
  })
}

// A single trace + its spans. Only fires once a traceId is set (enabled guard); `timeHintNs`
// narrows the manifest lookup and is part of the cache key.
export function useTrace(traceId: MaybeRefOrGetter<string>, timeHintNs: MaybeRefOrGetter<string | null | undefined>) {
  return useQuery({
    queryKey: computed(() => ['trace', toValue(traceId), String(toValue(timeHintNs) ?? '')]),
    queryFn: ({ signal }) => api.getTrace(toValue(traceId), toValue(timeHintNs), { signal }),
    enabled: computed(() => !!toValue(traceId)),
  })
}

// Top values by count for one trace field. Only fires once a field is chosen (enabled guard).
export function useTracesFacet(
  field: MaybeRefOrGetter<string>,
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  limit: MaybeRefOrGetter<number> = 50,
) {
  return useQuery({
    queryKey: computed(() => [
      'traces-facet',
      toValue(field),
      toValue(query),
      String(toValue(startNs)),
      String(toValue(endNs)),
      toValue(limit),
    ]),
    queryFn: ({ signal }): Promise<FacetResult> =>
      api.tracesFacet(toValue(field), toValue(query), toValue(startNs), toValue(endNs), toValue(limit), { signal }),
    enabled: computed(() => !!toValue(field)),
    staleTime: 30_000,
    gcTime: 5 * 60_000,
    placeholderData: keepPreviousData,
  })
}

// Span-volume histogram for a window + query.
export function useTracesHistogram(
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number> = 48,
) {
  return useQuery({
    queryKey: computed(() => [
      'traces-histogram',
      toValue(query),
      String(toValue(startNs)),
      String(toValue(endNs)),
      toValue(buckets),
    ]),
    queryFn: ({ signal }): Promise<TracesHistogramBucket[]> =>
      api.tracesHistogram(toValue(query), toValue(startNs), toValue(endNs), toValue(buckets), { signal }),
    staleTime: 30_000,
    gcTime: 5 * 60_000,
    placeholderData: keepPreviousData,
  })
}

// Latency-distribution histogram for a window + query.
export function useTracesLatency(
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number> = 48,
) {
  return useQuery({
    queryKey: computed(() => [
      'traces-latency',
      toValue(query),
      String(toValue(startNs)),
      String(toValue(endNs)),
      toValue(buckets),
    ]),
    queryFn: ({ signal }): Promise<LatencyResult> =>
      api.tracesLatency(toValue(query), toValue(startNs), toValue(endNs), toValue(buckets), { signal }),
    staleTime: 30_000,
    gcTime: 5 * 60_000,
    placeholderData: keepPreviousData,
  })
}

// RED metrics for a window + query, grouped by operation or service. Small bounded result — a
// plain `useQuery` (no pagination). The Metrics view calls this twice (current + previous window)
// to derive KPI trend deltas.
export function useRed(
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  group: MaybeRefOrGetter<RedGroup> = 'operation',
) {
  return useQuery({
    queryKey: computed(() => [
      'red',
      toValue(query),
      String(toValue(startNs)),
      String(toValue(endNs)),
      toValue(group),
    ]),
    queryFn: ({ signal }): Promise<RedRow[]> =>
      api.red(toValue(query), toValue(startNs), toValue(endNs), toValue(group), { signal }),
  })
}

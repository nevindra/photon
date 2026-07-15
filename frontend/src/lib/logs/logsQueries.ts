// Thin TanStack Query wrappers over the `api.*` log methods. Each accepts reactive inputs (refs
// OR getter functions, normalized with `toValue`), derives a reactive `queryKey` so identical
// requests dedupe and changed inputs refetch, and threads the AbortSignal so superseded requests
// cancel. No business logic lives here — views compose these and read `.data`/`.isFetching`.
import { computed, toValue } from 'vue'
import type { MaybeRefOrGetter } from 'vue'
import { useQuery, keepPreviousData } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import type { LogSearchRequest } from '@/lib/core/api'

// Distinct service names. Rarely changes, so a long staleTime avoids refetching on every mount.
export function useServices() {
  return useQuery({
    queryKey: ['services'],
    queryFn: ({ signal }) => api.services({ signal }),
    staleTime: 5 * 60 * 1000,
  })
}

// Field catalog for a time window (manifest-only, no data scan).
export function useFields(startNs: MaybeRefOrGetter<string>, endNs: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => ['fields', String(toValue(startNs)), String(toValue(endNs))]),
    queryFn: ({ signal }) => api.fields(toValue(startNs), toValue(endNs), { signal }),
    // The field set rarely changes; hold it fresh for a bucket-width so a remount within the same
    // (60s-rounded) window serves from cache instead of refetching (new attribute keys still surface
    // within a minute, and any window change is a new key that refetches immediately).
    staleTime: 60_000,
  })
}

// Row search — split into a RELATIVE key and a FETCH-TIME request builder.
//
//   - `searchKey` (ref/getter) is the cache key: the *relative* descriptor of the search — query
//     text, time range, custom range, limit. It must NOT contain the now-anchored absolute window,
//     which would churn the key every millisecond and defeat caching/dedupe.
//   - `buildRequest()` is called at FETCH time (inside the queryFn, on every refetch — including
//     each `refetchInterval` live-tail poll) and returns the full request envelope with absolute
//     start/end ns resolved against the current clock. This is what advances the window to "now"
//     without touching the key.
//   - `options` threads through extra reactive query options (e.g. `refetchInterval`, `enabled`);
//     refs/computeds are supported (vue-query deep-unwraps them). Loosely typed: it's a passthrough
//     spread of arbitrary `useQuery` option overrides, not a shape this module owns.
//
// `placeholderData: keepPreviousData` keeps the last good page (and its matched_count/elapsed_ms)
// on screen while a new or failed search resolves — matching the pre-Query behavior where a bad
// query left the previous rows visible under the error underline.
export function useSearchLogs(
  searchKey: MaybeRefOrGetter<unknown>,
  buildRequest: () => LogSearchRequest,
  options: Record<string, unknown> = {},
) {
  return useQuery({
    queryKey: computed(() => ['search-logs', toValue(searchKey)]),
    queryFn: ({ signal }) => api.search(buildRequest(), { signal }),
    placeholderData: keepPreviousData,
    ...options,
  })
}

// Top values by count for one field. Only fires once a field is chosen (enabled guard).
export function useFacet(
  field: MaybeRefOrGetter<string>,
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  limit: MaybeRefOrGetter<number> = 50,
) {
  return useQuery({
    queryKey: computed(() => [
      'facet',
      toValue(field),
      toValue(query),
      String(toValue(startNs)),
      String(toValue(endNs)),
      toValue(limit),
    ]),
    queryFn: ({ signal }) =>
      api.facet(toValue(field), toValue(query), toValue(startNs), toValue(endNs), toValue(limit), { signal }),
    enabled: computed(() => !!toValue(field)),
    staleTime: 30_000,
    gcTime: 5 * 60_000,
    placeholderData: keepPreviousData,
  })
}

// Severity-stacked volume histogram for a window + query.
export function useHistogram(
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number> = 48,
) {
  return useQuery({
    queryKey: computed(() => [
      'histogram',
      toValue(query),
      String(toValue(startNs)),
      String(toValue(endNs)),
      toValue(buckets),
    ]),
    queryFn: ({ signal }) =>
      api.histogram(toValue(query), toValue(startNs), toValue(endNs), toValue(buckets), { signal }),
    staleTime: 30_000,
    gcTime: 5 * 60_000,
    placeholderData: keepPreviousData,
  })
}

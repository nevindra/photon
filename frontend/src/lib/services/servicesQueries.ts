// Thin TanStack Query wrappers over the `api.*` service (APM) methods. Same contract as
// `tracesQueries.js`/`uptimeQueries.js`: reactive inputs (refs OR getter functions), normalized
// with `toValue`, feed a reactive `queryKey` for refetch-on-change + dedup, and a threaded
// AbortSignal for cancellation. `useServicesList` mirrors `tracesQueries.useRed` exactly — it's
// the RED table grouped by `service` instead of `operation` (rows already carry `apdex`) — and
// polls every 15s like the monitors dashboard (`uptimeQueries.useMonitors`) for live-tail, since
// the services list has no search-bar/drawer pause logic to gate on (unlike logs/traces search).
// The settings mutations follow `uptimeQueries.js`'s pattern: `api.setServiceSettings` has no
// internal try/catch for a 400 and throws a real ky `HTTPError` (`.body.error` via the
// `beforeError` hook), so success/failure branch via `onSuccess`/`onError`, not a resolved
// `{ ok, error }` value. On success they invalidate both the services list (so Apdex/KPIs
// recompute) and this service's own detail keys via a queryKey PREFIX match (TanStack's default
// `exact: false`) — `['service', service]` invalidates timeseries/deps/settings for that service
// in one call.
import { computed, toValue, type MaybeRefOrGetter } from 'vue'
import { useQuery, useMutation, useQueryClient, type QueryClient } from '@tanstack/vue-query'
import { api, type ApiError, type ServiceDependencies, type ServiceSettings, type ServiceTimeseriesBucket } from '@/lib/core/api'
import { toast } from '@/components/ui/toast'

export const servicesListQueryKey = (
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) => [
  'services',
  'red',
  toValue(query),
  String(toValue(startNs)),
  String(toValue(endNs)),
]
export const serviceTimeseriesQueryKey = (
  service: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number>,
) => [
  'service',
  'timeseries',
  toValue(service),
  String(toValue(startNs)),
  String(toValue(endNs)),
  toValue(buckets),
]
export const serviceDependenciesQueryKey = (
  service: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) => [
  'service',
  'deps',
  toValue(service),
  String(toValue(startNs)),
  String(toValue(endNs)),
]
export const serviceSettingsQueryKey = (service: MaybeRefOrGetter<string>) => [
  'service',
  'settings',
  toValue(service),
]

// Services (APM) list — RED metrics grouped by service (`api.red(..., 'service')`); rows already
// carry `apdex`. Small bounded result like `useRed`, so a plain `useQuery` — polled for live-tail.
export function useServicesList(
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => servicesListQueryKey(query, startNs, endNs)),
    queryFn: ({ signal }) => api.red(toValue(query), toValue(startNs), toValue(endNs), 'service', { signal }),
    refetchInterval: 15_000,
  })
}

// Request-rate/latency/error timeseries for one service's detail charts. Only fires once a
// service is set (enabled guard, same as `useTrace`/`useMonitor`).
export function useServiceTimeseries(
  service: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number> = 48,
) {
  return useQuery<ServiceTimeseriesBucket[]>({
    queryKey: computed(() => serviceTimeseriesQueryKey(service, startNs, endNs, buckets)),
    queryFn: ({ signal }) =>
      api.serviceTimeseries(
        toValue(service),
        { start: toValue(startNs), end: toValue(endNs), buckets: toValue(buckets) },
        { signal },
      ),
    enabled: computed(() => !!toValue(service)),
  })
}

// Upstream/downstream dependency graph for one service's detail page.
export function useServiceDependencies(
  service: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery<ServiceDependencies>({
    queryKey: computed(() => serviceDependenciesQueryKey(service, startNs, endNs)),
    queryFn: ({ signal }) =>
      api.serviceDependencies(toValue(service), { start: toValue(startNs), end: toValue(endNs) }, { signal }),
    enabled: computed(() => !!toValue(service)),
  })
}

// A service's Apdex threshold (or the default, flagged via `is_default`).
export function useServiceSettings(service: MaybeRefOrGetter<string>) {
  return useQuery<ServiceSettings>({
    queryKey: computed(() => serviceSettingsQueryKey(service)),
    queryFn: ({ signal }) => api.serviceSettings(toValue(service), { signal }),
    enabled: computed(() => !!toValue(service)),
  })
}

function invalidateServicesList(qc: QueryClient) {
  return qc.invalidateQueries({ queryKey: ['services'] })
}

// PREFIX match (default `exact: false`) — invalidates timeseries/deps/settings for this service
// in one call.
function invalidateService(qc: QueryClient, service: string) {
  return qc.invalidateQueries({ queryKey: ['service', service] })
}

// Unlike the user/data mutations, `api.setServiceSettings`/`resetServiceSettings` have no
// internal try/catch — a non-2xx response throws a real ky `HTTPError` (see `lib/api.js`'s
// `beforeError` hook, which attaches `.body.error` from the server's JSON error body) that rejects
// the mutation and lands in `onError` below. Fall back to a generic sentence for network failures
// or bodies without a parsed error message.
function mutationErrorMessage(err: unknown, fallback: string): string {
  return (err as ApiError)?.body?.error ?? fallback
}

export function useSetServiceSettings() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ service, ms }: { service: string; ms: number }) => api.setServiceSettings(service, ms),
    onSuccess: (_data, { service }) => {
      invalidateServicesList(qc)
      invalidateService(qc, service)
      toast({ variant: 'success', title: 'Apdex threshold updated' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't update Apdex threshold",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

export function useResetServiceSettings() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (service: string) => api.resetServiceSettings(service),
    onSuccess: (_data, service) => {
      invalidateServicesList(qc)
      invalidateService(qc, service)
      toast({ variant: 'success', title: 'Apdex threshold reset to default' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't reset Apdex threshold",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

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
import { useQuery, useMutation, useQueryClient, keepPreviousData, type QueryClient } from '@tanstack/vue-query'
import {
  api,
  type ApiError,
  type BackendIssueDetail,
  type BackendIssueFacetsResult,
  type BackendIssuesResult,
  type DeploymentsResult,
  type Slo,
  type SloBudget,
  type SloInput,
  type LatencyHeatmap,
  type ServiceDependencies,
  type ServiceMapResult,
  type ServiceSettings,
  type ServiceTimeseriesBucket,
} from '@/lib/core/api'
import { contextQueryTerm, environment, tenant, withContext } from '@/lib/core/context'
import { grammarTerm } from '@/lib/core/queryLang'
import { toast } from '@/components/ui/toast'

// The Backend (APM) vertical filters by the global context (Site + Environment) like
// logs/traces/metrics — `withContext` is the shared appender, `contextQueryTerm()` the cache key
// slot it re-keys on.

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
    queryKey: computed(() => [...servicesListQueryKey(query, startNs, endNs), contextQueryTerm()]),
    queryFn: ({ signal }) => api.red(withContext(toValue(query)), toValue(startNs), toValue(endNs), 'service', { signal }),
    refetchInterval: 15_000,
  })
}

export const serviceOperationsQueryKey = (
  service: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) => [
  'service',
  'operations',
  toValue(service),
  String(toValue(startNs)),
  String(toValue(endNs)),
]

/**
 * One service's operations (`api.red(..., 'operation')` narrowed to that service).
 *
 * The term goes through `grammarTerm`, which quotes a name containing whitespace or a comma:
 * unquoted, `service.name:my api` parses as a service plus a stray free-text word, and the panel
 * would silently describe a different set of spans than the row it hangs off.
 *
 * `enabled` is a parameter, not `!!service` — the index expands one row at a time and must not
 * fire a query per collapsed row.
 */
export function useServiceOperations(
  service: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  enabled: MaybeRefOrGetter<boolean> = true,
) {
  return useQuery({
    queryKey: computed(() => [...serviceOperationsQueryKey(service, startNs, endNs), contextQueryTerm()]),
    queryFn: ({ signal }) =>
      api.red(
        withContext(grammarTerm('service.name', toValue(service)) ?? ''),
        toValue(startNs),
        toValue(endNs),
        'operation',
        { signal },
      ),
    enabled: computed(() => !!toValue(service) && !!toValue(enabled)),
    staleTime: 15_000,
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
    queryKey: computed(() => [...serviceTimeseriesQueryKey(service, startNs, endNs, buckets), contextQueryTerm()]),
    queryFn: ({ signal }) =>
      api.serviceTimeseries(
        toValue(service),
        { start: toValue(startNs), end: toValue(endNs), buckets: toValue(buckets), q: contextQueryTerm() ?? undefined },
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
  enabled: MaybeRefOrGetter<boolean> = true,
) {
  return useQuery<ServiceDependencies>({
    queryKey: computed(() => [...serviceDependenciesQueryKey(service, startNs, endNs), contextQueryTerm()]),
    queryFn: ({ signal }) =>
      api.serviceDependencies(
        toValue(service),
        { start: toValue(startNs), end: toValue(endNs), q: contextQueryTerm() ?? undefined },
        { signal },
      ),
    enabled: computed(() => !!toValue(service) && !!toValue(enabled)),
  })
}

export const serviceOperationHeatmapQueryKey = (
  service: MaybeRefOrGetter<string>,
  operation: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number>,
) => [
  'service',
  'heatmap',
  toValue(service),
  toValue(operation),
  String(toValue(startNs)),
  String(toValue(endNs)),
  toValue(buckets),
]

/** One operation's latency distribution over time — the shape percentile lines cannot draw. */
export function useOperationHeatmap(
  service: MaybeRefOrGetter<string>,
  operation: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number> = 48,
) {
  return useQuery<LatencyHeatmap>({
    queryKey: computed(() => [
      ...serviceOperationHeatmapQueryKey(service, operation, startNs, endNs, buckets),
      contextQueryTerm(),
    ]),
    queryFn: ({ signal }) =>
      api.serviceOperationHeatmap(
        toValue(service),
        toValue(operation),
        contextQueryTerm() ?? '',
        toValue(startNs),
        toValue(endNs),
        toValue(buckets),
        { signal },
      ),
    enabled: computed(() => !!toValue(service) && !!toValue(operation)),
    placeholderData: keepPreviousData,
  })
}

export const backendIssuesQueryKey = (
  service: MaybeRefOrGetter<string | null>,
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) => [
  'service',
  'issues',
  toValue(service) ?? '__fleet__',
  toValue(query),
  String(toValue(startNs)),
  String(toValue(endNs)),
]

/**
 * Backend Issues for one service, or for the whole fleet when `service` is `null`.
 *
 * One composable for both because they are one list read two ways — the fleet view is the
 * per-service view without the service term, and giving them separate hooks is how the two drift
 * into disagreeing about ordering or limits.
 *
 * `placeholderData: keepPreviousData` so typing in the search bar dims the current rows instead
 * of blanking the table on every keystroke.
 */
export function useBackendIssues(
  service: MaybeRefOrGetter<string | null>,
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery<BackendIssuesResult>({
    queryKey: computed(() => [...backendIssuesQueryKey(service, query, startNs, endNs), contextQueryTerm()]),
    queryFn: ({ signal }) =>
      api.backendIssues(
        toValue(service),
        withContext(toValue(query)),
        toValue(startNs),
        toValue(endNs),
        { signal },
      ),
    placeholderData: keepPreviousData,
  })
}

export const backendIssueFacetsQueryKey = (
  service: MaybeRefOrGetter<string>,
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) => [
  'service',
  'issue-facets',
  toValue(service),
  toValue(query),
  String(toValue(startNs)),
  String(toValue(endNs)),
]

/** The six Issue facet fields for one service, all in one response. */
export function useBackendIssueFacets(
  service: MaybeRefOrGetter<string>,
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery<BackendIssueFacetsResult>({
    queryKey: computed(() => [...backendIssueFacetsQueryKey(service, query, startNs, endNs), contextQueryTerm()]),
    queryFn: ({ signal }) =>
      api.backendIssueFacets(
        toValue(service),
        withContext(toValue(query)),
        toValue(startNs),
        toValue(endNs),
        { signal },
      ),
    enabled: computed(() => !!toValue(service)),
    placeholderData: keepPreviousData,
  })
}

export const backendIssueDetailQueryKey = (
  service: MaybeRefOrGetter<string>,
  fingerprint: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) => [
  'service',
  'issue',
  toValue(service),
  toValue(fingerprint),
  String(toValue(startNs)),
  String(toValue(endNs)),
]

/** One Issue's full detail. Keyed on the PAIR, because that is the Issue's identity. */
export function useBackendIssueDetail(
  service: MaybeRefOrGetter<string>,
  fingerprint: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery<BackendIssueDetail>({
    queryKey: computed(() => [...backendIssueDetailQueryKey(service, fingerprint, startNs, endNs), contextQueryTerm()]),
    queryFn: ({ signal }) =>
      api.backendIssueDetail(
        toValue(service),
        toValue(fingerprint),
        contextQueryTerm() ?? '',
        toValue(startNs),
        toValue(endNs),
        { signal },
      ),
    enabled: computed(() => !!toValue(service) && !!toValue(fingerprint)),
  })
}

export const serviceMapQueryKey = (
  service: MaybeRefOrGetter<string | null>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) => ['service', 'map', toValue(service) ?? '__fleet__', String(toValue(startNs)), String(toValue(endNs))]

/**
 * The service topology — the fleet graph when `service` is `null`, that service's focused map
 * otherwise. One composable, because the focused map is the fleet map narrowed: two would be two
 * places for an edge's error count to drift.
 *
 * The service goes as its own param, NOT through `withContext`/the grammar `q`. Scoping the scan
 * to one service would delete the parent spans the join needs, and the map would come back empty
 * looking like "nothing calls this".
 */
export function useServiceMap(
  service: MaybeRefOrGetter<string | null>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  enabled: MaybeRefOrGetter<boolean> = true,
) {
  return useQuery<ServiceMapResult>({
    queryKey: computed(() => [...serviceMapQueryKey(service, startNs, endNs), contextQueryTerm()]),
    queryFn: ({ signal }) =>
      api.serviceMap(toValue(service), contextQueryTerm() ?? '', toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(enabled)),
    placeholderData: keepPreviousData,
  })
}

export const deploymentsQueryKey = (
  service: MaybeRefOrGetter<string | null>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) => ['service', 'deployments', toValue(service) ?? '__all__', String(toValue(startNs)), String(toValue(endNs))]

/**
 * Releases in the window. `service` is `null` for every service at once.
 *
 * The window is passed as ordinary NANOSECOND strings like everywhere else — the handler converts
 * to the milliseconds the table stores. The environment comes from the app context rather than
 * from a grammar `q`: this table is a control-plane table, not a span scan, so there is no query
 * language to compile against it.
 */
export function useDeployments(
  service: MaybeRefOrGetter<string | null>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  enabled: MaybeRefOrGetter<boolean> = true,
) {
  return useQuery<DeploymentsResult>({
    queryKey: computed(() => [...deploymentsQueryKey(service, startNs, endNs), environment.value, tenant.value]),
    queryFn: ({ signal }) =>
      api.deployments(
        {
          ...(toValue(service) ? { service: toValue(service) as string } : {}),
          ...(environment.value ? { environment: environment.value } : {}),
          ...(tenant.value ? { tenant: tenant.value } : {}),
          start: toValue(startNs),
          end: toValue(endNs),
        },
        { signal },
      ),
    enabled: computed(() => !!toValue(enabled)),
    placeholderData: keepPreviousData,
  })
}

export const slosQueryKey = (service: MaybeRefOrGetter<string>) => ['service', 'slos', toValue(service)]

/** A service's objectives. Not windowed: an SLO carries its own window (`window_days`). */
export function useSlos(service: MaybeRefOrGetter<string>) {
  return useQuery<{ slos: Slo[] }>({
    queryKey: computed(() => slosQueryKey(service)),
    queryFn: ({ signal }) => api.slos(toValue(service), { signal }),
    enabled: computed(() => !!toValue(service)),
  })
}

export const sloBudgetQueryKey = (service: MaybeRefOrGetter<string>, id: MaybeRefOrGetter<string>) => [
  'service',
  'slo-budget',
  toValue(service),
  toValue(id),
]

/**
 * One objective's budget and burn rate.
 *
 * Deliberately NOT keyed on the app's time window: a 30-day error budget is 30 days whatever the
 * page's picker says, and re-keying it on the window would make the same objective report four
 * different numbers as you flick between 15m and 24h. It polls instead, because a burn rate is a
 * now-fact.
 */
export function useSloBudget(service: MaybeRefOrGetter<string>, id: MaybeRefOrGetter<string>) {
  return useQuery<SloBudget>({
    queryKey: computed(() => sloBudgetQueryKey(service, id)),
    queryFn: ({ signal }) => api.sloBudget(toValue(service), toValue(id), { signal }),
    enabled: computed(() => !!toValue(service) && !!toValue(id)),
    refetchInterval: 60_000,
    placeholderData: keepPreviousData,
  })
}

function invalidateSlos(qc: QueryClient, service: string) {
  return qc.invalidateQueries({ queryKey: ['service', 'slos', service] })
}

export function useCreateSlo() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ service, input }: { service: string; input: SloInput }) => api.createSlo(service, input),
    onSuccess: (_data, { service }) => {
      invalidateSlos(qc, service)
      toast({ variant: 'success', title: 'Objective created' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't create the objective",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

export function useUpdateSlo() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ service, id, input }: { service: string; id: string; input: SloInput }) =>
      api.updateSlo(service, id, input),
    onSuccess: (_data, { service, id }) => {
      invalidateSlos(qc, service)
      qc.invalidateQueries({ queryKey: ['service', 'slo-budget', service, id] })
      toast({ variant: 'success', title: 'Objective updated' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't update the objective",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

export function useDeleteSlo() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ service, id }: { service: string; id: string }) => api.deleteSlo(service, id),
    onSuccess: (_data, { service }) => {
      invalidateSlos(qc, service)
      toast({ variant: 'success', title: 'Objective removed' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't remove the objective",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
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

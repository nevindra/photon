// Thin TanStack Query wrappers over the `api.rum*` methods (RUM: Core Web Vitals + JS errors).
// Same contract as `servicesQueries.js`/`metricsQueries.js`: reactive inputs (refs OR getter
// functions), normalized with `toValue`, feed a reactive `queryKey` for refetch-on-change +
// dedup, and a threaded AbortSignal for cancellation. `useRumApps` polls every 30s (app registry
// changes rarely); `useRumVitals` polls every 15s like the other live-tail dashboards and keeps
// the previous page's data on screen while the window/app changes (`keepPreviousData`).
import { computed, toValue } from 'vue'
import type { MaybeRefOrGetter } from 'vue'
import { useQuery, useQueries, useMutation, useQueryClient, keepPreviousData } from '@tanstack/vue-query'
import {
  api,
  type RequestOpts,
  type RumAppsResult,
  type RumAppInput,
  type MutationResult,
  type RumVitalsResult,
  type RumBreakdownResult,
  type RumPagesResult,
  type RumPageDetailResult,
  type RumErrorsResult,
  type RumErrorFacetsResult,
  type RumErrorDetailResult,
} from '@/lib/core/api'
import { toast } from '@/components/ui/toast'

export const rumVitalsKey = (
  app: MaybeRefOrGetter<string>,
  s: MaybeRefOrGetter<string>,
  e: MaybeRefOrGetter<string>,
): (string | undefined)[] => ['rum', 'vitals', toValue(app), String(toValue(s)), String(toValue(e))]

// The single source of truth for the RUM app-registry cache key — `useRumApps` reads it and the
// four mutations below invalidate it, so a successful create/update/rotate/delete always refetches
// the list.
export const rumAppsQueryKey = (): string[] => ['rum', 'apps']

export function useRumApps() {
  return useQuery({
    queryKey: rumAppsQueryKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RumAppsResult> => api.rumApps({ signal }),
    refetchInterval: 30_000,
  })
}

// Mutations never reject (the `api.rum*App` methods return the non-throwing `{ ok, error }`
// shape) — `onSuccess` always fires, so it's the single place to both invalidate the cache and
// toast, branching on the resolved result. Mirrors `usersQueries.ts`'s useCreateUser/useDeleteUser.
export function useCreateRumApp() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ name, input }: { name: string; input: RumAppInput }): Promise<MutationResult & { key?: string }> =>
      api.rumCreateApp(name, input),
    onSuccess: (res: MutationResult & { key?: string }) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't add app", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: rumAppsQueryKey() })
      toast({ variant: 'success', title: 'App added' })
    },
  })
}

export function useUpdateRumApp() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ name, input }: { name: string; input: RumAppInput }): Promise<MutationResult> =>
      api.rumUpdateApp(name, input),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't save app", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: rumAppsQueryKey() })
      toast({ variant: 'success', title: 'App updated' })
    },
  })
}

export function useRotateRumAppKey() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (name: string): Promise<MutationResult & { key?: string }> => api.rumRotateAppKey(name),
    onSuccess: (res: MutationResult & { key?: string }) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't rotate key", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: rumAppsQueryKey() })
      toast({ variant: 'success', title: 'Key rotated' })
    },
  })
}

export function useDeleteRumApp() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (name: string): Promise<MutationResult> => api.rumDeleteApp(name),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't remove app", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: rumAppsQueryKey() })
      toast({ variant: 'success', title: 'App removed' })
    },
  })
}

// --- Executive-summary fan-out (`/rum` overview) -------------------------------------------------
// One query per registered app, so the overview can aggregate vitals + errors + pages across the
// whole fleet. `useQueries` returns a reactive array of results index-aligned to `apps`; the array
// re-derives when the app list or window changes. Each fan-out keeps the previous window's data on
// screen (`keepPreviousData`) and polls at the same cadence as the single-app dashboards.
function fanOut<T>(
  apps: MaybeRefOrGetter<string[] | null | undefined>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  keyKind: string,
  fetchFn: (app: string, startNs: string, endNs: string, opts: RequestOpts) => Promise<T>,
  refetchInterval: number,
) {
  return useQueries({
    queries: computed(() =>
      (toValue(apps) ?? []).map((app: string) => ({
        queryKey: ['rum', keyKind, app, String(toValue(startNs)), String(toValue(endNs))],
        queryFn: ({ signal }: { signal: AbortSignal }): Promise<T> =>
          fetchFn(app, toValue(startNs), toValue(endNs), { signal }),
        placeholderData: keepPreviousData,
        refetchInterval,
      })),
    ),
  })
}

export function useRumAppsVitals(
  apps: MaybeRefOrGetter<string[] | null | undefined>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return fanOut<RumVitalsResult>(apps, startNs, endNs, 'vitals', api.rumVitals, 15_000)
}

export function useRumAppsErrors(
  apps: MaybeRefOrGetter<string[] | null | undefined>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return fanOut<RumErrorsResult>(apps, startNs, endNs, 'errors', api.rumErrors, 15_000)
}

export function useRumAppsPages(
  apps: MaybeRefOrGetter<string[] | null | undefined>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return fanOut<RumPagesResult>(apps, startNs, endNs, 'pages', api.rumPages, 30_000)
}

export function useRumVitals(
  app: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => rumVitalsKey(app, startNs, endNs)),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RumVitalsResult> =>
      api.rumVitals(toValue(app), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(app)),
    placeholderData: keepPreviousData,
    refetchInterval: 15_000,
  })
}

export function useRumBreakdown(
  app: MaybeRefOrGetter<string>,
  dimension: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => [
      'rum',
      'breakdown',
      toValue(app),
      toValue(dimension),
      String(toValue(startNs)),
      String(toValue(endNs)),
    ]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RumBreakdownResult> =>
      api.rumBreakdown(toValue(app), toValue(dimension), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(app)),
  })
}

export function useRumPages(
  app: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => ['rum', 'pages', toValue(app), String(toValue(startNs)), String(toValue(endNs))]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RumPagesResult> =>
      api.rumPages(toValue(app), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(app)),
  })
}

export function useRumPageDetail(
  app: MaybeRefOrGetter<string>,
  route: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => [
      'rum',
      'page',
      toValue(app),
      toValue(route),
      String(toValue(startNs)),
      String(toValue(endNs)),
    ]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RumPageDetailResult> =>
      api.rumPageDetail(toValue(app), toValue(route), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(app) && !!toValue(route)),
  })
}

export function useRumErrors(
  app: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  q: MaybeRefOrGetter<string> = () => '',
) {
  return useQuery({
    queryKey: computed(() => ['rum', 'errors', toValue(app), String(toValue(startNs)), String(toValue(endNs)), toValue(q)]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RumErrorsResult> =>
      api.rumErrors(toValue(app), toValue(startNs), toValue(endNs), { signal }, toValue(q)),
    enabled: computed(() => !!toValue(app)),
  })
}

export function useRumErrorFacets(
  app: MaybeRefOrGetter<string>,
  q: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => ['rum', 'error-facets', toValue(app), toValue(q), String(toValue(startNs)), String(toValue(endNs))]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RumErrorFacetsResult> =>
      api.rumErrorFacets(toValue(app), toValue(q), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(app)),
  })
}

export function useRumErrorDetail(
  app: MaybeRefOrGetter<string>,
  fingerprint: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => [
      'rum',
      'error-detail',
      toValue(app),
      toValue(fingerprint),
      String(toValue(startNs)),
      String(toValue(endNs)),
    ]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RumErrorDetailResult> =>
      api.rumErrorDetail(toValue(app), toValue(fingerprint), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(app) && !!toValue(fingerprint)),
  })
}

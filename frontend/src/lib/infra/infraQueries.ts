// Thin TanStack Query wrappers over the `api.infra*` methods (Infrastructure: host/GPU resource
// monitoring). Same contract as `rumQueries.ts`/`servicesQueries.ts`: reactive inputs (refs OR
// getter functions), normalized with `toValue`, feed a reactive `queryKey` for refetch-on-change +
// dedup, and a threaded AbortSignal for cancellation. `useInfraHosts`/`useInfraHostSeries` poll
// every 15s like the other live-tail dashboards (RUM vitals, services) and keep the previous
// window's data on screen while the range/host changes (`keepPreviousData`).
import { computed, toValue } from 'vue'
import type { MaybeRefOrGetter } from 'vue'
import { useQuery, keepPreviousData } from '@tanstack/vue-query'
import { api, type InfraHostsResult, type InfraHostDetail, type InfraSeriesResult } from '@/lib/core/api'

export const infraHostsKey = (
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
): string[] => ['infra', 'hosts', String(toValue(startNs)), String(toValue(endNs))]

export function useInfraHosts(
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => infraHostsKey(startNs, endNs)),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<InfraHostsResult> =>
      api.infraHosts(toValue(startNs), toValue(endNs), { signal }),
    placeholderData: keepPreviousData,
    refetchInterval: 15_000,
  })
}

export function useInfraHost(
  host: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => [
      'infra',
      'host',
      toValue(host),
      String(toValue(startNs)),
      String(toValue(endNs)),
    ]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<InfraHostDetail> =>
      api.infraHost(toValue(host), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(host)),
    placeholderData: keepPreviousData,
  })
}

export function useInfraHostSeries(
  host: MaybeRefOrGetter<string>,
  resource: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  // Extra gate on top of the host check below — e.g. the GPU panel passes `() => hasGpu` so the
  // (15s-polling) query never fires for hosts that don't report a GPU. Defaults to `true` so
  // existing call sites (cpu/memory/disk/network) are unaffected.
  enabled: MaybeRefOrGetter<boolean> = true,
) {
  return useQuery({
    queryKey: computed(() => [
      'infra',
      'series',
      toValue(host),
      toValue(resource),
      String(toValue(startNs)),
      String(toValue(endNs)),
    ]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<InfraSeriesResult> =>
      api.infraHostSeries(toValue(host), toValue(resource), toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(host) && !!toValue(enabled)),
    placeholderData: keepPreviousData,
    refetchInterval: 15_000,
  })
}

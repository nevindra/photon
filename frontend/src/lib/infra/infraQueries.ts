// Thin TanStack Query wrappers over the `api.infra*` methods (Infrastructure: host/GPU resource
// monitoring). Same contract as `rumQueries.ts`/`servicesQueries.ts`: reactive inputs (refs OR
// getter functions), normalized with `toValue`, feed a reactive `queryKey` for refetch-on-change +
// dedup, and a threaded AbortSignal for cancellation. `useInfraHosts`/`useInfraHostSeries` poll
// every 15s like the other live-tail dashboards (RUM vitals, services) and keep the previous
// window's data on screen while the range/host changes (`keepPreviousData`).
import { computed, toValue } from 'vue'
import type { MaybeRefOrGetter } from 'vue'
import { useQuery, keepPreviousData } from '@tanstack/vue-query'
import { api, type InfraHostsResult, type InfraHostDetail, type InfraSeriesResult, type InfraProcessesResult } from '@/lib/core/api'
import { tenantQueryTerm } from '@/lib/core/context'

export const infraHostsKey = (
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
): string[] => ['infra', 'hosts', String(toValue(startNs)), String(toValue(endNs))]

export function useInfraHosts(
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => [...infraHostsKey(startNs, endNs), tenantQueryTerm()]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<InfraHostsResult> =>
      api.infraHosts(toValue(startNs), toValue(endNs), tenantQueryTerm() ?? undefined, { signal }),
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
      tenantQueryTerm(),
    ]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<InfraHostDetail> =>
      api.infraHost(toValue(host), toValue(startNs), toValue(endNs), tenantQueryTerm() ?? undefined, { signal }),
    enabled: computed(() => !!toValue(host)),
    placeholderData: keepPreviousData,
  })
}

// Per-host supervised-process roster + resource usage for the Processes table. Polls every
// 15s like the other host queries and keeps the previous host/window's rows on screen across a
// change (`keepPreviousData`), gated on a non-empty host like `useInfraHost`.
export function useInfraHostProcesses(
  host: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => [
      'infra',
      'processes',
      toValue(host),
      String(toValue(startNs)),
      String(toValue(endNs)),
      tenantQueryTerm(),
    ]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<InfraProcessesResult> =>
      api.infraHostProcesses(toValue(host), toValue(startNs), toValue(endNs), tenantQueryTerm() ?? undefined, { signal }),
    enabled: computed(() => !!toValue(host)),
    placeholderData: keepPreviousData,
    refetchInterval: 15_000,
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
      tenantQueryTerm(),
    ]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<InfraSeriesResult> =>
      api.infraHostSeries(toValue(host), toValue(resource), toValue(startNs), toValue(endNs), tenantQueryTerm() ?? undefined, { signal }),
    enabled: computed(() => !!toValue(host) && !!toValue(enabled)),
    placeholderData: keepPreviousData,
    refetchInterval: 15_000,
  })
}

// The full per-resource series bundle for one host — the view creates it ONCE and passes it to
// both the glance tiles and the trend panels, so a tile and its section chart always read the
// same query cache entry. GPU-dependent resources are gated on `hasGpu` (no polling for hosts
// without a GPU), mirroring the existing gpu query's `enabled` gate.
export function useHostResourceSeries(
  host: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  hasGpu: MaybeRefOrGetter<boolean>,
) {
  return {
    cpu: useInfraHostSeries(host, 'cpu', startNs, endNs),
    memory: useInfraHostSeries(host, 'memory', startNs, endNs),
    disk: useInfraHostSeries(host, 'disk', startNs, endNs),
    network: useInfraHostSeries(host, 'network', startNs, endNs),
    load: useInfraHostSeries(host, 'load', startNs, endNs),
    gpu: useInfraHostSeries(host, 'gpu', startNs, endNs, hasGpu),
    gpuMemory: useInfraHostSeries(host, 'gpu_memory', startNs, endNs, hasGpu),
    gpuTemp: useInfraHostSeries(host, 'gpu_temp', startNs, endNs, hasGpu),
    gpuPower: useInfraHostSeries(host, 'gpu_power', startNs, endNs, hasGpu),
  }
}
export type HostResourceSeries = ReturnType<typeof useHostResourceSeries>

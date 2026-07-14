// Thin TanStack Query wrappers over the data/retention api.* methods. Mutations return the
// api's { ok, error } shape (no throw on 400), so callers read res.ok and surface res.error.
import { computed, toValue, type ComputedRef, type MaybeRefOrGetter } from 'vue'
import { useQuery, useMutation, useQueryClient, keepPreviousData, type UseQueryOptions } from '@tanstack/vue-query'
import {
  api,
  type StorageStats,
  type UsageSeries,
  type Retention,
  type SetRetentionResult,
  type PurgeRequest,
  type PurgeResult,
} from '@/lib/core/api'
import { windowMs } from '@/lib/core/context'
import { toast } from '@/components/ui/toast'

export const storageQueryKey = (): string[] => ['storage']
export const retentionQueryKey = (): string[] => ['retention']

// The four coarse buckets `/api/usage/series` accepts.
export type UsageWindow = '1h' | '24h' | '7d' | '30d'

// The usage-series bucket ('1h'|'24h'|'7d'|'30d') derived from the ONE global time control (the
// ContextBar's range, via context.windowMs) — so the /data usage charts + storage sparklines follow
// it instead of a second, page-local window selector. `/api/usage/series` only accepts these four
// coarse buckets, so we map the arbitrary global window onto the nearest one (sub-hour ranges clamp
// to '1h', the smallest bucket; anything past 7d falls to '30d', reachable only via a custom range).
export const usageWindow: ComputedRef<UsageWindow> = computed(() => {
  const w = windowMs.value
  if (w <= 3_600_000) return '1h'
  if (w <= 86_400_000) return '24h'
  if (w <= 604_800_000) return '7d'
  return '30d'
})

export function useStorage() {
  return useQuery({
    queryKey: storageQueryKey(),
    queryFn: ({ signal }): Promise<StorageStats> => api.getStorage({ signal }),
  })
}

export function useRetention() {
  return useQuery({
    queryKey: retentionQueryKey(),
    queryFn: ({ signal }): Promise<Retention> => api.getRetention({ signal }),
  })
}

// Neither mutation rejects (see the header comment) — `onSuccess` always fires, so it's the
// single place to both invalidate the cache and toast, branching on the resolved `{ ok, error }`
// shape.
export function useSetRetention() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (partial: Partial<Retention>): Promise<SetRetentionResult> => api.setRetention(partial),
    onSuccess: (res: SetRetentionResult) => {
      if (res && res.ok === false) {
        toast({
          variant: 'error',
          title: "Couldn't save retention settings",
          description: res.error ?? 'Please try again.',
        })
        return
      }
      qc.invalidateQueries({ queryKey: retentionQueryKey() })
      toast({ variant: 'success', title: 'Retention saved' })
    },
  })
}

export function usePurge() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: PurgeRequest): Promise<PurgeResult> => api.purgeData(body),
    onSuccess: (res: PurgeResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't purge data", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: storageQueryKey() })
      toast({ variant: 'success', title: 'Data purged' })
    },
  })
}

export function useUsageSeries(
  window: MaybeRefOrGetter<string>,
  options: Partial<UseQueryOptions<UsageSeries>> = {},
) {
  return useQuery({
    queryKey: computed(() => ['usage-series', toValue(window)]),
    queryFn: ({ signal }): Promise<UsageSeries> => api.getUsageSeries({ window: toValue(window) }, { signal }),
    placeholderData: keepPreviousData,
    staleTime: 30_000,
    refetchInterval: 30_000,
    ...options,
  })
}

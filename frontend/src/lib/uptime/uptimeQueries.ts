// Thin TanStack Query wrappers over the `api.*` uptime-monitor methods. Same contract as
// `logsQueries.js`/`tracesQueries.js`: reactive inputs (refs OR getter functions, normalized
// with `toValue`), a reactive `queryKey` for refetch-on-change + dedup, and a threaded
// AbortSignal for cancellation. No business logic here — views compose these and read
// `.data`/`.isFetching`.
import { computed, toValue, type MaybeRefOrGetter } from 'vue'
import { useQuery, useMutation, useQueryClient, type QueryClient } from '@tanstack/vue-query'
import { api, type ApiError, type Monitor, type MonitorInput, type HeartbeatsResult } from '@/lib/core/api'
import { toast } from '@/components/ui/toast'

export const monitorsQueryKey = (): string[] => ['monitors']
export const monitorQueryKey = (id: MaybeRefOrGetter<string>) => ['monitor', toValue(id)]
export const heartbeatsQueryKey = (id: MaybeRefOrGetter<string>, window: MaybeRefOrGetter<string>) => [
  'heartbeats',
  toValue(id),
  toValue(window),
]
export const incidentsQueryKey = (id: MaybeRefOrGetter<string>) => ['incidents', toValue(id)]

// All monitors — polled for live status.
export function useMonitors() {
  return useQuery({
    queryKey: monitorsQueryKey(),
    queryFn: ({ signal }): Promise<Monitor[]> => api.listMonitors({ signal }),
    refetchInterval: 15_000,
  })
}

// A single monitor.
export function useMonitor(id: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => monitorQueryKey(id)),
    queryFn: ({ signal }): Promise<Monitor> => api.getMonitor(toValue(id), { signal }),
    enabled: computed(() => toValue(id) != null),
    refetchInterval: 15_000,
  })
}

// Heartbeat history + uptime % for a monitor over a window.
export function useHeartbeats(id: MaybeRefOrGetter<string>, window: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => heartbeatsQueryKey(id, window)),
    queryFn: ({ signal }): Promise<HeartbeatsResult> => api.getHeartbeats(toValue(id), toValue(window), { signal }),
    enabled: computed(() => toValue(id) != null),
    refetchInterval: 15_000,
  })
}

// Incident history for a monitor.
export function useIncidents(id: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => incidentsQueryKey(id)),
    queryFn: ({ signal }): Promise<unknown[]> => api.getIncidents(toValue(id), { signal }),
    enabled: computed(() => toValue(id) != null),
  })
}

function invalidateMonitors(qc: QueryClient) {
  return qc.invalidateQueries({ queryKey: monitorsQueryKey() })
}

// Unlike the user/data mutations below, `api.createMonitor`/`updateMonitor`/`deleteMonitor`/
// `pauseMonitor`/`resumeMonitor` have no internal try/catch — a non-2xx response throws a real
// ky `HTTPError` (see lib/api.js's `beforeError` hook, which attaches `.body.error` from the
// server's JSON error body) that rejects the mutation and lands in `onError` below. Fall back to
// a generic sentence for network failures or bodies without a parsed error message.
function mutationErrorMessage(err: unknown, fallback: string): string {
  return (err as ApiError)?.body?.error ?? fallback
}

export function useCreateMonitor() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: MonitorInput): Promise<Monitor> => api.createMonitor(body),
    onSuccess: () => {
      invalidateMonitors(qc)
      toast({ variant: 'success', title: 'Monitor created' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't create monitor",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

export function useUpdateMonitor() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: MonitorInput }): Promise<Monitor> => api.updateMonitor(id, body),
    onSuccess: (_data, { id }) => {
      qc.invalidateQueries({ queryKey: monitorsQueryKey() })
      qc.invalidateQueries({ queryKey: monitorQueryKey(id) })
      toast({ variant: 'success', title: 'Monitor updated' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't update monitor",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

export function useDeleteMonitor() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string): Promise<boolean> => api.deleteMonitor(id),
    onSuccess: () => {
      invalidateMonitors(qc)
      toast({ variant: 'success', title: 'Monitor deleted' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't delete monitor",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

export function usePauseMonitor() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string): Promise<Monitor> => api.pauseMonitor(id),
    onSuccess: () => {
      invalidateMonitors(qc)
      toast({ variant: 'success', title: 'Monitor paused' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't pause monitor",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

export function useResumeMonitor() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string): Promise<Monitor> => api.resumeMonitor(id),
    onSuccess: () => {
      invalidateMonitors(qc)
      toast({ variant: 'success', title: 'Monitor resumed' })
    },
    onError: (err) => {
      toast({
        variant: 'error',
        title: "Couldn't resume monitor",
        description: mutationErrorMessage(err, 'Please try again.'),
      })
    },
  })
}

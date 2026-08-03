// Thin TanStack Query wrappers over the `api.*tenant*`/`api.federationStatus` methods. Same
// contract as `rumQueries.ts`: mutations never reject (the `api.*` methods return the
// non-throwing `{ ok, error }` shape) — `onSuccess` always fires, so it's the single place to
// both invalidate the cache and toast, branching on the resolved result. `useTenantsSummary`
// polls every 15s (the Home tenant board is a live dashboard); `useFederationStatus` polls every
// 30s (matches this photon's own push interval order of magnitude).
import { useQuery, useMutation, useQueryClient } from '@tanstack/vue-query'
import { api, type TenantsResult, type TenantSummary, type MutationResult, type FederationStatusResult } from '@/lib/core/api'
import { toast } from '@/components/ui/toast'

export type { TenantSummary }

export const tenantsQueryKey = (): string[] => ['tenants']
export const tenantsSummaryQueryKey = (): string[] => ['tenants', 'summary']
export const federationStatusQueryKey = (): string[] => ['federation', 'status']

export function useTenants() {
  return useQuery({
    queryKey: tenantsQueryKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<TenantsResult> => api.tenants({ signal }),
  })
}

export function useTenantsSummary() {
  return useQuery({
    queryKey: tenantsSummaryQueryKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<TenantSummary[]> => api.tenantsSummary({ signal }),
    refetchInterval: 15_000,
  })
}

export function useFederationStatus() {
  return useQuery({
    queryKey: federationStatusQueryKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<FederationStatusResult> => api.federationStatus({ signal }),
    refetchInterval: 30_000,
  })
}

export function useCreateTenant() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ name, uiUrl }: { name: string; uiUrl?: string | null }): Promise<MutationResult & { token?: string }> =>
      api.createTenant(name, uiUrl),
    onSuccess: (res: MutationResult & { token?: string }) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't add tenant", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: tenantsQueryKey() })
      toast({ variant: 'success', title: 'Tenant added' })
    },
  })
}

export function useUpdateTenant() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ name, uiUrl }: { name: string; uiUrl: string | null }): Promise<MutationResult> => api.updateTenant(name, uiUrl),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't save tenant", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: tenantsQueryKey() })
      toast({ variant: 'success', title: 'Tenant updated' })
    },
  })
}

export function useRotateTenantToken() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (name: string): Promise<MutationResult & { token?: string }> => api.rotateTenantToken(name),
    onSuccess: (res: MutationResult & { token?: string }) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't rotate token", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: tenantsQueryKey() })
      toast({ variant: 'success', title: 'Token rotated' })
    },
  })
}

export function useDeleteTenant() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (name: string): Promise<MutationResult> => api.deleteTenant(name),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't remove tenant", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: tenantsQueryKey() })
      toast({ variant: 'success', title: 'Tenant removed' })
    },
  })
}

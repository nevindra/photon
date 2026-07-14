// Thin TanStack Query wrappers over the `api.*` user-management methods — same contract as
// `uptimeQueries.js`. Mutations return the api's `{ ok, error }` shape (they do not throw on
// 400/409), so callers read `res.ok` and surface `res.error`.
import { useQuery, useMutation, useQueryClient } from '@tanstack/vue-query'
import { api, type UsersResult, type MutationResult } from '@/lib/core/api'
import { toast } from '@/components/ui/toast'

export const usersQueryKey = (): string[] => ['users']

export function useUsers() {
  return useQuery({
    queryKey: usersQueryKey(),
    queryFn: ({ signal }): Promise<UsersResult> => api.listUsers({ signal }),
  })
}

// Never rejects (see the header comment) — `onSuccess` always fires, so it's the single place
// to both invalidate the cache and toast, branching on the resolved `{ ok, error }` shape.
export function useCreateUser() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ username, password }: { username: string; password: string }): Promise<MutationResult> =>
      api.createUser(username, password),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't add user", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: usersQueryKey() })
      toast({ variant: 'success', title: 'User added' })
    },
  })
}

export function useDeleteUser() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (username: string): Promise<MutationResult> => api.deleteUser(username),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't remove user", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: usersQueryKey() })
      toast({ variant: 'success', title: 'User removed' })
    },
  })
}

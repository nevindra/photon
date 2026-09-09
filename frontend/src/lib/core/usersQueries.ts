// Thin TanStack Query wrappers over the `api.*` user-management methods — same contract as
// `uptimeQueries.js`. Mutations return the api's `{ ok, error }` shape (they do not throw on
// 400/409), so callers read `res.ok` and surface `res.error`.
import { useQuery, useMutation, useQueryClient } from '@tanstack/vue-query'
import { api, type UsersResult, type MutationResult, type Role } from '@/lib/core/api'
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
    mutationFn: ({ username, password, role }: { username: string; password: string; role?: Role }): Promise<MutationResult> =>
      api.createUser(username, password, role ?? 'viewer'),
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

export function useUpdateUserRole() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ username, role }: { username: string; role: Role }): Promise<MutationResult> =>
      api.updateUserRole(username, role),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't change role", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: usersQueryKey() })
      toast({ variant: 'success', title: 'Role updated' })
    },
  })
}

export function useChangePassword() {
  return useMutation({
    mutationFn: ({ username, password, currentPassword }: { username: string; password: string; currentPassword?: string }): Promise<MutationResult> =>
      api.changePassword(username, password, currentPassword),
    onSuccess: (res: MutationResult, vars) => {
      // No cache to invalidate: the list does not show passwords. The refusal is the dialog's to
      // place under its field, so only the success toasts here.
      if (res && res.ok === false) return
      toast({ variant: 'success', title: 'Password changed', description: `${vars.username} signs in with the new one from now on.` })
    },
  })
}

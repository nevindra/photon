<script setup lang="ts">
// The people who can sign in, one row each: who, what they may change, when they were added, and
// a Remove that explains itself when it refuses. An admin changes a role in place — the picker
// sits where the badge would be, so nothing moves when permission arrives. Your own row shows
// the badge even for an admin: the server refuses to demote the last admin and to delete
// yourself, and a control that would only ever toast an error is worse than none. A key beside
// the bin sets a password: your own on your row whatever your role, anyone's for an admin.
import { computed } from 'vue'
import { Trash2, UserRound, KeyRound } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { SelectMenu } from '@/components/ui/select-menu'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { EmptyState } from '@/components/ui/empty-state'
import UserAvatar from './UserAvatar.vue'
import RoleBadge from './RoleBadge.vue'
import type { UserInfo } from '@/lib/core/api'
import { ROLES, ROLE_META, type Role } from '@/lib/deploy/roles'
import { roleOf, addedLabel, removeBlockedReason, passwordBlockedReason } from '@/lib/core/users'

const props = defineProps<{
  users: UserInfo[]
  /** The unfiltered count — the anti-lockout rule is about everyone, not the rows that match a search. */
  total: number
  loading: boolean
  me: string | null
  /** May the signed-in user add and remove. */
  canManage: boolean
  /** Does this server speak roles (and may the signed-in user assign them). */
  canSetRoles: boolean
  rolesSupported: boolean
  /** Filtered to nothing — the empty state says so instead of looking like an empty install. */
  filtered: boolean
  /** The username to flash after it was just added, so the eye lands on the new row. */
  highlight?: string | null
}>()
const emit = defineEmits<{ remove: [user: UserInfo]; 'change-role': [user: UserInfo, role: Role]; 'change-password': [user: UserInfo] }>()

const roleOptions = ROLES.map((r) => ({ value: r, label: ROLE_META[r].label }))

const removeReason = (u: UserInfo) => removeBlockedReason(u, { me: props.me, total: props.total, allowed: props.canManage })
const passwordReason = (u: UserInfo) => passwordBlockedReason(u, { me: props.me, allowed: props.canManage })
const fullDate = (ms: number) => (ms ? new Date(ms).toLocaleString() : 'Created during onboarding')
const columns = computed(() => (props.rolesSupported ? 4 : 3))
</script>

<template>
  <Table containerClass="overflow-visible" data-testid="users-table">
    <TableHeader>
      <TableRow class="hover:bg-transparent">
        <TableHead>User</TableHead>
        <TableHead v-if="rolesSupported" class="w-44">Role</TableHead>
        <TableHead class="w-36">Added</TableHead>
        <TableHead class="w-20"><span class="sr-only">Actions</span></TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <template v-if="loading">
        <TableRow v-for="i in 3" :key="i" class="hover:bg-transparent">
          <TableCell :colspan="columns" class="py-3">
            <div class="flex items-center gap-3">
              <Skeleton class="size-8 rounded-full" />
              <Skeleton class="h-4 w-32" />
              <Skeleton class="ml-auto h-6 w-20" />
            </div>
          </TableCell>
        </TableRow>
      </template>

      <TableRow v-else-if="users.length === 0" class="hover:bg-transparent">
        <TableCell :colspan="columns" class="p-0">
          <EmptyState
            :title="filtered ? 'No one matches that search' : 'No users yet'"
            :description="filtered ? 'Try a shorter name, or clear the role filter.' : 'Add the first person with the button above.'"
            class="py-12"
            data-testid="users-empty"
          >
            <template #icon><UserRound /></template>
          </EmptyState>
        </TableCell>
      </TableRow>

      <TableRow
        v-for="u in users"
        v-else
        :key="u.username"
        data-testid="user-row"
        :data-username="u.username"
        :data-state="u.username === highlight ? 'selected' : undefined"
        class="transition-colors duration-700"
      >
        <TableCell class="py-2.5">
          <div class="flex min-w-0 items-center gap-3">
            <UserAvatar :username="u.username" :me="u.username === me" />
            <div class="flex min-w-0 items-center gap-2">
              <span class="truncate text-sm font-medium text-foreground">{{ u.username }}</span>
              <Badge v-if="u.username === me" variant="brand" data-testid="user-you">You</Badge>
            </div>
          </div>
        </TableCell>

        <TableCell v-if="rolesSupported">
          <SelectMenu
            v-if="canSetRoles && u.username !== me"
            :model-value="roleOf(u)"
            :options="roleOptions"
            content-class="w-36"
            :aria-label="`Role for ${u.username}`"
            :data-testid="`role-${u.username}`"
            @update:model-value="(r: unknown) => r !== roleOf(u) && emit('change-role', u, r as Role)"
          />
          <RoleBadge v-else :role="roleOf(u)" :data-testid="`role-${u.username}`" />
        </TableCell>

        <TableCell>
          <span class="text-xs text-muted-foreground" :title="fullDate(u.created_at)">{{ addedLabel(u.created_at) }}</span>
        </TableCell>

        <TableCell class="pr-2 text-right">
          <div class="inline-flex items-center gap-0.5">
          <Tooltip v-if="passwordReason(u)">
            <TooltipTrigger as-child>
              <span class="inline-flex" tabindex="0">
                <Button variant="ghost" size="icon" class="size-8 text-muted-foreground" disabled aria-disabled="true" :aria-label="`Change password for ${u.username}`" :data-testid="`password-${u.username}`">
                  <KeyRound class="size-4" />
                </Button>
              </span>
            </TooltipTrigger>
            <TooltipContent side="left">{{ passwordReason(u) }}</TooltipContent>
          </Tooltip>
          <Tooltip v-else>
            <TooltipTrigger as-child>
              <Button
                variant="ghost"
                size="icon"
                class="size-8 text-muted-foreground"
                :aria-label="`Change password for ${u.username}`"
                :data-testid="`password-${u.username}`"
                @click="emit('change-password', u)"
              >
                <KeyRound class="size-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="left">{{ u.username === me ? 'Change your password' : 'Reset password' }}</TooltipContent>
          </Tooltip>
          <Tooltip v-if="removeReason(u)">
            <TooltipTrigger as-child>
              <!-- A disabled button swallows pointer events; the tooltip anchors on a wrapper. -->
              <span class="inline-flex" tabindex="0">
                <Button variant="ghost" size="icon" class="size-8 text-muted-foreground" disabled aria-disabled="true" :aria-label="`Remove ${u.username}`" :data-testid="`remove-${u.username}`">
                  <Trash2 class="size-4" />
                </Button>
              </span>
            </TooltipTrigger>
            <TooltipContent side="left">{{ removeReason(u) }}</TooltipContent>
          </Tooltip>
          <Button
            v-else
            variant="ghost"
            size="icon"
            class="size-8 text-muted-foreground hover:bg-sev-error-soft hover:text-sev-error"
            :aria-label="`Remove ${u.username}`"
            :data-testid="`remove-${u.username}`"
            @click="emit('remove', u)"
          >
            <Trash2 class="size-4" />
          </Button>
          </div>
        </TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>

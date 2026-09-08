<script setup lang="ts">
// `/settings/users`: who can sign in to this Photon and what each of them may change. It replaced
// a Settings modal that held the list and the add form in one scrolling box — a page has room to
// say what a role means, to explain a refused Remove, and to keep the form where the eye can
// return to it after the toast. Control-plane, not telemetry: no time picker.
//
// Layout: the table takes the width; adding a person is a modal off the header button, and the
// two password shapes (your own with the current one, a reset for someone else) are a second
// modal off the row's key. The search and role filter narrow the table only; the anti-lockout
// rules (`removeBlockedReason`) look at everyone. The table is paged client-side — the whole
// list is one small request, and paging keeps a long roster from being a long scroll.
import { ref, computed, watch } from 'vue'
import { Search, X, TriangleAlert, UserRoundPlus } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import UsersTable from '@/components/settings/UsersTable.vue'
import AddUserDialog from '@/components/settings/AddUserDialog.vue'
import ChangePasswordDialog from '@/components/settings/ChangePasswordDialog.vue'
import RolesCard from '@/components/settings/RolesCard.vue'
import RemoveUserDialog from '@/components/settings/RemoveUserDialog.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Pagination } from '@/components/ui/pagination'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { api, type UserInfo } from '@/lib/core/api'
import { username as me, can, rolesSupported } from '@/lib/core/auth'
import { ROLES, ROLE_META, type Role } from '@/lib/deploy/roles'
import { countRoles, filterUsers, paginate, pageOf, PAGE_SIZES, DEFAULT_PAGE_SIZE } from '@/lib/core/users'
import { useUsers, useCreateUser, useDeleteUser, useUpdateUserRole, useChangePassword } from '@/lib/core/usersQueries'

const { data, isLoading } = useUsers()
const createUser = useCreateUser()
const deleteUser = useDeleteUser()
const updateRole = useUpdateUserRole()
const changePassword = useChangePassword()

const users = computed(() => data.value?.users ?? [])
const counts = computed(() => countRoles(users.value))
// Adding and removing need the Users permission. Assigning a role additionally needs a server that
// speaks roles — on one that does not, every user is the single implicit role and a picker would
// toast success over a request the server drops.
const canManage = computed(() => can('users'))
const canSetRoles = computed(() => rolesSupported.value && canManage.value)

const q = ref('')
const roleFilter = ref<Role | null>(null)
const visible = computed(() => filterUsers(users.value, q.value, roleFilter.value))
const filtered = computed(() => q.value.trim() !== '' || roleFilter.value !== null)
function onRoleFilter(v: unknown): void {
  roleFilter.value = typeof v === 'string' && (ROLES as string[]).includes(v) ? (v as Role) : null
}

// --- Paging ---
const page = ref(1)
const pageSize = ref<number>(DEFAULT_PAGE_SIZE)
const paged = computed(() => paginate(visible.value, page.value, pageSize.value))
// A narrower list starts from its first page; `paginate` clamps the rest (a removal that empties the last page).
watch([q, roleFilter, pageSize], () => (page.value = 1))
// Mounted whenever the roster could need paging — sized to everyone, not the current match, so
// the strip does not jump in and out as a search is typed.
const showPager = computed(() => !isLoading.value && (paged.value.pages > 1 || users.value.length > PAGE_SIZES[0]))

// One admin is a lockout waiting to happen: the server refuses to demote or remove them, so the
// day that account is lost, nobody can add another. Said once, above the table, only when true.
const soleAdmin = computed(() => rolesSupported.value && users.value.length > 1 && counts.value.admin === 1)

// --- Add ---
const addOpen = ref(false)
const addError = ref<string | null>(null)
const highlight = ref<string | null>(null)
let highlightTimer: ReturnType<typeof setTimeout> | null = null
async function onAdd(input: { username: string; password: string; role: Role }): Promise<void> {
  addError.value = null
  const res = await createUser.mutateAsync(input)
  if (res && res.ok === false) {
    addError.value = res.error ?? 'Could not add user.'
    return
  }
  addOpen.value = false
  q.value = ''
  roleFilter.value = null
  highlight.value = input.username
  if (highlightTimer) clearTimeout(highlightTimer)
  highlightTimer = setTimeout(() => (highlight.value = null), 2500)
  // The new row lands wherever the sort puts it; turn to that page so the flash is seen. The
  // refetch may have landed before or after this line, so try now and again when the list moves.
  pendingJump.value = input.username
  jumpToPending()
}
const pendingJump = ref<string | null>(null)
function jumpToPending(): void {
  if (!pendingJump.value) return
  const p = pageOf(visible.value, (u) => u.username === pendingJump.value, pageSize.value)
  if (p === null) return
  page.value = p
  pendingJump.value = null
}
watch(visible, jumpToPending)

// --- Password ---
const passwordFor = ref<UserInfo | null>(null)
const passwordError = ref<string | null>(null)
const passwordOpen = computed({ get: () => passwordFor.value !== null, set: (o: boolean) => (o ? undefined : (passwordFor.value = null)) })
async function onChangePassword(input: { password: string; currentPassword?: string }): Promise<void> {
  const u = passwordFor.value
  if (!u) return
  passwordError.value = null
  const res = await changePassword.mutateAsync({ username: u.username, ...input })
  if (res && res.ok === false) {
    passwordError.value = res.error ?? 'Could not change the password.'
    return
  }
  passwordFor.value = null
}

// --- Role ---
function onChangeRole(u: UserInfo, role: Role): void {
  updateRole.mutate({ username: u.username, role })
}

// --- Remove ---
const removing = ref<UserInfo | null>(null)
const removeOpen = computed({ get: () => removing.value !== null, set: (o: boolean) => (o ? undefined : (removing.value = null)) })
async function onConfirmRemove(): Promise<void> {
  const u = removing.value
  if (!u) return
  await deleteUser.mutateAsync(u.username)
  // The mutation toasts a refusal itself; the dialog closes either way so a stale name is not left on screen.
  removing.value = null
}
watch(users, (list) => {
  // Someone else removed the user a dialog is about — close rather than act on a ghost.
  if (removing.value && !list.some((u) => u.username === removing.value?.username)) removing.value = null
  if (passwordFor.value && !list.some((u) => u.username === passwordFor.value?.username)) passwordFor.value = null
})
</script>

<template>
  <AppShell :mock="api.mock" crumb="Users" :time-picker="false">
    <section class="flex min-h-0 flex-1 flex-col overflow-y-auto px-5 pb-8 pt-5" data-testid="users-view">
      <header class="mb-5 flex flex-wrap items-end justify-between gap-4">
        <div class="max-w-2xl">
          <h1 class="text-xl font-semibold text-foreground">Users</h1>
          <p class="mt-1 text-sm text-muted-foreground">
            Who can sign in to this Photon, and what each of them may change. Sign-ins are per person — share an account and the audit log can no longer say who did what.
          </p>
        </div>
        <div class="flex flex-wrap items-center gap-5">
          <dl class="flex items-center gap-4 text-xs text-muted-foreground" data-testid="users-summary">
            <div class="flex items-baseline gap-1.5">
              <dd class="text-lg font-semibold tabular-nums text-foreground">{{ users.length }}</dd>
              <dt>{{ users.length === 1 ? 'user' : 'users' }}</dt>
            </div>
            <template v-if="rolesSupported">
              <div v-for="r in [...ROLES].reverse()" :key="r" class="flex items-baseline gap-1.5">
                <dd class="text-lg font-semibold tabular-nums text-foreground">{{ counts[r] }}</dd>
                <dt>{{ ROLE_META[r].label.toLowerCase() }}{{ counts[r] === 1 ? '' : 's' }}</dt>
              </div>
            </template>
          </dl>
          <!-- Disabled, not hidden, for a non-admin: the hover says who to ask. -->
          <GatedButton perm="users" data-testid="add-user-open" @click="addOpen = true">
            <UserRoundPlus />
            Add a user
          </GatedButton>
        </div>
      </header>

      <div class="grid gap-5" :class="rolesSupported && !canManage ? 'lg:grid-cols-[minmax(0,1fr)_22rem] lg:items-start' : ''">
        <div class="flex min-w-0 flex-col gap-3">
          <div class="flex flex-wrap items-center gap-2" data-testid="users-filters">
            <div class="relative min-w-[14rem] flex-1">
              <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input v-model="q" placeholder="Find a user…" class="h-8 pl-8 pr-8 text-xs" data-testid="users-search" aria-label="Find a user" />
              <button
                v-if="q"
                type="button"
                class="absolute right-1 top-1/2 flex size-6 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
                aria-label="Clear search"
                @click="q = ''"
              >
                <X class="size-3.5" />
              </button>
            </div>
            <Segmented v-if="rolesSupported" :model-value="roleFilter ?? ''" class="h-8" aria-label="Filter by role" data-testid="users-role-filter" @update:model-value="onRoleFilter">
              <SegmentedItem value="" class="px-2.5 text-xs">All</SegmentedItem>
              <SegmentedItem v-for="r in [...ROLES].reverse()" :key="r" :value="r" class="px-2.5 text-xs">{{ ROLE_META[r].label }}</SegmentedItem>
            </Segmented>
          </div>

          <div
            v-if="soleAdmin"
            class="flex items-start gap-2 rounded-lg border border-sev-warn/25 bg-sev-warn-soft px-3 py-2 text-xs text-foreground"
            data-testid="sole-admin-note"
          >
            <TriangleAlert class="mt-0.5 size-3.5 shrink-0 text-sev-warn" />
            <span>There is only one admin. Photon will refuse to demote or remove them, so if that account is lost nobody can add another — make a second admin before you need one.</span>
          </div>

          <Card class="overflow-hidden">
            <UsersTable
              :users="paged.rows"
              :total="users.length"
              :loading="isLoading"
              :me="me"
              :can-manage="canManage"
              :can-set-roles="canSetRoles"
              :roles-supported="rolesSupported"
              :filtered="filtered"
              :highlight="highlight"
              @remove="removing = $event"
              @change-role="onChangeRole"
              @change-password="passwordFor = $event"
            />
            <Pagination
              v-if="showPager"
              class="border-t border-border px-3 py-2"
              :page="paged.page"
              :pages="paged.pages"
              :from="paged.from"
              :to="paged.to"
              :total="paged.total"
              :page-size="pageSize"
              :page-sizes="PAGE_SIZES"
              :noun="filtered ? 'matching' : 'users'"
              @update:page="page = $event"
              @update:page-size="pageSize = $event"
            />
          </Card>
          <p v-if="filtered && !isLoading" class="text-xs text-muted-foreground" data-testid="users-filter-count">
            Showing {{ visible.length }} of {{ users.length }}
          </p>
        </div>

        <aside v-if="rolesSupported && !canManage" class="flex flex-col gap-5">
          <RolesCard />
        </aside>
      </div>
    </section>

    <AddUserDialog
      v-model:open="addOpen"
      :roles-supported="rolesSupported"
      :busy="createUser.isPending.value"
      :server-error="addError"
      @submit="onAdd"
      @clear-error="addError = null"
    />

    <ChangePasswordDialog
      v-model:open="passwordOpen"
      :user="passwordFor"
      :self="passwordFor?.username === me"
      :pending="changePassword.isPending.value"
      :server-error="passwordError"
      @submit="onChangePassword"
      @clear-error="passwordError = null"
    />

    <RemoveUserDialog
      v-model:open="removeOpen"
      :user="removing"
      :roles-supported="rolesSupported"
      :pending="deleteUser.isPending.value"
      @confirm="onConfirmRemove"
    />
  </AppShell>
</template>

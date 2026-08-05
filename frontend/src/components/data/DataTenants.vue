<script setup lang="ts">
// Body of the /tenants page (Manage group): a table of registered federation tenants (redacted
// token, UI URL, created date) with per-row edit/delete actions, plus an "Add tenant" button that
// opens the add/edit dialog (TenantManageDialog) in add mode.
import { ref, computed } from 'vue'
import { Plus, Pencil, Trash2 } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { EmptyState } from '@/components/ui/empty-state'
import { formatFull } from '@/lib/core/format'
import type { Tenant } from '@/lib/core/api'
import { useTenants, useDeleteTenant } from '@/lib/tenants/tenantsQueries'
import TenantManageDialog from '@/components/tenants/TenantManageDialog.vue'

const { data } = useTenants()
const tenants = computed(() => data.value?.tenants ?? [])
const deleteMut = useDeleteTenant()

const dialogOpen = ref(false)
const editing = ref<Tenant | null>(null)
const deleting = ref<Tenant | null>(null)

function openAdd() {
  editing.value = null
  dialogOpen.value = true
}
function openEdit(tenant: Tenant) {
  editing.value = tenant
  dialogOpen.value = true
}
function confirmDelete() {
  if (!deleting.value) return
  deleteMut.mutate(deleting.value.name)
  deleting.value = null
}

const createdAt = (ms: number): string => formatFull(BigInt(ms) * 1_000_000n)
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex items-center justify-end">
      <Button data-testid="tenants-add-trigger" size="sm" @click="openAdd">
        <Plus class="size-4" /> Add tenant
      </Button>
    </div>

    <EmptyState v-if="!tenants.length" title="No tenants registered" description="Register your first tenant to receive federated telemetry." class="h-auto">
      <button
        type="button"
        class="mt-2 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90"
        @click="openAdd"
      >
        Register your first tenant
      </button>
    </EmptyState>

    <div v-else data-testid="tenants-table" class="overflow-x-auto rounded-lg border border-border">
      <table class="w-full border-collapse text-sm">
        <thead>
          <tr class="border-b border-border text-[10px] uppercase tracking-wide text-muted-foreground">
            <th class="px-3 py-2 text-left font-semibold">Name</th>
            <th class="px-3 py-2 text-left font-semibold">Token</th>
            <th class="px-3 py-2 text-left font-semibold">UI URL</th>
            <th class="px-3 py-2 text-left font-semibold">Created</th>
            <th class="px-3 py-2 text-right font-semibold">Actions</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="tenant in tenants" :key="tenant.name" data-testid="tenant-row" class="border-b border-border/50 last:border-0">
            <td class="px-3 py-2.5 font-medium text-foreground">{{ tenant.name }}</td>
            <td class="px-3 py-2.5 font-mono text-xs text-muted-foreground">{{ tenant.token }}</td>
            <td class="px-3 py-2.5 text-muted-foreground">{{ tenant.ui_url ?? '—' }}</td>
            <td class="px-3 py-2.5 text-muted-foreground">{{ createdAt(tenant.created_at) }}</td>
            <td class="px-3 py-2.5">
              <div class="flex justify-end gap-1">
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-7 text-muted-foreground hover:text-foreground"
                  :data-testid="`tenant-edit-${tenant.name}`"
                  aria-label="Edit tenant"
                  title="Edit tenant"
                  @click="openEdit(tenant)"
                >
                  <Pencil class="size-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-7 text-muted-foreground hover:text-sev-error"
                  :data-testid="`tenant-delete-${tenant.name}`"
                  aria-label="Delete tenant"
                  title="Delete tenant"
                  @click="deleting = tenant"
                >
                  <Trash2 class="size-4" />
                </Button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <TenantManageDialog v-model:open="dialogOpen" :tenant="editing" />

    <!-- Delete confirmation — revoking a tenant kills its push token immediately. -->
    <Dialog :open="deleting !== null" @update:open="(v: boolean) => { if (!v) deleting = null }">
      <DialogContent class="max-w-md">
        <DialogHeader>
          <DialogTitle>Delete {{ deleting?.name }}?</DialogTitle>
          <DialogDescription>
            This revokes the tenant's push token immediately — its pushes will start failing with 401.
            Telemetry already stored on this install is kept until retention removes it.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" data-testid="tenant-delete-cancel" @click="deleting = null">Cancel</Button>
          <Button variant="destructive" data-testid="tenant-delete-confirm" @click="confirmDelete">Delete tenant</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>

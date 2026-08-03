<script setup lang="ts">
// The Tenants tab of the /data page: a table of registered federation tenants (redacted token, UI
// URL, created date) + the manage dialog trigger — the /data-side twin of RumAppsView's "Manage
// apps" button + RumManageAppsDialog pairing, just for the tenant registry instead of RUM apps.
import { ref, computed } from 'vue'
import { EmptyState } from '@/components/ui/empty-state'
import { formatFull } from '@/lib/core/format'
import { useTenants } from '@/lib/tenants/tenantsQueries'
import TenantManageDialog from '@/components/tenants/TenantManageDialog.vue'

const { data } = useTenants()
const tenants = computed(() => data.value?.tenants ?? [])
const manageOpen = ref(false)

const createdAt = (ms: number): string => formatFull(BigInt(ms) * 1_000_000n)
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex items-center justify-end">
      <button
        type="button"
        data-testid="tenants-manage-trigger"
        class="rounded-md border border-border px-2.5 py-1 text-xs font-medium text-muted-foreground hover:bg-accent hover:text-foreground"
        @click="manageOpen = true"
      >
        Manage tenants
      </button>
    </div>

    <EmptyState v-if="!tenants.length" title="No tenants registered" description="Register your first tenant to receive federated telemetry." class="h-auto">
      <button
        type="button"
        class="mt-2 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90"
        @click="manageOpen = true"
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
          </tr>
        </thead>
        <tbody>
          <tr v-for="tenant in tenants" :key="tenant.name" data-testid="tenant-row" class="border-b border-border/50 last:border-0">
            <td class="px-3 py-2.5 font-medium text-foreground">{{ tenant.name }}</td>
            <td class="px-3 py-2.5 font-mono text-xs text-muted-foreground">{{ tenant.token }}</td>
            <td class="px-3 py-2.5 text-muted-foreground">{{ tenant.ui_url ?? '—' }}</td>
            <td class="px-3 py-2.5 text-muted-foreground">{{ createdAt(tenant.created_at) }}</td>
          </tr>
        </tbody>
      </table>
    </div>

    <TenantManageDialog v-model:open="manageOpen" :tenants="tenants" />
  </div>
</template>

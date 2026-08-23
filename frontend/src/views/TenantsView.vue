<script setup lang="ts">
// `/tenants` (Manage group): the federation tenant registry, promoted out of the /data tabs into
// its own page. Two layers, in the order somebody actually needs them: the live fleet-health board
// on top (one card per Site — liveness, ingest rate, open incidents), then the registry table +
// manage dialog below (DataTenants, kept under components/data/ to avoid churning its tests).
//
// The board used to sit on `/home`. It moved here when `/home` became the Attention Board: that
// page judges Sites into the same ranked list as every other Subject and has no room for a second,
// differently-shaped view of the same fleet. Detail belongs on the page you open to manage them.
import AppShell from '@/components/common/AppShell.vue'
import DataTenants from '@/components/data/DataTenants.vue'
import TenantCard from '@/components/tenants/TenantCard.vue'
import { api, type TenantSummary } from '@/lib/core/api'
import { useTenantsSummary } from '@/lib/tenants/tenantsQueries'
import { setTenant } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'
import { useRouter } from 'vue-router'
import { computed } from 'vue'

const router = useRouter()
const summaryQuery = useTenantsSummary()
const tenants = computed<TenantSummary[]>(() => summaryQuery.data.value ?? [])

// Column count follows the card count (capped at 4/row) so a small fleet fills the row instead of
// leaving grid whitespace. Literal class strings — Tailwind can't see computed names.
const COLS: Record<number, string> = {
  1: 'grid-cols-1',
  2: 'grid-cols-1 sm:grid-cols-2',
  3: 'grid-cols-1 sm:grid-cols-2 xl:grid-cols-3',
  4: 'grid-cols-1 sm:grid-cols-2 xl:grid-cols-4',
}
const gridClass = computed(() => `grid gap-4 ${COLS[Math.min(tenants.value.length, 4) || 1]}`)

// Full-mode tenants mirror raw telemetry, so their data is queryable here via the tenant context
// dimension — drill into the first signal they actually mirror (`full:traces` mirrors no logs, so
// /logs would be empty). Summary-mode tenants only ever push synthetic health metrics, so there is
// nothing to browse locally; they link out to the tenant's own UI.
function openTenant(tenant: TenantSummary): void {
  if (tenant.mode === 'summary') {
    // No ui_url registered → nothing to open; a '#' tab would just be a blank page.
    if (tenant.ui_url) window.open(tenant.ui_url, '_blank')
    return
  }
  setTenant(tenant.name)
  // mode is `full` (all three) or `full:<csv>` — e.g. `full:traces,metrics`.
  const signals = tenant.mode?.includes(':') ? tenant.mode.slice(tenant.mode.indexOf(':') + 1).split(',') : null
  const path = !signals || signals.includes('logs') ? '/logs' : signals.includes('traces') ? '/traces' : '/metrics'
  router.push(correlate({ path }))
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Tenants">
    <section class="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-5 pt-5 pb-6">
      <div v-if="tenants.length" class="flex flex-col gap-2" data-testid="tenants-board">
        <h3 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">Fleet health</h3>
        <div :class="gridClass">
          <TenantCard v-for="t in tenants" :key="t.name" :tenant="t" @select="openTenant" />
        </div>
      </div>
      <DataTenants />
    </section>
  </AppShell>
</template>

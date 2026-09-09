<script setup lang="ts">
// Infrastructure host list (`/infra`): the fleet as ONE table. Two sources describe a host — the
// metrics its agent samples (CPU / memory / disk / GPU) and the managed-agent registry (status,
// site, Docker facts) — and `fleetRows` unions them, one row per host, ranked worst first, so a
// host enrolled a minute ago and a box that only ships metrics sit in the same list as the ones
// that are hot. Above it, one line of counts that double as the filter (`?status=`). Row click
// drills into InfraHostDetailView (`/infra/:host`).
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { Plus } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import HostFleetSummary from '@/components/infra/HostFleetSummary.vue'
import HostFleetTable from '@/components/infra/HostFleetTable.vue'
import AddHostDialog from '@/components/infra/AddHostDialog.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { api } from '@/lib/core/api'
import { startNs, endNs } from '@/lib/core/context'
import { useUrlParam } from '@/lib/core/useUrlState'
import { useInfraHosts } from '@/lib/infra/infraQueries'
import { fleetRows, fleetSummary, isFleetLevel, type FleetLevel } from '@/lib/infra/fleet'
import { useAgents } from '@/lib/deploy/deployQueries'

const router = useRouter()
const q = useInfraHosts(startNs, endNs)
const hosts = computed(() => q.data.value?.hosts ?? [])
const loading = computed(() => q.isLoading.value)
// "Add host" is the enrolment entry point (SSH bootstrap or a minted token).
const agentsQ = useAgents()
const agents = computed(() => agentsQ.data.value?.hosts ?? [])

const rows = computed(() => fleetRows(hosts.value, agents.value))
const summary = computed(() => fleetSummary(rows.value))

const status = ref<string | null>(null)
const query = ref<string | null>(null)
useUrlParam('status', status, { valid: isFleetLevel })
useUrlParam('q', query)
const filter = computed<FleetLevel | null>(() => (status.value && isFleetLevel(status.value) ? status.value : null))

const addOpen = ref(false)
function open(host: string): void {
  router.push('/infra/' + encodeURIComponent(host))
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Infrastructure">
    <template #actions>
      <GatedButton perm="agents" size="sm" data-testid="add-host" @click="addOpen = true"><Plus class="size-4" /> Add host</GatedButton>
    </template>
    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto p-4" data-testid="infra-hosts">
      <div v-if="loading && !rows.length" class="flex flex-1 items-center justify-center">
        <Spinner size="lg">Loading hosts…</Spinner>
      </div>
      <EmptyState
        v-else-if="!rows.length"
        title="No hosts yet"
        description="Add a host to install photon-agent over SSH, or run it yourself to start collecting CPU, memory, disk, network, and GPU metrics."
      >
        <GatedButton perm="agents" class="mt-3" @click="addOpen = true"><Plus class="size-4" /> Add host</GatedButton>
      </EmptyState>
      <div v-else class="flex flex-col gap-4">
        <HostFleetSummary :summary="summary" :filter="filter" @update:filter="status = $event" />
        <HostFleetTable :rows="rows" :filter="filter" :query="query ?? ''" @select="open" @update:query="query = $event || null" />
      </div>
    </main>
    <AddHostDialog :open="addOpen" @update:open="addOpen = $event" />
  </AppShell>
</template>

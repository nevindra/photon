<script setup lang="ts">
// Infrastructure host list (`/infra`): a fleet KPI band (host/warning/critical counts, avg CPU, GPU
// hosts) above a card grid — one HostCard per host reporting resource metrics via photon-agent, with
// a CPU/MEM/DSK/GPU glance and a degraded flag. Card click drills into InfraHostDetailView
// (`/infra/:host`).
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import HostFleetKpis from '@/components/infra/HostFleetKpis.vue'
import HostCard from '@/components/infra/HostCard.vue'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { api } from '@/lib/core/api'
import { startNs, endNs } from '@/lib/core/context'
import { useInfraHosts } from '@/lib/infra/infraQueries'

const router = useRouter()
const q = useInfraHosts(startNs, endNs)
const hosts = computed(() => q.data.value?.hosts ?? [])
const loading = computed(() => q.isLoading.value)
function open(host: string): void {
  router.push('/infra/' + encodeURIComponent(host))
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Infrastructure">
    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto p-4" data-testid="infra-hosts">
      <div v-if="loading && !hosts.length" class="flex flex-1 items-center justify-center">
        <Spinner size="lg">Loading hosts…</Spinner>
      </div>
      <EmptyState
        v-else-if="!hosts.length"
        title="No hosts reporting"
        description="Run photon-agent on a host to start collecting CPU, memory, disk, network, and GPU metrics."
      />
      <div v-else class="flex flex-col gap-4">
        <HostFleetKpis :hosts="hosts" />
        <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          <HostCard v-for="h in hosts" :key="h.host" :host="h" @select="open" />
        </div>
      </div>
    </main>
  </AppShell>
</template>

<script setup lang="ts">
// Infrastructure host detail (`/infra/:host`): OS/cores/GPU header + a glance stat-tile row
// (`HostStatTiles`) over `HostResourcePanels`' per-resource trend sections — CPU (total/per-core) +
// load average, Memory + Network I/O, Disk, and, when the host has one, a 4-chart GPU section
// (util/memory/temperature/power). Sets the global scope to this host on mount so "Related ▾" and
// cross-signal correlation carry `host.name` + the active time window. Mirrors RumErrorDetailView's
// AppShell + `#lead` back-arrow shell and its route-param normalization.
import { computed, watch } from 'vue'
import { useRoute, RouterLink } from 'vue-router'
import { ArrowLeft } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import HostStatTiles from '@/components/infra/HostStatTiles.vue'
import HostResourcePanels from '@/components/infra/HostResourcePanels.vue'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { formatBytes } from '@/lib/core/format'
import { startNs, endNs, setScope } from '@/lib/core/context'
import { useInfraHost, useHostResourceSeries } from '@/lib/infra/infraQueries'

const route = useRoute()

// Vue Router decodes params automatically; normalize the array form defensively (same pattern
// RumErrorDetailView/RumPageDetailView use for their app/route params).
const host = computed<string>(() => {
  const h = route.params.host
  return ((Array.isArray(h) ? h[0] : h) ?? '').trim()
})
watch(
  host,
  (h) => {
    if (h) setScope({ type: 'host', id: h, label: h })
  },
  { immediate: true },
)

const q = useInfraHost(host, startNs, endNs)
const detail = computed(() => q.data.value ?? null)
const loading = computed(() => q.isLoading.value)
const startMs = computed(() => Number(startNs.value) / 1_000_000)
const endMs = computed(() => Number(endNs.value) / 1_000_000)
const crumb = computed(() => 'Infrastructure › ' + host.value)
const hasGpu = computed(() => (detail.value?.gpus?.length ?? 0) > 0)
const res = useHostResourceSeries(host, startNs, endNs, hasGpu)
</script>

<template>
  <AppShell :mock="api.mock" :crumb="crumb">
    <template #lead>
      <RouterLink
        to="/infra"
        aria-label="Back to hosts"
        class="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
      >
        <ArrowLeft class="size-4" />
      </RouterLink>
    </template>
    <template #actions>
      <RelatedMenu :entity="{ kind: 'host', fields: { host: host } }" />
    </template>

    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4">
      <div v-if="loading && !detail" class="flex flex-1 items-center justify-center">
        <Spinner size="lg">Loading host…</Spinner>
      </div>
      <template v-else>
        <header class="flex flex-wrap gap-4 text-sm text-muted-foreground">
          <span>OS: {{ detail?.os ?? '—' }}</span>
          <span>Cores: {{ detail?.cores ?? '—' }}</span>
          <span>RAM: {{ formatBytes(detail?.totalRamBytes ?? null) }}</span>
          <span>GPU: {{ hasGpu ? detail?.gpus.join(', ') : '—' }}</span>
        </header>
        <HostStatTiles :res="res" :total-ram-bytes="detail?.totalRamBytes ?? null" :has-gpu="hasGpu" />
        <HostResourcePanels :res="res" :start-ms="startMs" :end-ms="endMs" :has-gpu="hasGpu" :gpu-names="detail?.gpus ?? []" />
      </template>
    </main>
  </AppShell>
</template>

<script setup lang="ts">
// Infrastructure host detail (`/infra/:host`): OS/cores/GPU header + a glance stat-tile row
// (`HostStatTiles`) over `HostResourcePanels`' per-resource trend sections — CPU (total/per-core) +
// load average, Memory + Network I/O, Disk, and, when the host has one, a 4-chart GPU section
// (util/memory/temperature/power). Sets the global scope to this host on mount so "Related ▾" and
// cross-signal correlation carry `host.name` + the active time window. The chrome (back button,
// sub-nav, Related menu) is HostScaffold's, shared with the Agent tab.
import { computed } from 'vue'
import HostScaffold from '@/components/infra/HostScaffold.vue'
import HostStatTiles from '@/components/infra/HostStatTiles.vue'
import HostResourcePanels from '@/components/infra/HostResourcePanels.vue'
import HostProcessesTable from '@/components/infra/HostProcessesTable.vue'
import { Spinner } from '@/components/ui/spinner'
import type { InfraProcess } from '@/lib/core/api'
import { formatBytes } from '@/lib/core/format'
import { startNs, endNs } from '@/lib/core/context'
import { useInfraHost, useHostResourceSeries, useInfraHostProcesses } from '@/lib/infra/infraQueries'
import { useRouteParam } from '@/lib/services/useServiceParam'

const host = useRouteParam('host')

const q = useInfraHost(host, startNs, endNs)
const detail = computed(() => q.data.value ?? null)
const loading = computed(() => q.isLoading.value)
const startMs = computed(() => Number(startNs.value) / 1_000_000)
const endMs = computed(() => Number(endNs.value) / 1_000_000)
const hasGpu = computed(() => (detail.value?.gpus?.length ?? 0) > 0)
const res = useHostResourceSeries(host, startNs, endNs, hasGpu)

// --- Processes: every supervised process on this host, ranked (default heaviest CPU first). ---
const procQ = useInfraHostProcesses(host, startNs, endNs)
const processes = computed<InfraProcess[]>(() => procQ.data.value?.processes ?? [])
const procLoading = computed(() => procQ.isLoading.value)
</script>

<template>
  <HostScaffold :host="host">
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

        <!-- Processes: every supervised process on this host, ranked by resource usage. -->
        <HostProcessesTable :processes="processes" :loading="procLoading" />
      </template>
    </main>
  </HostScaffold>
</template>

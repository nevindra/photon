<script setup lang="ts">
// Infrastructure host detail (`/infra/:host`): OS/cores/GPU header + a glance stat-tile row
// (`HostStatTiles`) over `HostResourcePanels`' per-resource trend sections — CPU (total/per-core) +
// load average, Memory + Network I/O, Disk, and, when the host has one, a 4-chart GPU section
// (util/memory/temperature/power). Sets the global scope to this host on mount so "Related ▾" and
// cross-signal correlation carry `host.name` + the active time window. Mirrors RumErrorDetailView's
// AppShell + `#lead` back-arrow shell and its route-param normalization.
import { computed, ref, watch } from 'vue'
import { useRoute, RouterLink } from 'vue-router'
import { ArrowLeft } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import HostStatTiles from '@/components/infra/HostStatTiles.vue'
import HostResourcePanels from '@/components/infra/HostResourcePanels.vue'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { Spinner } from '@/components/ui/spinner'
import { api, type InfraProcess } from '@/lib/core/api'
import { formatBytes, formatNumber } from '@/lib/core/format'
import { startNs, endNs, setScope } from '@/lib/core/context'
import { useInfraHost, useHostResourceSeries, useInfraHostProcesses } from '@/lib/infra/infraQueries'

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

// --- Processes table: every worker on this host, ranked (default heaviest CPU first). ---
const procQ = useInfraHostProcesses(host, startNs, endNs)
const processes = computed<InfraProcess[]>(() => procQ.data.value?.processes ?? [])
const procLoading = computed(() => procQ.isLoading.value)

type ProcSortKey = 'process' | 'cpuPct' | 'rssBytes' | 'fds' | 'threads' | 'restarts'
const sortKey = ref<ProcSortKey>('cpuPct')
const sortDir = ref<'asc' | 'desc'>('desc')

function toggleSort(key: ProcSortKey): void {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 'asc' ? 'desc' : 'asc'
  } else {
    sortKey.value = key
    // Text sorts read best ascending; numeric columns default to heaviest-first.
    sortDir.value = key === 'process' ? 'asc' : 'desc'
  }
}

function sortArrow(key: ProcSortKey): string {
  if (sortKey.value !== key) return ''
  return sortDir.value === 'asc' ? ' ↑' : ' ↓'
}

const sortedProcesses = computed<InfraProcess[]>(() => {
  const rows = [...processes.value]
  const key = sortKey.value
  const dir = sortDir.value === 'asc' ? 1 : -1
  rows.sort((a, b) => {
    if (key === 'process') return String(a.process).localeCompare(String(b.process)) * dir
    // Nulls sort to the bottom regardless of direction (missing metric = least interesting).
    const av = a[key]
    const bv = b[key]
    if (av == null && bv == null) return 0
    if (av == null) return 1
    if (bv == null) return -1
    return (Number(av) - Number(bv)) * dir
  })
  return rows
})

function fmtPct(v: number | null): string {
  return v == null ? '—' : v.toFixed(1) + '%'
}
function fmtCount(v: number | null): string {
  return v == null ? '—' : formatNumber(Math.round(v))
}
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

        <!-- Processes: every mandor worker on this host, ranked by resource usage. -->
        <section class="flex flex-col gap-2" data-testid="host-processes">
          <div class="flex items-center gap-2.5 text-xs text-muted-foreground">
            <h2 class="text-sm font-medium text-foreground">Processes</h2>
            <span class="font-mono tabular-nums">{{ formatNumber(processes.length) }}</span>
            <Spinner v-if="procLoading && !processes.length" size="sm">loading…</Spinner>
          </div>
          <div v-if="processes.length" class="overflow-x-auto rounded-md border border-border">
            <table class="w-full text-sm">
              <thead>
                <tr class="border-b border-border text-xs text-muted-foreground">
                  <th
                    class="cursor-pointer select-none px-3 py-2 text-left font-medium hover:text-foreground"
                    @click="toggleSort('process')"
                  >
                    Process<span class="tabular-nums">{{ sortArrow('process') }}</span>
                  </th>
                  <th
                    class="cursor-pointer select-none px-3 py-2 text-right font-medium hover:text-foreground"
                    @click="toggleSort('cpuPct')"
                  >
                    CPU %<span class="tabular-nums">{{ sortArrow('cpuPct') }}</span>
                  </th>
                  <th
                    class="cursor-pointer select-none px-3 py-2 text-right font-medium hover:text-foreground"
                    @click="toggleSort('rssBytes')"
                  >
                    RSS<span class="tabular-nums">{{ sortArrow('rssBytes') }}</span>
                  </th>
                  <th
                    class="cursor-pointer select-none px-3 py-2 text-right font-medium hover:text-foreground"
                    @click="toggleSort('fds')"
                  >
                    FDs<span class="tabular-nums">{{ sortArrow('fds') }}</span>
                  </th>
                  <th
                    class="cursor-pointer select-none px-3 py-2 text-right font-medium hover:text-foreground"
                    @click="toggleSort('threads')"
                  >
                    Threads<span class="tabular-nums">{{ sortArrow('threads') }}</span>
                  </th>
                  <th
                    class="cursor-pointer select-none px-3 py-2 text-right font-medium hover:text-foreground"
                    @click="toggleSort('restarts')"
                  >
                    Restarts<span class="tabular-nums">{{ sortArrow('restarts') }}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="p in sortedProcesses"
                  :key="p.process"
                  data-testid="process-row"
                  :data-process="p.process"
                  class="border-b border-border/50 last:border-0 hover:bg-accent/50"
                >
                  <td class="px-3 py-2 font-mono text-foreground">{{ p.process }}</td>
                  <td class="px-3 py-2 text-right tabular-nums">{{ fmtPct(p.cpuPct) }}</td>
                  <td class="px-3 py-2 text-right tabular-nums">{{ formatBytes(p.rssBytes) }}</td>
                  <td class="px-3 py-2 text-right tabular-nums">{{ fmtCount(p.fds) }}</td>
                  <td class="px-3 py-2 text-right tabular-nums">{{ fmtCount(p.threads) }}</td>
                  <td class="px-3 py-2 text-right tabular-nums">{{ fmtCount(p.restarts) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <p v-else-if="!procLoading" class="text-sm text-muted-foreground">No processes reporting on this host.</p>
        </section>
      </template>
    </main>
  </AppShell>
</template>

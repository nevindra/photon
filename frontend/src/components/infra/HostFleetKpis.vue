<script setup lang="ts">
// Fleet KPI band above the /infra host card grid: total/warning/critical host counts, mean CPU,
// and GPU-host count — all derived client-side from the already-fetched host list (no extra query),
// same StatTile row idiom as HostStatTiles on /infra/:host. Per-host status is the worst of
// cpu/mem/disk/gpu utilization through the shared hostStatus/utilAccent thresholds; a critical host
// counts only toward Critical, never double-counted as Warning.
import { computed } from 'vue'
import { StatTile } from '@/components/ui/stat-tile'
import { formatPct, hostStatus } from '@/lib/infra/hostStats'
import type { InfraHost } from '@/lib/core/api'

const props = defineProps<{ hosts: InfraHost[] }>()

function statusOf(h: InfraHost): 'error' | 'warning' | undefined {
  return hostStatus([h.cpuUtil, h.memUtil, h.diskUtil, h.gpuUtil])
}

const warningCount = computed(() => props.hosts.filter((h) => statusOf(h) === 'warning').length)
const criticalCount = computed(() => props.hosts.filter((h) => statusOf(h) === 'error').length)
const gpuCount = computed(() => props.hosts.filter((h) => h.hasGpu).length)
const avgCpu = computed(() => {
  const vals = props.hosts.map((h) => h.cpuUtil).filter((v): v is number => v != null)
  return vals.length ? vals.reduce((a, b) => a + b, 0) / vals.length : null
})
</script>

<template>
  <div class="grid grid-cols-2 gap-4 md:grid-cols-3 xl:grid-cols-5" data-testid="infra-fleet-kpis">
    <StatTile label="Hosts" :value="hosts.length" />
    <StatTile label="Warning" :value="warningCount" :accent="warningCount > 0 ? 'warning' : undefined" />
    <StatTile label="Critical" :value="criticalCount" :accent="criticalCount > 0 ? 'error' : undefined" />
    <StatTile label="Avg CPU" :value="formatPct(avgCpu)" />
    <StatTile label="GPU hosts" :value="gpuCount" />
  </div>
</template>

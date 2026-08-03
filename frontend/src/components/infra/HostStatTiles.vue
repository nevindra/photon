<script setup lang="ts">
// Glance layer for /infra/:host: one current-state tile per resource, derived from the LAST point
// of the SAME series the trend panels below chart (no extra API calls — `res` is the shared
// useHostResourceSeries bundle). Percent tiles tint warn/error at the shared 80%/90% thresholds.
//
// Disk and GPU are label-split (mountpoints, devices) and their headline is the WORST group — one
// full disk or one saturated card is a real problem even when its neighbours idle, and a plain
// average would hide it. But a bare max misleads the other way: 76% above a chart of four GPUs,
// three of them near zero, reads as a busy host. So every split resource shows the pair —
// "76% max · 20% avg" — plus a sub-label naming the group the max came from.
import { computed } from 'vue'
import { StatTile } from '@/components/ui/stat-tile'
import { Sparkline } from '@/components/ui/sparkline'
import { formatBytes, formatRate } from '@/lib/core/format'
import type { HostResourceSeries } from '@/lib/infra/infraQueries'
import type { GroupStat } from '@/lib/infra/hostStats'
import {
  cpuSeriesForMode, formatPct, groupStat, latestTotal, latestValue, sparkValues, utilAccent,
} from '@/lib/infra/hostStats'

const props = defineProps<{
  res: HostResourceSeries
  totalRamBytes: number | null
  hasGpu: boolean
}>()

const cpuTotal = computed(() => cpuSeriesForMode(props.res.cpu.data.value?.series, 'total')[0])
const cpuFrac = computed(() => latestValue(cpuTotal.value))
const memSeries = computed(() => props.res.memory.data.value?.series?.[0])
const memFrac = computed(() => latestValue(memSeries.value))
const memSub = computed(() => {
  if (memFrac.value == null || props.totalRamBytes == null) return undefined
  return `${formatBytes(memFrac.value * props.totalRamBytes)} / ${formatBytes(props.totalRamBytes)}`
})
const netRate = computed(() => latestTotal(props.res.network.data.value?.series))

const disk = computed(() => groupStat(props.res.disk.data.value?.series, 'mountpoint'))
const gpu = computed(() => groupStat(props.res.gpu.data.value?.series, 'gpu'))
const gpuTemp = computed(() => groupStat(props.res.gpuTemp.data.value?.series, 'gpu'))

// "max · <mean> avg", but only where a mean means anything: with a single mountpoint or GPU the
// two numbers are the same and the pair is noise, so the tile stays a plain reading.
function pair(g: GroupStat, fmt: (v: number | null) => string): string | undefined {
  return g.groups < 2 || g.mean == null ? undefined : `max · ${fmt(g.mean)} avg`
}
// Which group the max came from, and out of how many: "/data of 5 mounts", "gpu 2 of 4".
function source(g: GroupStat, noun: (id: string) => string, unit = ''): string | undefined {
  if (g.label == null) return undefined
  return g.groups < 2 ? noun(g.label) : `${noun(g.label)} of ${g.groups}${unit}`
}
const degrees = (v: number | null): string => (v == null ? '—' : `${Math.round(v)}°C`)

const diskPair = computed(() => pair(disk.value, formatPct))
const diskSource = computed(() => source(disk.value, (m) => m, ' mounts'))
const gpuPair = computed(() => pair(gpu.value, formatPct))
const gpuSource = computed(() => source(gpu.value, (id) => `gpu ${id}`))
const gpuTempPair = computed(() => pair(gpuTemp.value, degrees))
const gpuTempSource = computed(() => source(gpuTemp.value, (id) => `gpu ${id}`))
</script>

<template>
  <div class="grid grid-cols-2 gap-4 md:grid-cols-3" :class="hasGpu ? 'xl:grid-cols-6' : 'xl:grid-cols-4'">
    <StatTile label="CPU" :value="formatPct(cpuFrac)" :accent="utilAccent(cpuFrac)">
      <template #spark><Sparkline :points="sparkValues(cpuTotal)" /></template>
    </StatTile>
    <StatTile label="Memory" :value="formatPct(memFrac)" :sub="memSub" :accent="utilAccent(memFrac)">
      <template #spark><Sparkline :points="sparkValues(memSeries)" /></template>
    </StatTile>
    <StatTile
      label="Disk"
      :value="formatPct(disk.worst)"
      :secondary="diskPair"
      :sub="diskSource"
      :accent="utilAccent(disk.worst)"
    />
    <StatTile label="Network ⇅" :value="netRate == null ? '—' : formatRate(netRate)" />
    <template v-if="hasGpu">
      <StatTile
        label="GPU"
        :value="formatPct(gpu.worst)"
        :secondary="gpuPair"
        :sub="gpuSource"
        :accent="utilAccent(gpu.worst)"
      />
      <StatTile
        label="GPU temp"
        :value="degrees(gpuTemp.worst)"
        :secondary="gpuTempPair"
        :sub="gpuTempSource"
      />
    </template>
  </div>
</template>

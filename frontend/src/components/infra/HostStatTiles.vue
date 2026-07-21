<script setup lang="ts">
// Glance layer for /infra/:host: one current-state tile per resource, derived from the LAST point
// of the SAME series the trend panels below chart (no extra API calls — `res` is the shared
// useHostResourceSeries bundle). Percent tiles tint warn/error at the shared 80%/90% thresholds.
import { computed } from 'vue'
import { StatTile } from '@/components/ui/stat-tile'
import { Sparkline } from '@/components/ui/sparkline'
import { formatBytes, formatRate } from '@/lib/core/format'
import type { HostResourceSeries } from '@/lib/infra/infraQueries'
import {
  cpuSeriesForMode, formatPct, latestTotal, latestValue, sparkValues, utilAccent, worstSeries,
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
const worstDisk = computed(() => worstSeries(props.res.disk.data.value?.series, 'mountpoint'))
const netRate = computed(() => latestTotal(props.res.network.data.value?.series))
const gpuFrac = computed(() => worstSeries(props.res.gpu.data.value?.series, 'gpu')?.value ?? null)
const gpuTemp = computed(() => worstSeries(props.res.gpuTemp.data.value?.series, 'gpu')?.value ?? null)
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
      :value="formatPct(worstDisk?.value ?? null)"
      :sub="worstDisk?.label"
      :accent="utilAccent(worstDisk?.value ?? null)"
    />
    <StatTile label="Network ⇅" :value="netRate == null ? '—' : formatRate(netRate)" />
    <template v-if="hasGpu">
      <StatTile label="GPU" :value="formatPct(gpuFrac)" :accent="utilAccent(gpuFrac)" />
      <StatTile label="GPU temp" :value="gpuTemp == null ? '—' : `${Math.round(gpuTemp)}°C`" />
    </template>
  </div>
</template>

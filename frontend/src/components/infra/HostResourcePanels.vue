<script setup lang="ts">
// Trend layer for /infra/:host (layout B): one titled `ChartPanel` card per resource, every chart
// bound to the global time range. CPU defaults to the total series with a Segmented per-core toggle
// in the card's #summary slot (client-side filter — per-core data is already in the same query).
// GPU gets its own 4-card section.
import { computed, ref } from 'vue'
import ChartPanel from '@/components/charts/ChartPanel.vue'
import MetricChart from '@/components/metrics/MetricChart.vue'
import { Meter } from '@/components/ui/meter'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { cpuSeriesForMode, formatPct, latestValue, utilAccent } from '@/lib/infra/hostStats'
import type { HostResourceSeries } from '@/lib/infra/infraQueries'

const props = defineProps<{
  res: HostResourceSeries
  startMs: number
  endMs: number
  hasGpu: boolean
  gpuNames: string[]
}>()

const cpuMode = ref<'total' | 'per-core'>('total')
// Reka toggle groups emit '' on deselect — swallow it so a mode is always active.
function setCpuMode(v: unknown) {
  if (v === 'total' || v === 'per-core') cpuMode.value = v
}
const cpuSeries = computed(() => cpuSeriesForMode(props.res.cpu.data.value?.series, cpuMode.value))
const diskSeries = computed(() => props.res.disk.data.value?.series ?? [])
const diskMeters = computed(() =>
  diskSeries.value
    .map((s) => ({ mountpoint: s.labels.mountpoint ?? '?', frac: latestValue(s) }))
    .filter((m) => m.frac != null)
    .sort((a, b) => (b.frac ?? 0) - (a.frac ?? 0)),
)
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
      <ChartPanel title="CPU utilization" class="lg:col-span-2">
        <template #summary>
          <Segmented :model-value="cpuMode" @update:model-value="setCpuMode">
            <SegmentedItem value="total">Total</SegmentedItem>
            <SegmentedItem value="per-core">Per-core</SegmentedItem>
          </Segmented>
        </template>
        <MetricChart :series="cpuSeries" percent :y-range="[0, 100]" :start-ms="startMs" :end-ms="endMs" :loading="res.cpu.isLoading.value" viz="line" />
      </ChartPanel>
      <ChartPanel title="Load average (1m)">
        <MetricChart :series="res.load.data.value?.series ?? []" unit="1" :start-ms="startMs" :end-ms="endMs" :loading="res.load.isLoading.value" viz="line" />
      </ChartPanel>
    </div>

    <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <ChartPanel title="Memory utilization">
        <MetricChart :series="res.memory.data.value?.series ?? []" percent :y-range="[0, 100]" :start-ms="startMs" :end-ms="endMs" :loading="res.memory.isLoading.value" viz="line" />
      </ChartPanel>
      <ChartPanel title="Network I/O">
        <MetricChart :series="res.network.data.value?.series ?? []" unit="By/s" :start-ms="startMs" :end-ms="endMs" :loading="res.network.isLoading.value" viz="area" />
      </ChartPanel>
    </div>

    <ChartPanel title="Disk">
      <div v-if="diskMeters.length" class="mb-3 flex flex-col gap-2">
        <div v-for="m in diskMeters" :key="m.mountpoint" class="flex items-center gap-3 text-xs">
          <span class="w-32 truncate font-mono text-muted-foreground">{{ m.mountpoint }}</span>
          <Meter :value="m.frac ?? 0" :tone="utilAccent(m.frac) ?? 'info'" class="flex-1" />
          <span class="w-12 text-right tabular-nums">{{ formatPct(m.frac) }}</span>
        </div>
      </div>
      <MetricChart :series="diskSeries" percent :y-range="[0, 100]" :start-ms="startMs" :end-ms="endMs" :loading="res.disk.isLoading.value" viz="line" />
    </ChartPanel>

    <div v-if="hasGpu" class="grid grid-cols-1 gap-4 lg:grid-cols-2 xl:grid-cols-4">
      <ChartPanel title="GPU utilization" :subtitle="gpuNames.join(', ')">
        <MetricChart :series="res.gpu.data.value?.series ?? []" percent :y-range="[0, 100]" :start-ms="startMs" :end-ms="endMs" :loading="res.gpu.isLoading.value" viz="line" />
      </ChartPanel>
      <ChartPanel title="GPU memory">
        <MetricChart :series="res.gpuMemory.data.value?.series ?? []" percent :y-range="[0, 100]" :start-ms="startMs" :end-ms="endMs" :loading="res.gpuMemory.isLoading.value" viz="line" />
      </ChartPanel>
      <ChartPanel title="GPU temperature">
        <MetricChart :series="res.gpuTemp.data.value?.series ?? []" unit="°C" :start-ms="startMs" :end-ms="endMs" :loading="res.gpuTemp.isLoading.value" viz="line" />
      </ChartPanel>
      <ChartPanel title="GPU power">
        <MetricChart :series="res.gpuPower.data.value?.series ?? []" unit="W" :start-ms="startMs" :end-ms="endMs" :loading="res.gpuPower.isLoading.value" viz="line" />
      </ChartPanel>
    </div>
  </div>
</template>

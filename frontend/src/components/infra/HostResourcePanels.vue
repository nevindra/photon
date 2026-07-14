<script setup lang="ts">
// Per-resource utilization charts for one host (Infrastructure detail, `/infra/:host`): CPU/Memory/
// Disk/Network always render; GPU only when the host reports one. Each panel is its own
// `useInfraHostSeries` query so panels load/refresh independently; charts reuse the existing
// `MetricChart` (line/area viz) — no bespoke chart code.
import { computed } from 'vue'
import MetricChart from '@/components/metrics/MetricChart.vue'
import { useInfraHostSeries } from '@/lib/infra/infraQueries'

const props = defineProps<{ host: string; startMs: number; endMs: number; hasGpu: boolean }>()
const startNs = computed(() => String(BigInt(Math.round(props.startMs)) * 1_000_000n))
const endNs = computed(() => String(BigInt(Math.round(props.endMs)) * 1_000_000n))

const cpu = useInfraHostSeries(() => props.host, 'cpu', startNs, endNs)
const mem = useInfraHostSeries(() => props.host, 'memory', startNs, endNs)
const disk = useInfraHostSeries(() => props.host, 'disk', startNs, endNs)
const net = useInfraHostSeries(() => props.host, 'network', startNs, endNs)
const gpu = useInfraHostSeries(() => props.host, 'gpu', startNs, endNs, () => props.hasGpu)
</script>

<template>
  <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
    <section>
      <h3 class="mb-2 text-sm font-medium">CPU utilization</h3>
      <MetricChart :series="cpu.data.value?.series ?? []" unit="1" :start-ms="startMs" :end-ms="endMs" :loading="cpu.isLoading.value" viz="line" />
    </section>
    <section>
      <h3 class="mb-2 text-sm font-medium">Memory utilization</h3>
      <MetricChart :series="mem.data.value?.series ?? []" unit="1" :start-ms="startMs" :end-ms="endMs" :loading="mem.isLoading.value" viz="line" />
    </section>
    <section>
      <h3 class="mb-2 text-sm font-medium">Filesystem utilization</h3>
      <MetricChart :series="disk.data.value?.series ?? []" unit="1" :start-ms="startMs" :end-ms="endMs" :loading="disk.isLoading.value" viz="line" />
    </section>
    <section>
      <h3 class="mb-2 text-sm font-medium">Network I/O</h3>
      <MetricChart :series="net.data.value?.series ?? []" unit="By/s" :start-ms="startMs" :end-ms="endMs" :loading="net.isLoading.value" viz="area" />
    </section>
    <section v-if="hasGpu" class="lg:col-span-2">
      <h3 class="mb-2 text-sm font-medium">GPU utilization</h3>
      <MetricChart :series="gpu.data.value?.series ?? []" unit="1" :start-ms="startMs" :end-ms="endMs" :loading="gpu.isLoading.value" viz="line" />
    </section>
  </div>
</template>

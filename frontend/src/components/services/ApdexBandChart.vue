<script setup>
// Apdex band breakdown (satisfied / tolerating / frustrated) as stacked bars — makes the Apdex
// score legible ("why 0.71"). Thin adapter over charts/BarChart.vue, consistent with
// ServiceVolumeChart. satisfied = emerald "good" (no --sev-good token, so a fixed emerald hsl),
// tolerating = sev-warn, frustrated = sev-error.
import { computed } from 'vue'
import BarChart from '@/components/charts/BarChart.vue'
import { useChartTheme } from '@/components/charts/useChartTheme.js'
import { formatNumber } from '@/lib/core/format'

const props = defineProps({
  buckets: { type: Array, default: () => [] }, // [{ ts, satisfied, tolerating, frustrated }]
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  loading: { type: Boolean, default: false },
})

const { version } = useChartTheme()
const colors = computed(() => {
  void version.value
  const cs = typeof document !== 'undefined' && typeof getComputedStyle === 'function'
    ? getComputedStyle(document.documentElement) : null
  const raw = (n, f) => (cs?.getPropertyValue(n) || '').trim() || f
  return {
    satisfied: 'hsl(160 84% 39%)', // emerald — reads well in both themes
    tolerating: `hsl(${raw('--sev-warn', '38 92% 50%')})`,
    frustrated: `hsl(${raw('--sev-error', '0 72% 51%')})`,
  }
})

const NS = 1_000_000n
const STACK = [
  { key: 'satisfied', label: 'Satisfied' },
  { key: 'tolerating', label: 'Tolerating' },
  { key: 'frustrated', label: 'Frustrated' },
]
const chartBuckets = computed(() =>
  props.buckets.map((b) => ({
    t: Number(BigInt(b?.ts ?? 0) / NS),
    segments: STACK.map((s) => ({
      key: s.key,
      label: s.label,
      color: colors.value[s.key],
      value: Number(b?.[s.key]) || 0,
    })),
  })),
)
</script>

<template>
  <BarChart :buckets="chartBuckets" :start-ms="startMs" :end-ms="endMs" :stacked="true" :format-value="formatNumber" :loading="loading" />
</template>

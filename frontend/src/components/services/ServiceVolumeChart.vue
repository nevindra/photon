<script setup>
// Absolute request/error volume as stacked bars — a thin adapter over charts/BarChart.vue (same
// technique as traces/SpanVolumeHistogram.vue), but it takes `buckets` as a prop (the detail
// view's single timeseries fetch) instead of owning a fetch, and stacks ok vs error absolute
// counts rather than status. Colour is reserved for `error` (red); `ok` is neutral grey.
import { computed } from 'vue'
import BarChart from '@/components/charts/BarChart.vue'
import { useChartTheme } from '@/components/charts/useChartTheme.js'
import { formatNumber } from '@/lib/core/format'

const props = defineProps({
  buckets: { type: Array, default: () => [] }, // [{ ts: epoch ms, count, error_count }]
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  loading: { type: Boolean, default: false },
  markers: { type: Array, default: () => [] }, // [{ xMs, label, color }] — deploy lines
})

// Resolve tokens to concrete colours for the canvas, re-resolving on a light↔dark flip.
const { version } = useChartTheme()
const colors = computed(() => {
  void version.value
  const cs = typeof document !== 'undefined' && typeof getComputedStyle === 'function'
    ? getComputedStyle(document.documentElement) : null
  const raw = (n, f) => (cs?.getPropertyValue(n) || '').trim() || f
  return {
    ok: `hsl(${raw('--muted-foreground', '0 0% 45.1%')})`,
    error: `hsl(${raw('--sev-error', '0 72% 51%')})`,
  }
})

// `/api/services/:service/timeseries` sends `ts` as an epoch-MILLISECOND number
// (`crates/photon-api/src/services.rs`), and `BarChart` plots milliseconds. This used to divide by
// 1e6, from back when the route sent nanosecond strings — which put every bar around epoch 1970,
// outside `[startMs, endMs]`, and drew an empty chart.
const chartBuckets = computed(() =>
  props.buckets.map((b) => {
    const total = Number(b?.count ?? 0)
    const err = Number(b?.error_count ?? 0)
    return {
      t: Number(b?.ts ?? 0),
      segments: [
        { key: 'ok', label: 'Ok', color: colors.value.ok, value: Math.max(0, total - err) },
        { key: 'error', label: 'Error', color: colors.value.error, value: err },
      ],
    }
  }),
)
</script>

<template>
  <BarChart :markers="markers" :buckets="chartBuckets" :start-ms="startMs" :end-ms="endMs" :stacked="true" :format-value="formatNumber" :loading="loading" />
</template>

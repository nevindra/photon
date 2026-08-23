<script setup>
// Apdex band breakdown (satisfied / tolerating / frustrated) as stacked bars — makes the Apdex
// score legible ("why 0.71").
//
// This is the ONE chart rendered by TanStack Charts rather than uPlot (Task 21). It was picked
// because it is a plain stacked bar over evenly-spaced time buckets with no brush, no drag-zoom and
// no drag-to-filter — the interactions TanStack Charts has no documented equivalent for. Its twin
// `ServiceVolumeChart` stays on uPlot deliberately, so the two engines sit side by side on the same
// page over the same buckets and can be compared on something real. See `docs/frontend.md`. satisfied = emerald "good" (no --sev-good token, so a fixed emerald hsl),
// tolerating = sev-warn, frustrated = sev-error.
import { computed } from 'vue'
// Loaded lazily, exactly like the fleet graph's layout library: TanStack Charts costs ~97 kB raw /
// ~34 kB gzipped — nearly twice uPlot's 52 kB, which draws every OTHER chart in the app — so it
// must not sit in the initial bundle that someone opening `/logs` pays for.
import { LazyStackedBandChart } from '@/components/charts/tanstack/lazy'
import { useChartTheme } from '@/components/charts/useChartTheme.js'
import { formatNumber } from '@/lib/core/format'

const props = defineProps({
  buckets: { type: Array, default: () => [] }, // [{ ts, satisfied, tolerating, frustrated }]
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  loading: { type: Boolean, default: false },
  markers: { type: Array, default: () => [] }, // [{ xMs, label, color }] — deploy lines
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

// `/api/services/:service/timeseries` sends `ts` as an epoch-MILLISECOND number
// (`crates/photon-api/src/services.rs`), and `BarChart` plots milliseconds. This used to divide by
// 1e6, from back when the route sent nanosecond strings — which put every bar around epoch 1970,
// outside `[startMs, endMs]`, and drew an empty chart.
const STACK = [
  { key: 'satisfied', label: 'Satisfied' },
  { key: 'tolerating', label: 'Tolerating' },
  { key: 'frustrated', label: 'Frustrated' },
]
const chartBuckets = computed(() =>
  props.buckets.map((b) => ({
    t: Number(b?.ts ?? 0),
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
  <LazyStackedBandChart
    :buckets="chartBuckets"
    :start-ms="startMs"
    :end-ms="endMs"
    :markers="markers"
    :loading="loading"
    :format-value="formatNumber"
    aria-label="Apdex bands over time"
  />
</template>

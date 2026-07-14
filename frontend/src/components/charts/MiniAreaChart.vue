<script setup>
// A compact, INTERACTIVE trend chart for inline slots (e.g. the /data Storage cards' on-disk
// footprint). It's a thin wrapper over LineChart in `compact` mode — so it renders through the very
// same BaseChart pipeline as every other chart and inherits its crosshair + floating hover tooltip
// for free — but with the axes/gridlines/legend hidden and a small height, so it reads as a
// sparkline rather than a full chart. This is deliberately NOT `ui/sparkline` (a static SVG with no
// interactivity): a trend on a data page should behave like the app's other charts.
//
// Accepts `points` as a bare number[] (index becomes x) or a [{ t: <ms>, v }] series; passing real
// timestamps makes the hover tooltip show the bucket time. `formatValue` formats the hovered value.
import { computed } from 'vue'
import { formatNumber } from '@/lib/core/format'
import LineChart from './LineChart.vue'

const props = defineProps({
  points: { type: Array, default: () => [] }, // number[] (x = index) OR [{ t, v }]
  color: { type: String, default: null }, //     series stroke/fill hue; defaults to brand cyan
  label: { type: String, default: 'value' }, //   tooltip row label
  height: { type: Number, default: 48 },
  area: { type: Boolean, default: true },
  formatValue: { type: Function, default: formatNumber },
})

// One LineChart series; normalize bare numbers into { t: index, v }.
const series = computed(() => [
  {
    key: 'v',
    label: props.label,
    color: props.color ?? undefined,
    points: props.points.map((p, i) =>
      p != null && typeof p === 'object'
        ? { t: Number(p.t), v: p.v == null ? null : Number(p.v) }
        : { t: i, v: p == null ? null : Number(p) },
    ),
  },
])

// x window = the points' own extent (LineChart requires explicit start/end; axes are hidden anyway).
const bounds = computed(() => {
  const ts = series.value[0].points.map((p) => p.t)
  return ts.length ? { startMs: Math.min(...ts), endMs: Math.max(...ts) } : { startMs: 0, endMs: 1 }
})
</script>

<template>
  <LineChart
    :series="series"
    :start-ms="bounds.startMs"
    :end-ms="bounds.endMs"
    :format-value="formatValue"
    :area="area"
    :height="height"
    compact
  />
</template>

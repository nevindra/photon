<!-- frontend/src/components/data/UsageChart.vue -->
<script setup>
// Thin adapter over charts/LineChart for the /data page's usage-over-time charts (storage
// footprint by signal, ingestion rate by signal). DataOverview hands us the same shape it always
// has — `[{ key, points:[{t:<ms>, v}] }]` plus `startMs`/`endMs`/`formatValue`/`area`/`loading` —
// we just relabel each series (`label` = `key`) for LineChart's contract and pass everything else
// straight through. LineChart already derives per-series colour (via the same `seriesColor` hash
// this component used to call directly), the empty/loading overlay, and isolated single-point dots
// (`isolatedPointIndices`), so none of that lives here anymore.
import { computed } from 'vue'
import LineChart from '@/components/charts/LineChart.vue'

const props = defineProps({
  series: { type: Array, default: () => [] },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  formatValue: { type: Function, default: (v) => String(v) },
  area: { type: Boolean, default: false },
  stacked: { type: Boolean, default: false }, // stack signals into a filled TOTAL band (footprint)
  loading: { type: Boolean, default: false },
})

// LineChart wants a `label` per series; usage series have no label of their own today, so the
// key doubles as the display label (matches the old component's implicit `seriesColor(key)`
// labelling — legend chips read the same signal name).
const lineSeries = computed(() =>
  props.series.map((s) => ({ key: s.key, label: s.key, points: s.points })),
)
</script>

<template>
  <LineChart
    :series="lineSeries"
    :start-ms="startMs"
    :end-ms="endMs"
    :format-value="formatValue"
    :area="area"
    :stacked="stacked"
    :loading="loading"
  />
</template>

<!-- frontend/src/components/metrics/MetricChart.vue -->
<script setup>
// Viz-routing adapter over the charts/ layer. Maps the metrics-domain `series` (labels +
// ns-string timestamps) onto the right renderer per `viz`: line/area/stacked → LineChart,
// bar/stacked-bar → BarChart (via seriesToBuckets), stat → MetricStat, table → MetricLegendTable.
import { computed } from 'vue'
import { seriesColor, seriesLabelKey } from '@/lib/core/seriesColor'
import { formatNumber } from '@/lib/core/format'
import { seriesToBuckets } from '@/lib/metrics/metricViz'
import LineChart from '@/components/charts/LineChart.vue'
import BarChart from '@/components/charts/BarChart.vue'
import MetricStat from '@/components/metrics/MetricStat.vue'
import MetricLegendTable from '@/components/metrics/MetricLegendTable.vue'

const props = defineProps({
  series: { type: Array, default: () => [] },
  unit: { type: String, default: '' },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  highlightKey: { type: String, default: null },
  loading: { type: Boolean, default: false },
  viz: { type: String, default: 'line' },
  yLog: { type: Boolean, default: false },
})
const emit = defineEmits(['exemplar', 'zoom', 'legend-toggle', 'point-click', 'highlight'])

const lineSeries = computed(() =>
  props.series.map((s) => {
    const key = seriesLabelKey(s.labels)
    return {
      key,
      label: key,
      color: seriesColor(key).stroke,
      points: s.points.map((p) => ({ t: Number(p.t) / 1e6, v: p.v })),
    }
  }),
)
const buckets = computed(() => seriesToBuckets(props.series))

const isBar = computed(() => ['bar', 'stacked-bar'].includes(props.viz))
const area = computed(() => props.viz === 'area' || (props.viz === 'line' && props.series.length === 1))
const stacked = computed(() => props.viz === 'stacked')
const barStacked = computed(() => props.viz === 'stacked-bar')

function formatValue(v) {
  return formatNumber(v) + (props.unit && props.unit !== '1' ? ' ' + props.unit : '')
}

// LineChart/BarChart emit point-click with x in x-scale units (seconds on a time axis). Convert to ms.
function onPointClick({ x }) {
  emit('point-click', { tMs: Math.round(x * 1000) })
}
// BarChart's zoom already carries ms; LineChart's too. Pass through.
function onZoom(e) { emit('zoom', e) }

defineExpose({ onPointClick })
</script>

<template>
  <MetricStat v-if="viz === 'stat'" :series="series" :unit="unit" :loading="loading" />

  <MetricLegendTable
    v-else-if="viz === 'table'"
    :series="series" :unit="unit" :highlight-key="highlightKey"
    @highlight="emit('highlight', $event)"
  />

  <BarChart
    v-else-if="isBar"
    :buckets="buckets"
    :start-ms="startMs"
    :end-ms="endMs"
    :stacked="barStacked"
    :format-value="formatValue"
    :loading="loading"
    @zoom="onZoom"
    @legend-toggle="(e) => emit('legend-toggle', e)"
  />

  <LineChart
    v-else
    :series="lineSeries"
    :start-ms="startMs"
    :end-ms="endMs"
    :format-value="formatValue"
    :area="area"
    :stacked="stacked"
    :y-log="yLog"
    :highlight-key="highlightKey"
    :loading="loading"
    @exemplar="(e) => emit('exemplar', e)"
    @zoom="onZoom"
    @legend-toggle="(e) => emit('legend-toggle', e)"
    @point-click="onPointClick"
  />
</template>

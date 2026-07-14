<script setup>
// Public bar / stacked / histogram chart. A thin adapter over BaseChart: it curries the pure
// `buildBarOptions` builder, derives a legend only when there is more than one segment (a plain
// single-segment histogram needs none), passes percentile/threshold markers through, and
// translates BaseChart's generic `select {minX,maxX}` into either `zoom` (time axis) or `brush`
// (a duration/value axis, e.g. the latency histogram).
//
// `xUnit` picks the x semantics (mirrors buildBarOptions): 'time' → bucket.t is epoch-ms
// (converted to seconds, clock-labelled) and a select is a wall-clock `zoom {startMs,endMs}`;
// 'value' → bucket.t is a raw numeric axis (e.g. ns latency) used as-is and a select is a
// `brush {minNs,maxNs}` in those raw units.
import { computed } from 'vue'
import { formatNumber } from '@/lib/core/format'
import { seriesColor } from '@/lib/core/seriesColor'
import { buildBarOptions, msToSec } from './chartOptions.js'
import BaseChart from './BaseChart.vue'

// A no-op uPlot stand-in so we can run buildBarOptions purely to extract its `tooltipData` (the raw
// de-stacked segment values) without a real engine — the builder only touches `uPlot.paths.*`.
const PROBE_UPLOT = { paths: { spline: () => null, bars: () => null } }

const props = defineProps({
  // [{ t: <ms|raw>, segments: [{ key, label, color, value }] }]
  buckets: { type: Array, default: () => [] },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  stacked: { type: Boolean, default: true }, // stack segments (single-segment = plain bars)
  // [{ x, label, color }] — x in the SAME domain as bucket.t (epoch-ms for time, raw for value).
  markers: { type: Array, default: () => [] },
  formatValue: { type: Function, default: formatNumber },
  loading: { type: Boolean, default: false },
  xUnit: { type: String, default: 'time' }, // 'time' | 'value'
  xFormat: { type: Function, default: null }, // duration/value axis + tooltip-header formatter
  xLog: { type: Boolean, default: false }, // value axis only: base-10 log x-scale (long-tailed latency)
})

const emit = defineEmits(['zoom', 'brush', 'legend-toggle'])

function builderArgs(U, theme) {
  return {
    uPlot: U,
    buckets: props.buckets,
    startMs: props.startMs,
    endMs: props.endMs,
    stacked: props.stacked,
    formatValue: props.formatValue,
    theme,
    xUnit: props.xUnit,
    xFormat: props.xFormat,
    xLog: props.xLog,
  }
}

function buildOptions(U, theme) {
  return buildBarOptions(builderArgs(U, theme))
}

// Raw de-stacked segment values for BaseChart's tooltip (so a stacked segment reads its own value,
// not its cumulative top). BaseChart only reads back { opts, data } from buildOptions, so we run the
// pure builder once more (probe ctor, no canvas) to forward `tooltipData`. `null` when unstacked.
const tooltipData = computed(() => buildBarOptions(builderArgs(PROBE_UPLOT, {})).tooltipData)

// Distinct segment keys in first-seen (bottom→top) order. buildBarOptions draws a stack top-down
// (largest cumulative first so smaller bands paint in front), so for stacked charts the legend is
// reversed to keep chip i ↔ built series i+1 aligned — which also reads as top-to-bottom stack order.
// A single-segment histogram gets no legend.
const legendItems = computed(() => {
  const seen = new Map()
  for (const b of props.buckets || []) {
    for (const seg of b.segments || []) {
      if (!seen.has(seg.key)) {
        seen.set(seg.key, { key: seg.key, label: seg.label ?? seg.key, color: seg.color ?? seriesColor(seg.key).stroke })
      }
    }
  }
  const items = seen.size > 1 ? [...seen.values()] : []
  return props.stacked ? items.reverse() : items
})

// Markers arrive in bucket.t's domain; map into the x-scale's units the way bucket.t is mapped
// (time → seconds, value → raw). Accepts `x` (canonical) or `xMs` (spec alias).
const markersForBase = computed(() =>
  (props.markers || []).map((m) => {
    const x = m.x ?? m.xMs
    return { x: props.xUnit === 'value' ? x : msToSec(x), label: m.label, color: m.color }
  }),
)

// BaseChart select is in x-scale units: time → seconds (→ ms zoom); value → raw ns (→ brush).
function onSelect({ minX, maxX }) {
  if (props.xUnit === 'value') emit('brush', { minNs: minX, maxNs: maxX })
  else emit('zoom', { startMs: minX * 1000, endMs: maxX * 1000 })
}
</script>

<template>
  <BaseChart
    :build-options="buildOptions"
    :format-value="formatValue"
    :format-x="xUnit === 'value' ? xFormat : null"
    :legend-items="legendItems"
    :tooltip-data="tooltipData"
    :markers="markersForBase"
    :loading="loading"
    @select="onSelect"
    @legend-toggle="(e) => emit('legend-toggle', e)"
  />
</template>

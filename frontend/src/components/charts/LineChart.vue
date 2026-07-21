<script setup>
// Public line/area chart. A thin adapter over BaseChart: it curries the pure `buildLineOptions`
// builder with its domain props, derives the legend from `series`, converts band times into the
// x-scale's units, flags isolated single-point runs as dots, and translates BaseChart's generic
// `select {minX,maxX}` (seconds) into the `zoom {startMs,endMs}` (ms) the views consume.
//
// All timestamps cross this boundary as millisecond Numbers; uPlot works in seconds, so the ms→s
// conversion happens here (bands) and in buildLineOptions (series x + window).
import { computed } from 'vue'
import { formatNumber } from '@/lib/core/format'
import { seriesColor } from '@/lib/core/seriesColor'
import { buildLineOptions, isolatedPointIndices, msToSec } from './chartOptions.js'
import BaseChart from './BaseChart.vue'

// A no-op uPlot stand-in so we can run buildLineOptions purely to extract its `tooltipData` (the
// raw de-stacked values) without a real engine — the builder only touches `uPlot.paths.*`.
const PROBE_UPLOT = { paths: { spline: () => null, bars: () => null } }

const props = defineProps({
  // [{ key, label, points: [{ t: <ms>, v: Number|null }], color? }]
  series: { type: Array, default: () => [] },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  formatValue: { type: Function, default: formatNumber },
  area: { type: Boolean, default: false }, // gradient fill (lead series, or every band when stacked)
  stacked: { type: Boolean, default: false }, // accumulate series into cumulative bands (a filled TOTAL)
  refLines: { type: Array, default: () => [] }, // [{ y, label, color, style? }] — y in value units
  bands: { type: Array, default: () => [] }, //   [{ x0Ms, x1Ms, label?, color }]
  highlightKey: { type: String, default: null },
  loading: { type: Boolean, default: false },
  compact: { type: Boolean, default: false }, // mini mode: no axes/legend, tight padding, small height
  height: { type: Number, default: 240 }, //     chart pixel height (forwarded to BaseChart)
  yLog: { type: Boolean, default: false }, //    base-10 log y-scale (wide-dynamic-range metrics)
  yRange: { type: Array, default: null }, //     fixed [min,max] y bounds (e.g. [0,100] for percent charts)
})

// `exemplar` is declared per the contract but inert until backend exemplars ship (metrics).
const emit = defineEmits(['zoom', 'legend-toggle', 'exemplar', 'point-click'])

// Curry the pure builder with our props, then enable point dots at isolated indices per series —
// a 1-vertex run has no segment to draw, so uPlot would render nothing without a point there.
// uPlot's `points.show` accepts a filter fn returning the data indices to mark.
function builderArgs(U, theme) {
  return {
    uPlot: U,
    series: props.series,
    startMs: props.startMs,
    endMs: props.endMs,
    formatValue: props.formatValue,
    area: props.area,
    stacked: props.stacked,
    theme,
    highlightKey: props.highlightKey,
    compact: props.compact,
    yLog: props.yLog,
    yRange: props.yRange,
  }
}

function buildOptions(U, theme) {
  const built = buildLineOptions(builderArgs(U, theme))
  // Isolated single-point dots only make sense for un-stacked raw series (built.data y-series map
  // 1:1 to props.series in natural order). Stacked bands are cumulative and drawn top-down, so the
  // index mapping no longer aligns — skip the decoration there.
  if (!props.stacked) {
    props.series.forEach((s, i) => {
      const iso = new Set(isolatedPointIndices(built.data[i + 1]))
      if (!iso.size) return
      built.opts.series[i + 1].points = {
        ...(built.opts.series[i + 1].points || {}),
        show: (u, si, i0, i1) => {
          const out = []
          for (let k = i0; k <= i1; k++) if (iso.has(k)) out.push(k)
          return out
        },
        size: 6,
      }
    })
  }
  return built
}

// Raw de-stacked per-series values for BaseChart's tooltip. The builder produces them, but BaseChart
// only reads back { opts, data } from buildOptions — so we run the pure builder once more (probe
// ctor, no canvas) to forward its `tooltipData`. `null` for unstacked (BaseChart reads u.data then).
const tooltipData = computed(() => buildLineOptions(builderArgs(PROBE_UPLOT, {})).tooltipData)

// Legend chip per series, in the SAME order the builder emits y-series (chip i ↔ built y-series
// i+1, which BaseChart relies on for tooltip labels + toggle). Colour mirrors the builder's stroke
// choice: explicit `series.color`, else the hashed identity colour. Stacked bands are drawn
// top-down (total first), so the legend is reversed to keep the chip↔series mapping aligned — this
// also reads nicely as the visual top-to-bottom stack order.
const legendItems = computed(() => {
  const items = props.series.map((s) => ({ key: s.key, label: s.label ?? s.key, color: s.color ?? seriesColor(s.key).stroke }))
  // NOTE: toggling a middle stacked band hides its cumulative line and leaves a visual gap in the
  // stack (uPlot has no native stacking to recompute) — accepted limitation for now.
  return props.stacked ? items.reverse() : items
})

// Bands cross in as ms; the line x-scale is in seconds, so convert to match BaseChart's units.
const bandsForBase = computed(() =>
  (props.bands || []).map((b) => ({ x0: msToSec(b.x0Ms), x1: msToSec(b.x1Ms), label: b.label, color: b.color })),
)

// BaseChart select is in x-scale units (seconds for a time axis) → zoom carries ms.
function onSelect({ minX, maxX }) {
  emit('zoom', { startMs: minX * 1000, endMs: maxX * 1000 })
}
</script>

<template>
  <BaseChart
    :build-options="buildOptions"
    :format-value="formatValue"
    :legend-items="legendItems"
    :tooltip-data="tooltipData"
    :ref-lines="refLines"
    :bands="bandsForBase"
    :loading="loading"
    :height="height"
    @select="onSelect"
    @legend-toggle="(e) => emit('legend-toggle', e)"
    @point-click="(e) => emit('point-click', e)"
  />
</template>

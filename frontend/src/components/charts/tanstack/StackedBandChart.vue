<script setup lang="ts">
// A stacked time-bucket bar chart, rendered by TanStack Charts.
//
// It takes the SAME `buckets` shape `charts/BarChart.vue` does — `[{ t: <ms>, segments: [{ key,
// label, color, value }] }]` — so a caller switches engines by changing one import and nothing
// else. That is the point of this file: the port is reversible, and the two engines can be
// compared on a real chart rather than in the abstract.
//
// Why the x scale is `scaleLinear` over epoch milliseconds rather than d3's `scaleUtc`: a linear
// scale over epoch ms IS a time scale — the only thing d3 adds is calendar-aware tick placement
// ("the 1st", "midnight"), and these charts already format their own ticks against a window the
// caller chose. Taking `d3-scale` for tick placement alone would be a dependency for a label.
//
// SVG, not canvas: TanStack Charts' canvas surface has React and Octane entry points, not a Vue
// one. That is fine at Photon's point counts — charts default to 48 buckets
// (`crates/photon-api/src/histogram.rs`) — but it is why the uPlot explorers stay where they are.
import { computed } from 'vue'
import { Chart } from '@tanstack/charts/vue'
import { barY, defineChart, ruleX } from '@tanstack/charts'
import { scaleLinear } from '@tanstack/charts/scales/linear'
import { formatNumber } from '@/lib/core/format'
import { useTanstackChartTheme } from './chartTheme'

export interface BandSegment {
  key: string
  label: string
  color: string
  value: number
}
export interface BandBucket {
  /** Bucket start, epoch milliseconds. */
  t: number
  segments: BandSegment[]
}
export interface ChartMarker {
  xMs: number
  label: string
  color: string
}

const props = withDefaults(
  defineProps<{
    buckets?: BandBucket[]
    startMs: number
    endMs: number
    markers?: ChartMarker[]
    loading?: boolean
    height?: number
    /** Accessible name. Required in TanStack Charts, so it is required here too. */
    ariaLabel?: string
    formatValue?: (n: number) => string
  }>(),
  {
    buckets: () => [],
    markers: () => [],
    loading: false,
    height: 220,
    ariaLabel: 'Stacked time series',
    formatValue: formatNumber,
  },
)

const theme = useTanstackChartTheme()

/** One row per (bucket, segment) — the long shape a grammar-of-graphics mark wants. */
interface Row {
  t: number
  key: string
  label: string
  color: string
  value: number
}

const rows = computed<Row[]>(() =>
  props.buckets.flatMap((b) =>
    (b.segments ?? []).map((s) => ({
      t: Number(b.t),
      key: s.key,
      label: s.label,
      color: s.color,
      value: Number(s.value) || 0,
    })),
  ),
)

const hasData = computed(() => rows.value.some((r) => r.value > 0))

/**
 * The definition is rebuilt whenever the data, the window, the markers or the THEME change.
 * Identity matters here: TanStack Charts re-reads a definition when its reference changes, so a
 * mutated-in-place definition would keep painting the old theme after a light↔dark flip.
 */
const definition = computed(() =>
  defineChart({
    marks: [
      barY(rows.value, {
        x: 't',
        y: 'value',
        // `z` groups the rows into stacks; with implicit stacking that is all a stacked bar needs.
        z: 'key',
        // Colours come off the design tokens per segment, not from a palette scale — the three
        // Apdex bands mean something (good / tolerating / frustrated) and must not be recoloured
        // by an ordinal scale that only knows they are three distinct keys.
        fill: (d: Row) => d.color,
      }),
      // Deploy markers. Rules emit no interaction points, which is exactly right: a marker is an
      // annotation, and it must not steal the hover from the bar underneath it.
      ruleX(props.markers, {
        x: (m: ChartMarker) => m.xMs,
        stroke: (m: ChartMarker) => m.color,
        strokeDasharray: '4 3',
        strokeOpacity: 0.9,
      }),
    ],
    x: {
      // Fixed to the caller's window, not inferred: a chart whose axis stops at the last bucket
      // with data silently rescales when a service goes quiet, which reads as the window changing.
      scale: () => scaleLinear().domain([props.startMs, props.endMs]),
      axis: { ticks: { format: (v: number) => clock(v) } },
    },
    y: {
      scale: scaleLinear,
      nice: true,
      grid: true,
      axis: { ticks: { format: (v: number) => props.formatValue(v) } },
    },
    // The engine's defaults use `currentColor`, which would inherit whatever text colour the
    // panel happens to have. Photon's charts are styled from `tokens.css` like everything else, so
    // the axis, guide and grid colours are handed over explicitly.
    theme: {
      foreground: theme.value.muted,
      muted: theme.value.muted,
      grid: theme.value.grid,
    },
  }),
)

/** Axis labels: clock time for a same-day window, date + time once it spans days. */
function clock(ms: number): string {
  const d = new Date(ms)
  const spansDays = props.endMs - props.startMs > 24 * 3600 * 1000
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  return spansDays ? `${d.getDate()}/${d.getMonth() + 1} ${hh}:${mm}` : `${hh}:${mm}`
}
</script>

<template>
  <div class="relative w-full" :style="{ height: `${height}px` }">
    <Chart
      v-if="hasData"
      data-testid="tanstack-stacked-chart"
      :definition="definition"
      :aria-label="ariaLabel"
      :height="height"
      class="w-full"
    />
    <!-- An empty window says so rather than drawing an axis pair around nothing. -->
    <div
      v-else
      data-testid="tanstack-stacked-empty"
      class="flex h-full items-center justify-center text-xs text-muted-foreground"
    >
      {{ loading ? 'Loading…' : 'No data in range' }}
    </div>
  </div>
</template>

<script setup>
// Response-time chart for the monitor detail dialog. Thin adapter over charts/LineChart: a single
// 'latency' series (down beats → v:null so the line breaks, never dives to zero), a translucent
// outage band per contiguous down run, and dashed avg/p95 reference lines. The min/avg/p95/max stat
// header + legend are this component's own markup (not chart-canvas content) around <LineChart>.
// B&W ink everywhere; red/amber (--sev-error/--sev-warn) are reserved for outage/p95 (token policy).
import { computed } from 'vue'
import { Card } from '@/components/ui/card'
import { EmptyState } from '@/components/ui/empty-state'
import LineChart from '@/components/charts/LineChart.vue'
import { useChartTheme } from '@/components/charts/useChartTheme.js'
import { formatNumber } from '@/lib/core/format'

const props = defineProps({
  heartbeats: { type: Array, default: () => [] },
  height: { type: Number, default: 200 },
})

// `theme.text` feeds the series stroke: uPlot paints to a <canvas>, which (unlike the DOM overlays
// below) can't resolve a raw `var(--foo)` string, so the concrete colour needs the same
// getComputedStyle resolution BaseChart itself uses for axis/grid ink.
const { theme } = useChartTheme()

const heartbeats = computed(() => props.heartbeats ?? [])
const okLats = computed(() => heartbeats.value.filter((h) => h.ok).map((h) => h.latency_ms))
const hasData = computed(() => okLats.value.length > 0)

const stats = computed(() => {
  const ys = okLats.value
  if (!ys.length) return null
  const sorted = [...ys].sort((a, b) => a - b)
  const sum = ys.reduce((a, b) => a + b, 0)
  const p95i = Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))
  return {
    min: sorted[0],
    avg: Math.round(sum / ys.length),
    p95: sorted[p95i],
    max: sorted[sorted.length - 1],
  }
})

// Single latency series; a down beat maps to v:null so LineChart breaks the line there instead of
// diving to zero. Colour is the resolved --foreground ink (not the hashed per-key identity colour
// LineChart defaults to) — this chart is monochrome by design.
const lineSeries = computed(() => [
  {
    key: 'latency',
    label: 'Latency',
    color: theme.value.text,
    points: heartbeats.value.map((h) => ({ t: h.ts, v: h.ok ? h.latency_ms : null })),
  },
])

const startMs = computed(() => heartbeats.value[0]?.ts ?? 0)
const endMs = computed(() => heartbeats.value[heartbeats.value.length - 1]?.ts ?? 1)

// Contiguous down runs → one band each, edges extended halfway to the neighboring beat (so the
// band visually covers the full gap around the down run, matching the old SVG's index-padding).
const bands = computed(() => {
  const hbs = heartbeats.value
  const runs = []
  let start = null
  hbs.forEach((h, i) => {
    if (!h.ok) {
      if (start == null) start = i
    } else if (start != null) {
      runs.push([start, i - 1])
      start = null
    }
  })
  if (start != null) runs.push([start, hbs.length - 1])
  return runs.map(([a, b]) => ({
    x0Ms: hbs[a].ts - (a > 0 ? (hbs[a].ts - hbs[a - 1].ts) / 2 : 0),
    x1Ms: hbs[b].ts + (b < hbs.length - 1 ? (hbs[b + 1].ts - hbs[b].ts) / 2 : 0),
    label: 'outage',
    // BaseChart paints bands as plain DOM rects (not canvas), so a raw CSS var stays theme-reactive
    // with no JS resolution needed here.
    color: 'hsl(var(--sev-error))',
  }))
})

// avg (muted ink) + p95 (amber/--sev-warn) reference lines over the up-beat latencies.
const refLines = computed(() => {
  if (!stats.value) return []
  return [
    { y: stats.value.avg, label: 'avg', color: theme.value.muted },
    { y: stats.value.p95, label: 'p95', color: 'hsl(var(--sev-warn))' },
  ]
})

const formatMs = (v) => `${Math.round(v)} ms`
</script>

<template>
  <Card class="p-4">
    <!-- header: title + min/avg/p95/max chips -->
    <div class="mb-3 flex items-baseline justify-between gap-3">
      <h3 class="text-sm font-medium text-foreground">Response time</h3>
      <div v-if="stats" class="flex gap-4">
        <div v-for="s in ['min', 'avg', 'p95', 'max']" :key="s" class="text-right leading-tight">
          <div class="text-[9.5px] font-semibold uppercase tracking-wide text-muted-foreground">
            {{ s }}
          </div>
          <div class="text-[13px] font-semibold tabular-nums text-foreground">
            {{ formatNumber(stats[s]) }}<span class="text-[10px] font-normal text-muted-foreground">
              ms</span>
          </div>
        </div>
      </div>
    </div>

    <!-- empty state -->
    <EmptyState v-if="!hasData" title="no data yet" class="h-[120px] py-0" />

    <template v-else>
      <div :style="{ height: height + 'px' }">
        <LineChart
          :series="lineSeries"
          :start-ms="startMs"
          :end-ms="endMs"
          :format-value="formatMs"
          area
          :bands="bands"
          :ref-lines="refLines"
        />
      </div>

      <!-- legend -->
      <div class="mt-2.5 flex flex-wrap gap-3.5 text-[11px] text-muted-foreground">
        <span class="inline-flex items-center gap-1.5">
          <svg width="18" height="8" aria-hidden="true">
            <line x1="0" y1="4" x2="18" y2="4" stroke="hsl(var(--foreground))" stroke-width="2" />
          </svg>
          latency
        </span>
        <span class="inline-flex items-center gap-1.5">
          <svg width="18" height="8" aria-hidden="true">
            <line
              x1="0"
              y1="4"
              x2="18"
              y2="4"
              stroke="hsl(var(--foreground) / 0.45)"
              stroke-width="1"
              stroke-dasharray="4 3"
            />
          </svg>
          average
        </span>
        <span class="inline-flex items-center gap-1.5">
          <span
            class="inline-block h-2.5 w-3 rounded-sm"
            style="background: hsl(var(--sev-error) / 0.18)"
          />
          outage
        </span>
      </div>
    </template>
  </Card>
</template>

<script setup>
import { computed } from 'vue'
import BarChart from '@/components/charts/BarChart.vue'
import { useChartTheme } from '@/components/charts/useChartTheme.js'
import { flattenHsl } from '@/lib/core/color'
import { formatNumber } from '@/lib/core/format'
import { useHistogram } from '@/lib/logs/logsQueries'

// Self-contained severity-volume histogram: fetches its own buckets (folds in the former
// ServerHistogram loader) and renders through the shared charts/BarChart.vue. Container-style
// props (query + window + bucket count) mirror the traces-side SpanVolumeHistogram; this
// component only owns the fetch + the domain mapping from severity buckets to BarChart segments
// — layout, tooltip, legend, drag-to-zoom and empty/loading chrome all live in BarChart/BaseChart.
const props = defineProps({
  query: { type: String, default: '' },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  buckets: { type: Number, default: 48 },
})
const emit = defineEmits(['zoom'])

// vue-query loader (reacts to query/window/bucket-count prop changes). A bad query surfaces via
// the search box; on error `.data` stays undefined and the chart just clears (the `?? []`
// fallback), matching the old ServerHistogram catch-and-clear behaviour. `useHistogram` already
// sets `placeholderData: keepPreviousData`, so `data` (and therefore the bars BarChart draws)
// keeps the previous page's bars in view while a refetch is in flight instead of blanking.
const NS = 1_000_000n
const toNs = (ms) => (BigInt(Math.round(ms)) * NS).toString()
const startNs = computed(() => toNs(props.startMs))
const endNs = computed(() => toNs(props.endMs))
const queryRef = computed(() => props.query)
const bucketsRef = computed(() => props.buckets)
const histogramQuery = useHistogram(queryRef, startNs, endNs, bucketsRef)
const data = computed(() => histogramQuery.data.value ?? [])

// Severity stack, low→high — BarChart stacks segments bottom→top in array order, so this order
// IS the visual stack order. Colour policy follows the app-wide B&W theme (see lib/format.js):
// debug/info are neutral greys (two distinct shades so they still read apart), and only
// warn/error/fatal carry colour, so the coloured segments are the signal.
const STACK = [
  { key: 'debug', label: 'Debug' },
  { key: 'info', label: 'Info' },
  { key: 'warn', label: 'Warn' },
  { key: 'error', label: 'Error' },
  { key: 'fatal', label: 'Fatal' },
]

// Resolve the severity segment colours from the CSS tokens (styles/tokens.css) into concrete
// `hsl(...)` strings — BarChart's uPlot canvas paints from plain colour strings, not Tailwind
// classes. Kept inline rather than a shared helper: this is the only place that needs this exact
// severity palette (mirrors the resolve-and-react-to-`version` pattern in useChartTheme.js, which
// covers the generic chart chrome colours but not severity tones).
const { version } = useChartTheme()
const SEVERITY_TOKEN_FALLBACK = {
  '--muted-foreground': '0 0% 45.1%',
  '--card': '0 0% 100%',
  '--sev-warn': '32 81% 35%',
  '--sev-error': '0 72% 51%',
  '--sev-fatal': '262 83% 58%',
}
function resolveToken(name) {
  if (typeof document === 'undefined' || typeof getComputedStyle !== 'function') {
    return SEVERITY_TOKEN_FALLBACK[name]
  }
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name)
  return (raw || '').trim() || SEVERITY_TOKEN_FALLBACK[name]
}
const severityColor = computed(() => {
  void version.value // depend on the theme version so a light↔dark toggle re-resolves these
  const muted = resolveToken('--muted-foreground')
  return {
    // `debug` was `hsl(muted / 0.35)`, but on the canvas a stacked segment paints over the opaque
    // `info` grey behind it — a translucent grey just tints it and the band vanishes. Flatten to the
    // OPAQUE colour 35% muted-fg would show on the card surface so `debug` stays a distinct faint
    // grey (see lib/color.js). Colour still reserved for warn/error/fatal.
    debug: flattenHsl(muted, resolveToken('--card'), 0.35), // faintest — least signal
    info: `hsl(${muted})`,
    warn: `hsl(${resolveToken('--sev-warn')})`,
    error: `hsl(${resolveToken('--sev-error')})`,
    fatal: `hsl(${resolveToken('--sev-fatal')})`,
  }
})

// Map fetched severity buckets `{ t, debug, info, warn, error, fatal, total }` → BarChart's
// `{ t, segments:[{key,label,color,value}] }`. `t` arrives as an epoch-NANOSECOND string (like
// every other timestamp in this app); BarChart's time mode wants epoch-ms Numbers.
const chartBuckets = computed(() =>
  data.value.map((b) => ({
    t: Number(BigInt(b.t) / NS),
    segments: STACK.map((s) => ({
      key: s.key,
      label: s.label,
      color: severityColor.value[s.key],
      value: Number(b?.[s.key]) || 0,
    })),
  })),
)
</script>

<template>
  <div class="w-full" role="group" aria-label="Log volume histogram">
    <BarChart
      :buckets="chartBuckets"
      :start-ms="startMs"
      :end-ms="endMs"
      :stacked="true"
      :format-value="formatNumber"
      :loading="histogramQuery.isPending.value"
      @zoom="(e) => emit('zoom', e)"
    />
  </div>
</template>

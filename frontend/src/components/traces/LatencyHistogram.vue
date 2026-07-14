<script setup>
// Span duration-distribution histogram: a thin adapter over charts/BarChart. Unlike every other
// chart in the app, its x-axis is a DURATION (bucket_ns), not wall-clock — so it renders in
// BarChart's `xUnit:'value'` mode (bucket.t used as-is, axis/tooltip labelled via `formatDuration`)
// with p50/p90/p99 percentile markers overlaid. It runs that value axis in LOG mode (`x-log`):
// latency is long-tailed and the backend now emits GEOMETRIC buckets (each an equal ratio wider), so
// a log x-axis renders them evenly spaced — the fast bulk spreads out and one slow outlier is a
// single small bar at the right, not an empty tail stretching the whole axis. BarChart owns brushing
// in that mode and emits
// `brush {minNs,maxNs}` in the same raw-ns domain as the buckets — we just re-emit it; the parent
// turns it into a removable `duration>=A duration<=B` query pill.
import { computed } from 'vue'
import BarChart from '@/components/charts/BarChart.vue'
import { useChartTheme } from '@/components/charts/useChartTheme.js'
import { formatDuration, formatNumber } from '@/lib/core/format'
import { useTracesLatency } from '@/lib/traces/tracesQueries'

const props = defineProps({
  query: { type: String, default: '' },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  buckets: { type: Number, default: 48 },
})
const emit = defineEmits(['brush'])

const NS = 1_000_000n
const toNs = (ms) => (BigInt(Math.round(ms)) * NS).toString()
const startNs = computed(() => toNs(props.startMs))
const endNs = computed(() => toNs(props.endMs))
const queryRef = computed(() => props.query)
const bucketsRef = computed(() => props.buckets)

// A bad query surfaces via the search box; on error `.data` stays undefined and the chart just
// clears (the `?? null` fallback below), matching the old catch-and-clear behaviour.
// data shape: { buckets: [{bucket_ns, count}], p50, p90, p99 } | null
const latencyQuery = useTracesLatency(queryRef, startNs, endNs, bucketsRef)
const data = computed(() => latencyQuery.data.value ?? null)
const rawBuckets = computed(() => data.value?.buckets ?? [])

// Canvas can't read Tailwind classes, so marker/bar colours are resolved straight off the CSS
// custom properties (mirrors TraceMinimap/useChartTheme's `hsl(<token triplet>)` pattern, inlined
// here rather than shared since this is the only spot in the adapter that needs it). Fallback
// triplets mirror tokens.css's light palette for the (rare) empty-token case. `version` bumps on
// every light↔dark flip so the colours re-resolve.
const FALLBACK = {
  '--muted-foreground': '0 0% 45.1%',
  '--sev-warn': '32 81% 35%',
  '--sev-error': '0 72% 51%',
}
function cssColor(token) {
  if (typeof document === 'undefined' || typeof getComputedStyle !== 'function') {
    return `hsl(${FALLBACK[token]})`
  }
  const raw = getComputedStyle(document.documentElement).getPropertyValue(token).trim() || FALLBACK[token]
  return `hsl(${raw})`
}
const { version } = useChartTheme()
const themeColors = computed(() => {
  void version.value // establish the reactive dependency; re-resolve on every theme flip
  return {
    bar: cssColor('--muted-foreground'),
    warn: cssColor('--sev-warn'),
    error: cssColor('--sev-error'),
  }
})

// Single-tone bars: one 'count' segment per bucket (no severity/status breakdown for a duration
// histogram), `t` as the raw ns x-value — BarChart's `xUnit:'value'` mode uses it as-is (no
// ms→sec conversion).
const barBuckets = computed(() =>
  rawBuckets.value.map((b) => ({
    t: Number(b.bucket_ns),
    segments: [{ key: 'count', label: 'Count', color: themeColors.value.bar, value: b.count }],
  })),
)

// p50/p90/p99 markers, in the same ns domain as bucket.t. p50 is a fixed sky/blue (it carries no
// severity meaning of its own); p90/p99 borrow the warn/error severity tones so escalating
// percentiles read like escalating severity — the same visual language as the volume histograms.
const markers = computed(() => {
  if (!data.value || rawBuckets.value.length === 0) return []
  return [
    { x: Number(data.value.p50), label: 'p50', color: '#0ea5e9' },
    { x: Number(data.value.p90), label: 'p90', color: themeColors.value.warn },
    { x: Number(data.value.p99), label: 'p99', color: themeColors.value.error },
  ]
})

// BarChart's value-mode select already comes back as {minNs,maxNs} in the raw ns x-domain — just
// pass it through, reshaped defensively so the emitted payload's shape never drifts.
function onBrush({ minNs, maxNs }) {
  emit('brush', { minNs, maxNs })
}
</script>

<template>
  <BarChart
    :buckets="barBuckets"
    :start-ms="startMs"
    :end-ms="endMs"
    x-unit="value"
    x-log
    :x-format="formatDuration"
    :format-value="formatNumber"
    :markers="markers"
    :loading="latencyQuery.isPending.value"
    @brush="onBrush"
  />
</template>

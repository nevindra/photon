<script setup>
// Status-stacked span-volume histogram. A thin adapter over charts/BarChart.vue — it still owns
// its own fetch (`useTracesHistogram`, unchanged) and the status→BarChart-segment mapping;
// rendering, tooltip, legend and drag-to-zoom now all live in BarChart (mirrors the equivalent
// logs-side VolumeHistogram adapter and metrics/MetricChart's technique for LineChart).
import { computed } from 'vue'
import BarChart from '@/components/charts/BarChart.vue'
import { useChartTheme } from '@/components/charts/useChartTheme.js'
import { flattenHsl } from '@/lib/core/color'
import { formatNumber } from '@/lib/core/format'
import { useTracesHistogram } from '@/lib/traces/tracesQueries'

const props = defineProps({
  query: { type: String, default: '' },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
  buckets: { type: Number, default: 48 },
})
const emit = defineEmits(['zoom'])

// Status stack, low-signal→high-signal — first-seen order in each bucket's `segments` array IS
// BarChart's bottom→top stacking order (see chartOptions.js's orderedSegmentKeys), matching the
// old flex-col-reverse layering. Only `error` carries colour (always red); `unset`/`ok` are two
// neutral shades of --muted-foreground so they still read apart — colour stays reserved for signal.
const STACK = [
  { key: 'unset', label: 'Unset' },
  { key: 'ok', label: 'Ok' },
  { key: 'error', label: 'Error' },
]

// BarChart paints on a <canvas>, which can't read Tailwind classes — resolve the same tokens the
// old bg-muted-foreground/* + bg-sev-error classes read into concrete colour strings via
// getComputedStyle, keyed on useChartTheme's `version` so a light↔dark flip re-resolves them.
const { version } = useChartTheme()
const statusColors = computed(() => {
  void version.value
  const cs =
    typeof document !== 'undefined' && typeof getComputedStyle === 'function'
      ? getComputedStyle(document.documentElement)
      : null
  const raw = (name, fallback) => (cs?.getPropertyValue(name) || '').trim() || fallback
  // Fallbacks == tokens.css's light-theme values (jsdom returns '' for unloaded custom props).
  const mutedFg = raw('--muted-foreground', '0 0% 45.1%')
  const sevError = raw('--sev-error', '0 72% 51%')
  const card = raw('--card', '0 0% 100%')
  return {
    // `unset` was `hsl(mutedFg / 0.35)`, but a stacked bar draws each segment over the one behind it
    // on the canvas — a translucent grey over the opaque `ok` grey just tints it and the band
    // disappears. Flatten it to the OPAQUE colour that 35% muted-fg would show on the card surface so
    // the `unset` band reads as a genuinely lighter/dimmer grey (see lib/color.js).
    unset: flattenHsl(mutedFg, card, 0.35),
    ok: `hsl(${mutedFg})`,
    error: `hsl(${sevError})`,
  }
})

const NS = 1_000_000n
const toNs = (ms) => (BigInt(Math.round(ms)) * NS).toString()
const startNs = computed(() => toNs(props.startMs))
const endNs = computed(() => toNs(props.endMs))
const queryRef = computed(() => props.query)
const bucketsRef = computed(() => props.buckets)

// A bad query surfaces via the search box; on error `.data` stays undefined and the chart just
// clears (the `?? []` fallback below), matching the old catch-and-clear behaviour.
const histogramQuery = useTracesHistogram(queryRef, startNs, endNs, bucketsRef)
const data = computed(() => histogramQuery.data.value ?? [])

// { t: <ns string>, ok, error, unset, total } → BarChart's { t: <ms Number>, segments }.
const chartBuckets = computed(() =>
  data.value.map((b) => ({
    t: Number(BigInt(b?.t ?? 0) / NS),
    segments: STACK.map((lvl) => ({
      key: lvl.key,
      label: lvl.label,
      color: statusColors.value[lvl.key],
      value: Number(b?.[lvl.key]) || 0,
    })),
  })),
)
</script>

<template>
  <BarChart
    :buckets="chartBuckets"
    :start-ms="startMs"
    :end-ms="endMs"
    :stacked="true"
    :format-value="formatNumber"
    :loading="histogramQuery.isPending.value"
    @zoom="(e) => emit('zoom', e)"
  />
</template>

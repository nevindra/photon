<script setup>
import { computed, ref } from 'vue'
import HoverTooltip from '@/components/charts/HoverTooltip.vue'

// Option B heartbeat bar: bar HEIGHT encodes latency, bar COLOR encodes status.
// A down beat is a full-height red spike; a slow-but-up beat turns amber. Newest on the right.
const props = defineProps({
  heartbeats: { type: Array, default: () => [] },
  max: { type: Number, default: 40 },
  // strip container height in px per size
  size: { type: String, default: 'md' }, // 'sm' | 'md' | 'lg'
  slowFactor: { type: Number, default: 2.2 },
  showLegend: { type: Boolean, default: false },
})

const HEIGHTS = { sm: 20, md: 28, lg: 44 }
const H = computed(() => HEIGHTS[props.size] ?? HEIGHTS.md)

const beats = computed(() => props.heartbeats.slice(-props.max))

// Latency scale + slow threshold derived from healthy (up) beats only.
const upLats = computed(() =>
  beats.value.filter((b) => b.ok === true && b.latency_ms > 0).map((b) => b.latency_ms),
)
const maxLat = computed(() => Math.max(...upLats.value, 1))
const median = computed(() => {
  const xs = [...upLats.value].sort((a, b) => a - b)
  if (!xs.length) return null
  const mid = Math.floor(xs.length / 2)
  return xs.length % 2 ? xs[mid] : (xs[mid - 1] + xs[mid]) / 2
})
const slowThreshold = computed(() =>
  median.value != null ? median.value * props.slowFactor : Infinity,
)

// Keep the color classes LITERAL below so Tailwind keeps them in the build. `sev-warn`/`sev-error`,
// `success`, and `muted-foreground` are token-driven (tokens.css custom props via tailwind.config.js's
// `sev.*`/`success` maps) so they already track light/dark.
const LEGEND = [
  { cls: 'bg-success', label: 'Up' },
  { cls: 'bg-sev-warn', label: 'Slow' },
  { cls: 'bg-sev-error', label: 'Down' },
  { cls: 'bg-muted-foreground/25', label: 'No data' },
]

function isSlow(b) {
  return b.ok === true && b.latency_ms >= slowThreshold.value
}
function statusLabel(b) {
  if (b.ok == null) return 'no data'
  if (b.ok === false) return 'down'
  return isSlow(b) ? 'slow' : 'up'
}
function heightPx(b) {
  let frac
  if (b.ok == null) frac = 0.1 // no data — a low stub
  else if (b.ok === false) frac = 1 // down — full-height spike
  else frac = 0.16 + 0.84 * (b.latency_ms / maxLat.value) // 0.16 floor so fast beats still read
  return frac * H.value
}
function colorClass(b) {
  if (b.ok == null) return 'bg-muted-foreground/25'
  if (b.ok === false) return 'bg-sev-error'
  return isSlow(b) ? 'bg-sev-warn' : 'bg-success'
}
function fmtTime(ts) {
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

// One tooltip for the whole bar — this component owns the hover state. Content mirrors the
// ChartTooltipCard convention used by the histogram bars (VolumeHistogram/SpanVolumeHistogram):
// muted header = time, prominent line = the status reading, one breakdown row whose swatch
// matches the hovered bar's own colour so the tooltip visually confirms what you're pointing at.
const hovered = ref({ visible: false, x: 0, y: 0, title: '', subtitle: '', rows: [] })
function onMove(e, b) {
  const label = statusLabel(b)
  hovered.value = {
    visible: true,
    x: e.clientX,
    y: e.clientY,
    subtitle: fmtTime(b.ts),
    title: label.charAt(0).toUpperCase() + label.slice(1),
    rows: [
      {
        key: 'latency',
        label: 'Latency',
        value: b.ok === true ? `${b.latency_ms} ms` : '—',
        swatchClass: colorClass(b),
      },
    ],
  }
}
function onLeave() {
  hovered.value.visible = false
}
</script>

<template>
  <div>
    <div
      v-if="beats.length"
      class="flex items-end gap-0.5"
      :style="{ height: H + 'px' }"
      @mouseleave="onLeave"
    >
      <div
        v-for="(b, i) in beats"
        :key="i"
        data-tick
        class="min-w-[2px] flex-1 rounded-t-sm transition-opacity hover:opacity-90"
        :class="[colorClass(b), i === beats.length - 1 ? 'ring-1 ring-foreground/20' : '']"
        :style="{ height: heightPx(b) + 'px' }"
        @mousemove="onMove($event, b)"
      />
    </div>
    <span v-else class="text-xs text-muted-foreground">no checks yet</span>

    <div
      v-if="showLegend && beats.length"
      class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1"
    >
      <span
        v-for="item in LEGEND"
        :key="item.label"
        class="flex items-center gap-1.5 text-[11px] text-muted-foreground"
      >
        <span class="h-2.5 w-2.5 rounded-[3px]" :class="item.cls" />
        {{ item.label }}
      </span>
    </div>

    <HoverTooltip
      :visible="hovered.visible"
      :x="hovered.x"
      :y="hovered.y"
      :title="hovered.title"
      :subtitle="hovered.subtitle"
      :rows="hovered.rows"
    />
  </div>
</template>

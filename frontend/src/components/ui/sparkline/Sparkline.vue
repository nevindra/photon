<script setup>
// Minimal inline-SVG sparkline: a single polyline over a numeric series with a dot on the last
// point. Pure/presentational — colour is passed in (default currentColor). Used by the services
// "needs attention" cards.
import { computed } from 'vue'

const props = defineProps({
  points: { type: Array, default: () => [] },
  width: { type: Number, default: 96 },
  height: { type: Number, default: 26 },
  color: { type: String, default: 'currentColor' },
  strokeWidth: { type: Number, default: 1.5 },
})

const geom = computed(() => {
  const pts = props.points.filter((v) => v != null && Number.isFinite(Number(v))).map(Number)
  if (pts.length < 2) return null
  const pad = props.strokeWidth + 1
  const min = Math.min(...pts)
  const max = Math.max(...pts)
  const range = max - min || 1
  const x = (i) => pad + (i * (props.width - 2 * pad)) / (pts.length - 1)
  const y = (v) => props.height - pad - ((v - min) / range) * (props.height - 2 * pad)
  const d = pts.map((v, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)} ${y(v).toFixed(1)}`).join(' ')
  return { d, cx: x(pts.length - 1).toFixed(1), cy: y(pts[pts.length - 1]).toFixed(1) }
})
</script>

<template>
  <svg v-if="geom" :width="width" :height="height" :viewBox="`0 0 ${width} ${height}`" fill="none" aria-hidden="true">
    <path :d="geom.d" :stroke="color" :stroke-width="strokeWidth" stroke-linejoin="round" stroke-linecap="round" />
    <circle :cx="geom.cx" :cy="geom.cy" :r="strokeWidth + 0.3" :fill="color" />
  </svg>
  <span v-else class="text-[10px] text-muted-foreground/60">—</span>
</template>

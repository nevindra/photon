<script setup lang="ts">
// A table-row sparkline over `[ms, value]` points.
//
// It draws an area under the line, not a bare stroke: at 22px tall a 1px line reads as noise, and
// the point of a row sparkline is the SHAPE — a step, a spike, a cliff — not the values.
import { computed } from 'vue'
import type { SparkPoint } from '@/lib/core/api'

const props = withDefaults(
  defineProps<{
    points?: SparkPoint[]
    width?: number
    height?: number
    tone?: 'neutral' | 'error' | 'warning' | 'brand'
  }>(),
  { points: () => [], width: 110, height: 22, tone: 'brand' },
)

const TONE = {
  neutral: 'text-muted-foreground',
  error: 'text-sev-error',
  warning: 'text-sev-warn',
  brand: 'text-primary',
}

const geom = computed(() => {
  const vals = props.points.map((p) => Number(p?.[1])).filter((v) => Number.isFinite(v))
  if (vals.length < 2) return null
  const pad = 1.5
  // Baseline at ZERO, not at the series minimum: a series sitting between 900 and 1000 must not
  // look like it swings from empty to full. Rate charts are read against zero or they lie.
  const range = Math.max(...vals) || 1
  const x = (i: number) => pad + (i * (props.width - 2 * pad)) / (vals.length - 1)
  const y = (v: number) => props.height - pad - (v / range) * (props.height - 2 * pad)
  const line = vals.map((v, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)} ${y(v).toFixed(1)}`).join(' ')
  const area = `${line} L${x(vals.length - 1).toFixed(1)} ${props.height} L${x(0).toFixed(1)} ${props.height} Z`
  return { line, area, cx: x(vals.length - 1).toFixed(1), cy: y(vals[vals.length - 1]).toFixed(1) }
})
</script>

<template>
  <svg
    v-if="geom"
    :width="width"
    :height="height"
    :viewBox="`0 0 ${width} ${height}`"
    fill="none"
    aria-hidden="true"
    :class="TONE[tone]"
    data-testid="row-sparkline"
  >
    <path :d="geom.area" fill="currentColor" fill-opacity="0.12" />
    <path :d="geom.line" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round" stroke-linecap="round" />
    <circle :cx="geom.cx" :cy="geom.cy" r="1.6" fill="currentColor" />
  </svg>
  <span v-else class="text-[10px] text-muted-foreground/50">—</span>
</template>

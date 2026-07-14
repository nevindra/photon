<script setup>
// A single Web Vital scorecard: label + formatted p75 + a rating pill + the stacked
// good/needs/poor distribution bar + a "Good <= X" threshold hint. Card chrome mirrors
// `ui/stat-tile/StatTile.vue` (rounded-xl border border-border bg-card p-4); the rating
// pill's tone classes mirror `services/ApdexBadge.vue` (soft background + coloured text,
// literal Tailwind strings — never interpolated).
import { computed } from 'vue'
import VitalsDistributionBar from './VitalsDistributionBar.vue'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  metric: { type: String, required: true }, // e.g. "web_vitals.lcp"
  label: { type: String, required: true },
  p75: { type: Number, default: null },
  unit: { type: String, default: 'ms' },
  rating: { type: String, default: null }, // 'good' | 'needs' | 'poor'
  goodMax: { type: Number, default: null },
  poorMin: { type: Number, default: null },
  dist: { type: Object, default: () => ({ good: 0, needs: 0, poor: 0, total: 0 }) },
})

const TONE = {
  good: { text: 'text-success', soft: 'bg-success-soft', label: 'Good' },
  needs: { text: 'text-sev-warn', soft: 'bg-sev-warn-soft', label: 'Needs improvement' },
  poor: { text: 'text-sev-error', soft: 'bg-sev-error-soft', label: 'Poor' },
}

const tone = computed(() => TONE[props.rating] ?? { text: 'text-muted-foreground', soft: 'bg-muted', label: '—' })

// CLS is unitless (2dp); time metrics format ms sensibly — >=1000ms reads as seconds
// (e.g. 2800 -> "2.8s"), otherwise whole milliseconds (e.g. 184 -> "184ms").
function formatVital(metric, value) {
  if (value == null || !Number.isFinite(value)) return '—'
  if (metric === 'web_vitals.cls') return value.toFixed(2)
  if (value >= 1000) return (value / 1000).toFixed(1) + 's'
  return Math.round(value) + 'ms'
}

const formattedP75 = computed(() => formatVital(props.metric, props.p75))
</script>

<template>
  <div class="rounded-xl border border-border bg-card p-4">
    <div class="flex items-center justify-between gap-2">
      <p class="text-xs text-muted-foreground">{{ label }}</p>
      <span
        v-if="rating"
        :data-rating="rating"
        :class="cn('inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-medium', tone.text, tone.soft)"
      >
        {{ tone.label }}
      </span>
    </div>

    <p class="mt-2 text-2xl font-semibold tabular-nums text-foreground">{{ formattedP75 }}</p>

    <VitalsDistributionBar class="mt-3" :good="dist?.good" :needs="dist?.needs" :poor="dist?.poor" />

    <p v-if="goodMax != null" class="mt-2 text-[10px] text-muted-foreground">Good ≤ {{ goodMax }}{{ unit }}</p>
  </div>
</template>

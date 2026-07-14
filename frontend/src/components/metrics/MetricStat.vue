<!-- frontend/src/components/metrics/MetricStat.vue -->
<script setup lang="ts">
import { computed } from 'vue'
import { ArrowUp, ArrowDown } from 'lucide-vue-next'
import { statSummary, type Series } from '@/lib/metrics/metricViz'
import { formatNumber } from '@/lib/core/format'

const props = withDefaults(defineProps<{
  series?: Series[]
  unit?: string
  loading?: boolean
}>(), {
  series: () => [],
  unit: '',
  loading: false,
})

const summary = computed(() => statSummary(props.series))
const heroText = computed(() => (summary.value.hero == null ? '—' : formatNumber(Math.round(summary.value.hero * 100) / 100)))
const unitSuffix = computed(() => (props.unit && props.unit !== '1' ? props.unit : ''))
const deltaText = computed(() => {
  const d = summary.value.deltaPct
  return d == null ? '' : `${d > 0 ? '+' : ''}${d.toFixed(1)}% vs window`
})
// Rising is not inherently good/bad, but the tactile cue reads best as success-up / warn-down.
const deltaClass = computed(() => (summary.value.dir === 'up' ? 'text-success' : summary.value.dir === 'down' ? 'text-sev-warn' : 'text-muted-foreground'))
</script>

<template>
  <div class="flex h-[230px] flex-col items-center justify-center gap-2" data-testid="metric-stat">
    <div v-if="loading" class="text-[13px] text-muted-foreground">Loading…</div>
    <template v-else>
      <div data-testid="stat-hero" class="flex items-baseline gap-1.5 font-mono tabular-nums">
        <span class="text-[48px] font-semibold leading-none tracking-tight text-foreground">{{ heroText }}</span>
        <span v-if="unitSuffix" class="text-[16px] text-muted-foreground">{{ unitSuffix }}</span>
      </div>
      <div v-if="deltaText" data-testid="stat-delta" class="flex items-center gap-1 text-[13px] font-medium" :class="deltaClass">
        <ArrowUp v-if="summary.dir === 'up'" class="size-3.5" />
        <ArrowDown v-else-if="summary.dir === 'down'" class="size-3.5" />
        {{ deltaText }}
      </div>
    </template>
  </div>
</template>

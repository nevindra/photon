<!-- frontend/src/components/metrics/MetricPresets.vue -->
<script setup lang="ts">
import { computed } from 'vue'
import { presetsForType } from '@/lib/metrics/quickStarts'
import { AGG_OPTIONS } from '@/lib/metrics/metricFields'

const props = withDefaults(defineProps<{
  metricType?: string
  isMonotonic?: boolean | null
  currentAgg?: string | null
}>(), {
  metricType: '',
  isMonotonic: null,
  currentAgg: null,
})
const emit = defineEmits<{ apply: [payload: { agg: string }] }>()

const presets = computed(() => presetsForType(props.metricType, props.isMonotonic))
const label = (agg: string): string => (AGG_OPTIONS as Record<string, string>)[agg] || agg
</script>

<template>
  <div v-if="presets.length" data-testid="metric-presets" class="flex flex-wrap items-center gap-1.5">
    <span class="text-[11px] text-muted-foreground">presets</span>
    <button
      v-for="a in presets" :key="a" type="button" data-testid="preset-chip"
      class="rounded-full border px-2 py-0.5 text-[11px] font-mono transition-colors"
      :class="currentAgg === a ? 'border-brand/50 bg-brand/10 text-foreground' : 'border-border text-muted-foreground hover:border-foreground/40 hover:text-foreground'"
      @click="emit('apply', { agg: a })"
    >{{ label(a) }}</button>
  </div>
</template>

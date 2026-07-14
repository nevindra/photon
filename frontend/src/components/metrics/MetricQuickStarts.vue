<!-- frontend/src/components/metrics/MetricQuickStarts.vue -->
<script setup lang="ts">
import { computed } from 'vue'
import { Zap } from 'lucide-vue-next'
import { curatedQuickStarts, type QuickStart } from '@/lib/metrics/quickStarts'
import type { MetricEntry } from '@/lib/metrics/metricNamespaces'

const props = withDefaults(defineProps<{
  catalog?: MetricEntry[]
}>(), {
  catalog: () => [],
})
const emit = defineEmits<{
  apply: [payload: { metric: string; agg: string; group_by?: string[]; viz?: string }]
}>()

const cards = computed<QuickStart[]>(() => curatedQuickStarts(props.catalog))
</script>

<template>
  <div v-if="cards.length" data-testid="metric-quickstarts" class="flex flex-wrap justify-center gap-2.5">
    <button
      v-for="c in cards" :key="c.label" type="button" data-testid="quickstart-card"
      class="flex w-[220px] flex-col gap-1 rounded-xl border border-border bg-card p-3 text-left transition-colors hover:border-foreground/40"
      @click="emit('apply', { metric: c.metric, agg: c.agg, group_by: c.group_by, viz: c.viz })"
    >
      <span class="flex items-center gap-1.5 text-[13px] font-medium text-foreground">
        <Zap class="size-3.5 text-brand" /> {{ c.label }}
      </span>
      <span class="text-[12px] text-muted-foreground">{{ c.description }}</span>
      <span class="mt-0.5 font-mono text-[11px] text-muted-foreground/70">{{ c.metric }} · {{ c.agg }}</span>
    </button>
  </div>
</template>

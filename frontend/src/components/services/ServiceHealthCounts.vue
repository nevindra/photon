<script setup>
import { computed } from 'vue'
import { healthCounts, STATUS_META } from '@/lib/services/serviceHealth'

const props = defineProps({ rows: { type: Array, default: () => [] } })
const counts = computed(() => healthCounts(props.rows))
const ORDER = ['healthy', 'degraded', 'critical', 'idle']
const items = computed(() => ORDER.map((k) => ({ key: k, meta: STATUS_META[k], n: counts.value[k] })))
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-5 gap-y-2 rounded-xl border border-border bg-card px-4 py-3">
    <div v-for="it in items" :key="it.key" class="flex items-center gap-2">
      <span :class="['size-2 rounded-full', it.meta.dot]" />
      <span class="font-mono text-base font-semibold tabular-nums text-foreground">{{ it.n }}</span>
      <span class="text-xs text-muted-foreground">{{ it.meta.label }}</span>
    </div>
  </div>
</template>

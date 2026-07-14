<script setup>
import { computed } from 'vue'
import { STATUS_META } from '@/lib/services/serviceHealth'

const props = defineProps({
  status: { type: String, default: 'idle' },
  reasons: { type: Array, default: () => [] }, // strings, e.g. ['Error rate 8.2%', 'p99 1.2s ▲40%']
})
const meta = computed(() => STATUS_META[props.status] ?? STATUS_META.idle)
// Literal class strings per status (never build dynamically — Tailwind purge).
const TINT = {
  critical: 'border-sev-error/30 from-sev-error-soft',
  degraded: 'border-sev-warn/30 from-sev-warn-soft',
  healthy: 'border-success/30 from-success-soft',
  idle: 'border-border from-transparent',
}
const tint = computed(() => TINT[props.status] ?? TINT.idle)
</script>

<template>
  <div :class="['flex flex-wrap items-center gap-x-4 gap-y-1 rounded-xl border bg-gradient-to-r to-transparent px-4 py-3', tint]">
    <div :class="['flex items-center gap-2 text-sm font-bold tracking-wide', meta.text]">
      <span :class="['size-2.5 rounded-full', meta.dot]" />
      {{ meta.label.toUpperCase() }}
    </div>
    <div v-if="reasons.length" class="font-mono text-xs text-foreground/80">{{ reasons.join(' · ') }}</div>
    <div v-else class="font-mono text-xs text-muted-foreground">No issues in this window.</div>
  </div>
</template>

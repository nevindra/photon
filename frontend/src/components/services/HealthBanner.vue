<script setup>
// The service-detail verdict banner: one Attention Level, stated in words, with the "why" line
// beside it. Same five levels as the pill and the board.
import { computed } from 'vue'
import { STATUS_META } from '@/lib/services/serviceHealth'

const props = defineProps({
  level: { type: String, default: 'unknown' }, // 'down' | 'critical' | 'warning' | 'ok' | 'unknown'
  reasons: { type: Array, default: () => [] }, // strings, e.g. ['Error rate 8.2%', 'p99 1.2s ▲40%']
})
const meta = computed(() => STATUS_META[props.level] ?? STATUS_META.unknown)
// Literal class strings per level (never build dynamically — Tailwind purge). Down takes the same
// error hue as Critical but a heavier edge, matching the solid-vs-soft split on the pill.
const TINT = {
  down: 'border-sev-error/50 from-sev-error-soft',
  critical: 'border-sev-error/30 from-sev-error-soft',
  warning: 'border-sev-warn/30 from-sev-warn-soft',
  ok: 'border-success/30 from-success-soft',
  unknown: 'border-border from-transparent',
}
const tint = computed(() => TINT[props.level] ?? TINT.unknown)
// "No issues" is only true when we actually looked and found none. Anything else — no traffic, a
// stale Site, a service that stopped — has to say it saw nothing rather than claim it saw good.
const emptyLine = computed(() => (props.level === 'ok' ? 'No issues in this window.' : 'No data in this window.'))
</script>

<template>
  <div :class="['flex flex-wrap items-center gap-x-4 gap-y-1 rounded-xl border bg-gradient-to-r to-transparent px-4 py-3', tint]">
    <div :data-level="level" :class="['flex items-center gap-2 text-sm font-bold tracking-wide', meta.text]">
      <span :class="['size-2.5 rounded-full', meta.dot]" />
      {{ meta.label.toUpperCase() }}
    </div>
    <div v-if="reasons.length" class="font-mono text-xs text-foreground/80">{{ reasons.join(' · ') }}</div>
    <div v-else class="font-mono text-xs text-muted-foreground">{{ emptyLine }}</div>
  </div>
</template>

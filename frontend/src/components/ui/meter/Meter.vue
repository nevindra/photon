<script setup lang="ts">
// Horizontal proportion bar: a neutral track with a filled portion at `value` (0–1, clamped).
// `tone` colours the fill using the app's tone vocabulary (only warn/error carry real signal;
// neutral is a muted grey). Literal class strings so Tailwind keeps them.
import type { HTMLAttributes } from 'vue'
import { computed } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  value: number
  tone?: 'neutral' | 'error' | 'warning' | 'success' | 'info'
  class?: HTMLAttributes['class']
}>()

const TONE = {
  neutral: 'bg-muted-foreground/70',
  error: 'bg-sev-error',
  warning: 'bg-sev-warn',
  success: 'bg-success',
  info: 'bg-primary',
}

const pct = computed(() => {
  const v = Number(props.value)
  if (!Number.isFinite(v)) return 0
  return Math.max(0, Math.min(1, v)) * 100
})
const toneClass = computed(() => TONE[props.tone ?? 'neutral'])
</script>

<template>
  <div
    role="meter"
    :aria-valuenow="Math.round(pct)"
    aria-valuemin="0"
    aria-valuemax="100"
    :class="cn('relative h-1.5 w-full overflow-hidden rounded-full bg-muted', props.class)"
  >
    <div class="h-full rounded-full transition-[width]" :class="toneClass" :style="{ width: pct + '%' }" />
  </div>
</template>

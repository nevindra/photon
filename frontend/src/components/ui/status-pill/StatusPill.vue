<script setup lang="ts">
// Soft status pill: a tinted background + the matching tone foreground, held by an inset
// ring so the tint keeps an edge on a white card. Same pill geometry as Badge's soft
// variants (rounded-full, 11px, medium) — StatusPill is the one that leads with a dot.
// `dot` defaults on; turn it off when a leading icon in the slot already carries the state.
import type { HTMLAttributes } from 'vue'
import { computed } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  tone?: 'success' | 'error' | 'warning' | 'info' | 'neutral'
  dot?: boolean
  class?: HTMLAttributes['class']
}>()

const TONE = {
  success: {
    text: 'text-success',
    soft: 'bg-success-soft ring-1 ring-inset ring-success/15',
    dot: 'bg-success',
  },
  error: {
    text: 'text-sev-error',
    soft: 'bg-sev-error-soft ring-1 ring-inset ring-sev-error/15',
    dot: 'bg-sev-error',
  },
  warning: {
    text: 'text-sev-warn',
    soft: 'bg-sev-warn-soft ring-1 ring-inset ring-sev-warn/15',
    dot: 'bg-sev-warn',
  },
  info: {
    text: 'text-foreground',
    soft: 'bg-muted ring-1 ring-inset ring-border',
    dot: 'bg-primary',
  },
  neutral: {
    text: 'text-muted-foreground',
    soft: 'bg-muted ring-1 ring-inset ring-border',
    dot: 'bg-muted-foreground',
  },
}

const toneVal = computed(() => props.tone ?? 'neutral')
const showDot = computed(() => props.dot ?? true)
</script>

<template>
  <span
    :class="cn('inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] font-medium leading-4 [&_svg]:size-3 [&_svg]:shrink-0', TONE[toneVal].text, TONE[toneVal].soft, props.class)"
  >
    <span v-if="showDot" class="size-1.5 shrink-0 rounded-full" :class="TONE[toneVal].dot" />
    <slot />
  </span>
</template>

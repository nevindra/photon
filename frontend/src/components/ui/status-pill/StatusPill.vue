<script setup lang="ts">
// Soft status pill: a tinted background + the matching tone foreground, held by an inset
// ring so the tint keeps an edge on a white card. Same pill geometry as Badge's soft
// variants (rounded-full, 11px, medium) — StatusPill is the one that leads with a dot.
// `dot` defaults on; turn it off when a leading icon in the slot already carries the state.
//
// `solid` swaps the tint for the full-strength tone with reversed text. Spend it on the ONE
// state per screen that must not be mistaken for a milder neighbour (the Attention Board gives
// it to Down, so "not answering" never reads as "a bit slow"). Two solid pills side by side
// cancel each other out.
import type { HTMLAttributes } from 'vue'
import { computed } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  tone?: 'success' | 'error' | 'warning' | 'info' | 'neutral'
  dot?: boolean
  solid?: boolean
  class?: HTMLAttributes['class']
}>()

const TONE = {
  success: {
    text: 'text-success',
    soft: 'bg-success-soft ring-1 ring-inset ring-success/15',
    dot: 'bg-success',
    solid: 'bg-success text-white',
  },
  error: {
    text: 'text-sev-error',
    soft: 'bg-sev-error-soft ring-1 ring-inset ring-sev-error/15',
    dot: 'bg-sev-error',
    solid: 'bg-sev-error text-white',
  },
  warning: {
    text: 'text-sev-warn',
    soft: 'bg-sev-warn-soft ring-1 ring-inset ring-sev-warn/15',
    dot: 'bg-sev-warn',
    solid: 'bg-sev-warn text-white',
  },
  info: {
    text: 'text-foreground',
    soft: 'bg-muted ring-1 ring-inset ring-border',
    dot: 'bg-primary',
    solid: 'bg-primary text-primary-foreground',
  },
  neutral: {
    text: 'text-muted-foreground',
    soft: 'bg-muted ring-1 ring-inset ring-border',
    dot: 'bg-muted-foreground',
    solid: 'bg-muted-foreground text-white',
  },
}

const toneVal = computed(() => props.tone ?? 'neutral')
// The dot exists to carry the tone when the background cannot. A solid pill already is the tone,
// so it drops the dot unless the caller asks for one explicitly.
const showDot = computed(() => props.dot ?? !props.solid)
const skin = computed(() =>
  props.solid ? TONE[toneVal.value].solid : `${TONE[toneVal.value].text} ${TONE[toneVal.value].soft}`,
)
</script>

<template>
  <span
    :class="cn('inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] font-medium leading-4 [&_svg]:size-3 [&_svg]:shrink-0', skin, props.class)"
  >
    <span v-if="showDot" class="size-1.5 shrink-0 rounded-full" :class="props.solid ? 'bg-current opacity-80' : TONE[toneVal].dot" />
    <slot />
  </span>
</template>

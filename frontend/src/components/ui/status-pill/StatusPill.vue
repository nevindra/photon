<script setup lang="ts">
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
    soft: 'bg-success-soft',
    dot: 'bg-success',
  },
  error: {
    text: 'text-sev-error',
    soft: 'bg-sev-error-soft',
    dot: 'bg-sev-error',
  },
  warning: {
    text: 'text-sev-warn',
    soft: 'bg-sev-warn-soft',
    dot: 'bg-sev-warn',
  },
  info: {
    text: 'text-foreground',
    soft: 'bg-muted',
    dot: 'bg-primary',
  },
  neutral: {
    text: 'text-muted-foreground',
    soft: 'bg-muted',
    dot: 'bg-muted-foreground',
  },
}

const toneVal = computed(() => props.tone ?? 'neutral')
const showDot = computed(() => props.dot ?? true)
</script>

<template>
  <span
    :class="cn('inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium', TONE[toneVal].text, TONE[toneVal].soft, props.class)"
  >
    <span v-if="showDot" class="h-1.5 w-1.5 rounded-full" :class="TONE[toneVal].dot" />
    <slot />
  </span>
</template>

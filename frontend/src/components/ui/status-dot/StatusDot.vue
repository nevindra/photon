<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { computed } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  tone?: 'success' | 'error' | 'warning' | 'info' | 'neutral'
  size?: 'sm' | 'md' | 'lg'
  class?: HTMLAttributes['class']
}>()

const TONE = {
  success: { dot: 'bg-success' },
  error: { dot: 'bg-sev-error' },
  warning: { dot: 'bg-sev-warn' },
  info: { dot: 'bg-primary' },
  neutral: { dot: 'bg-muted-foreground' },
}

const sizeMap = {
  sm: 'h-1 w-1',
  md: 'h-1.5 w-1.5',
  lg: 'h-2.5 w-2.5',
}

const toneVal = computed(() => props.tone ?? 'neutral')
const sizeClass = computed(() => sizeMap[props.size ?? 'md'])
</script>

<template>
  <span
    class="shrink-0 rounded-full"
    :class="[TONE[toneVal].dot, sizeClass, props.class]"
  />
</template>

<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { computed } from 'vue'
import { ArrowUp, ArrowDown, Minus } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  label: string
  value: string | number
  mono?: boolean
  accent?: 'success' | 'error' | 'warning' | 'info' | 'neutral'
  delta?: number | string
  deltaDirection?: 'up' | 'down' | 'flat'
  class?: HTMLAttributes['class']
}>()

const TONE = {
  success: { dot: 'bg-success' },
  error: { dot: 'bg-sev-error' },
  warning: { dot: 'bg-sev-warn' },
  info: { dot: 'bg-primary' },
  neutral: { dot: 'bg-muted-foreground' },
}

const DeltaIcon = computed(() => {
  if (props.deltaDirection === 'up') return ArrowUp
  if (props.deltaDirection === 'down') return ArrowDown
  return Minus
})

const deltaColorClass = computed(() => {
  if (props.deltaDirection === 'up') return 'text-success'
  if (props.deltaDirection === 'down') return 'text-sev-error'
  return 'text-muted-foreground'
})
</script>

<template>
  <div
    :class="
      cn(
        'relative overflow-hidden rounded-lg border border-border bg-card p-4 shadow-1 transition-[transform,box-shadow] duration-150 ease-out hover:shadow-2 motion-safe:hover:-translate-y-0.5',
        props.class,
      )
    "
  >
    <div v-if="props.accent" class="absolute inset-y-0 left-0 w-1" :class="TONE[props.accent].dot" />
    <div class="space-y-2">
      <p class="text-xs text-muted-foreground">{{ props.label }}</p>
      <div class="flex items-baseline gap-2">
        <p :class="cn('text-2xl font-semibold tabular-nums text-foreground', props.mono && 'font-mono')">
          {{ props.value }}
        </p>
        <div v-if="props.delta !== undefined" class="flex items-center gap-1">
          <component :is="DeltaIcon" :class="cn('h-3.5 w-3.5', deltaColorClass)" />
          <span :class="cn('text-xs font-medium', deltaColorClass)">{{ props.delta }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

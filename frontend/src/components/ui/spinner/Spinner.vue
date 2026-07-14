<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { Loader2 } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  size?: 'sm' | 'md' | 'lg'
  class?: HTMLAttributes['class']
}>()

defineSlots<{
  default?: () => any
}>()

const sizeClasses = {
  sm: 'h-4 w-4',
  md: 'h-5 w-5',
  lg: 'h-6 w-6',
}

const spinnerClass = sizeClasses[props.size ?? 'md']
</script>

<template>
  <span
    v-if="$slots.default"
    class="inline-flex items-center gap-2"
  >
    <Loader2 :class="cn('animate-spin text-muted-foreground', spinnerClass, props.class)" />
    <slot />
  </span>
  <Loader2
    v-else
    :class="cn('animate-spin text-muted-foreground', spinnerClass, props.class)"
  />
</template>

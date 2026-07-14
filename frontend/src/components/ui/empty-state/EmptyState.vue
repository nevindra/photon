<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  title: string
  description?: string
  class?: HTMLAttributes['class']
}>()

defineSlots<{
  icon?: () => any
  default?: () => any
}>()
</script>

<template>
  <div :class="cn('flex h-full flex-col items-center justify-center gap-1 py-16 text-center', props.class)">
    <div v-if="$slots.icon" class="mb-2 text-muted-foreground [&_svg]:h-8 [&_svg]:w-8">
      <slot name="icon" />
    </div>
    <p class="text-sm text-foreground">{{ title }}</p>
    <p v-if="description" class="text-xs text-muted-foreground">{{ description }}</p>
    <slot />
  </div>
</template>

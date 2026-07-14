<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <kbd
    :class="
      cn(
        'inline-flex min-w-[15px] items-center justify-center rounded border border-border bg-background px-1 py-px font-mono text-[10px] leading-none text-muted-foreground',
        props.class,
      )
    "
  >
    <slot />
  </kbd>
</template>

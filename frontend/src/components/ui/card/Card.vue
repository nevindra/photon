<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

const props = withDefaults(
  defineProps<{
    class?: HTMLAttributes['class']
    /** Raised chrome that lifts on hover — for clickable cards. Dense-content cards should leave this false so they stay static. */
    interactive?: boolean
  }>(),
  {
    interactive: false,
  },
)
</script>

<template>
  <div
    :class="
      cn(
        'rounded-lg border border-border bg-card text-card-foreground shadow-1',
        props.interactive &&
          'transition-[transform,box-shadow] duration-150 ease-out hover:shadow-2 motion-safe:hover:-translate-y-0.5',
        props.class,
      )
    "
  >
    <slot />
  </div>
</template>

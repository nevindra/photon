<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

const props = withDefaults(
  defineProps<{
    class?: HTMLAttributes['class']
    /** Clickable cards: the hairline darkens and the card lifts onto a real shadow on hover. Dense-content cards should leave this false so they stay static. */
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
        'rounded-xl border border-border bg-card text-card-foreground shadow-1',
        props.interactive &&
          'cursor-pointer transition-[box-shadow,border-color] duration-150 ease-out hover:border-foreground/15 hover:shadow-2',
        props.class,
      )
    "
  >
    <slot />
  </div>
</template>

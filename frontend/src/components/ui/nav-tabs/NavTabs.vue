<script setup>
import { cn } from '@/lib/core/utils'

const props = defineProps({
  class: { type: String, default: '' },
  ariaLabel: { type: String, default: undefined },
})
</script>

<template>
  <nav
    :aria-label="ariaLabel"
    :class="
      cn(
        'inline-flex h-9 items-center justify-center gap-1 rounded-lg bg-muted p-1 text-muted-foreground',
        props.class,
      )
    "
  >
    <slot />
  </nav>
</template>

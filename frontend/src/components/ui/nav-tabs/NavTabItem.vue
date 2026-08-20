<script setup>
import { RouterLink } from 'vue-router'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  to: { type: [String, Object], required: true },
  active: { type: Boolean, default: false },
  class: { type: String, default: '' },
})
</script>

<template>
  <RouterLink
    :to="to"
    :aria-current="active ? 'page' : undefined"
    :data-state="active ? 'active' : 'inactive'"
    :class="
      cn(
        'inline-flex h-full items-center justify-center gap-1.5 whitespace-nowrap rounded-md px-3 text-sm font-medium text-muted-foreground transition-[background-color,box-shadow,color] duration-150 ease-out hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring data-[state=active]:bg-surface-1 dark:data-[state=active]:bg-secondary data-[state=active]:shadow-1 data-[state=active]:ring-1 data-[state=active]:ring-inset data-[state=active]:ring-border data-[state=active]:text-foreground [&_svg]:size-4 [&_svg]:shrink-0',
        props.class,
      )
    "
  >
    <slot />
  </RouterLink>
</template>

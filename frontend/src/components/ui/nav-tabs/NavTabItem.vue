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
        'inline-flex items-center justify-center gap-1.5 whitespace-nowrap border-b-2 border-transparent px-3 py-1 text-sm font-medium text-muted-foreground transition-all hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 data-[state=active]:border-b-brand data-[state=active]:text-brand [&_svg]:size-4 [&_svg]:shrink-0',
        props.class,
      )
    "
  >
    <slot />
  </RouterLink>
</template>

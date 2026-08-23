<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import { ToggleGroupItem, type ToggleGroupItemProps, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/core/utils'

const props = defineProps<
  ToggleGroupItemProps & {
    class?: HTMLAttributes['class']
  }
>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwardedProps = useForwardProps(delegatedProps)
</script>

<template>
  <ToggleGroupItem
    v-bind="forwardedProps"
    :class="
      cn(
        'inline-flex items-center justify-center gap-1.5 rounded-md px-2.5 py-1 text-xs font-medium text-muted-foreground transition-[background-color,box-shadow,color] duration-150 ease-out hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring data-[state=on]:bg-surface-1 dark:data-[state=on]:bg-secondary data-[state=on]:shadow-1 data-[state=on]:ring-1 data-[state=on]:ring-inset data-[state=on]:ring-border data-[state=on]:text-foreground disabled:pointer-events-none disabled:opacity-50',
        props.class,
      )
    "
  >
    <slot />
  </ToggleGroupItem>
</template>

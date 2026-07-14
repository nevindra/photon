<script setup lang="ts">
import { computed, type HTMLAttributes, inject } from 'vue'
import { ToggleGroupItem, type ToggleGroupItemProps, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/core/utils'
import { TOGGLE_GROUP_KEY, toggleVariants } from '.'

const props = defineProps<
  ToggleGroupItemProps & {
    class?: HTMLAttributes['class']
    variant?: 'default' | 'outline'
    size?: 'default' | 'sm' | 'lg'
  }
>()

const context = inject<{ variant: 'default' | 'outline'; size: 'default' | 'sm' | 'lg' }>(
  TOGGLE_GROUP_KEY,
  { variant: 'default', size: 'default' },
)

const delegatedProps = computed(() => {
  const { class: _, variant: __, size: ___, ...delegated } = props
  return delegated
})

const forwardedProps = useForwardProps(delegatedProps)
</script>

<template>
  <ToggleGroupItem
    v-bind="forwardedProps"
    :class="
      cn(
        toggleVariants({
          variant: context.variant || variant,
          size: context.size || size,
        }),
        props.class,
      )
    "
  >
    <slot />
  </ToggleGroupItem>
</template>

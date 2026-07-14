<script setup lang="ts">
import { computed, type HTMLAttributes, provide } from 'vue'
import {
  ToggleGroupRoot,
  type ToggleGroupRootEmits,
  type ToggleGroupRootProps,
  useForwardPropsEmits,
} from 'reka-ui'
import { cn } from '@/lib/core/utils'
import { TOGGLE_GROUP_KEY } from '.'

const props = withDefaults(
  defineProps<
    ToggleGroupRootProps & {
      class?: HTMLAttributes['class']
      variant?: 'default' | 'outline'
      size?: 'default' | 'sm' | 'lg'
    }
  >(),
  {
    variant: 'default',
    size: 'default',
  },
)

const emits = defineEmits<ToggleGroupRootEmits>()

provide(TOGGLE_GROUP_KEY, {
  variant: props.variant,
  size: props.size,
})

const delegatedProps = computed(() => {
  const { class: _, size: __, variant: ___, ...delegated } = props
  return delegated
})

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
  <ToggleGroupRoot
    v-bind="forwarded"
    :class="cn('flex items-center justify-center gap-1', props.class)"
  >
    <slot />
  </ToggleGroupRoot>
</template>

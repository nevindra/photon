<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import {
  ToggleGroupRoot,
  type ToggleGroupRootEmits,
  type ToggleGroupRootProps,
  useForwardPropsEmits,
} from 'reka-ui'
import { cn } from '@/lib/core/utils'

const props = withDefaults(
  defineProps<
    ToggleGroupRootProps & {
      class?: HTMLAttributes['class']
    }
  >(),
  {
    type: 'single',
  },
)

const emits = defineEmits<ToggleGroupRootEmits>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
  <ToggleGroupRoot
    v-bind="forwarded"
    :class="cn('inline-flex items-center gap-1 rounded-md bg-muted p-0.5 shadow-sink', props.class)"
  >
    <slot />
  </ToggleGroupRoot>
</template>

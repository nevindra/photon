<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import {
  TabsRoot,
  type TabsRootEmits,
  type TabsRootProps,
  useForwardPropsEmits,
} from 'reka-ui'
import { cn } from '@/lib/core/utils'

const props = defineProps<TabsRootProps & { class?: HTMLAttributes['class'] }>()
const emits = defineEmits<TabsRootEmits>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
  <TabsRoot v-bind="forwarded" :class="cn('flex flex-col gap-4', props.class)">
    <slot />
  </TabsRoot>
</template>

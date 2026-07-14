<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import { TabsList, type TabsListProps, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/core/utils'

const props = defineProps<TabsListProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardProps(delegatedProps)
</script>

<template>
  <TabsList
    v-bind="forwarded"
    :class="
      cn(
        'inline-flex h-9 items-center justify-center gap-1 rounded-lg bg-muted p-1 text-muted-foreground',
        props.class,
      )
    "
  >
    <slot />
  </TabsList>
</template>

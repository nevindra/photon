<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import { ToastDescription, type ToastDescriptionProps, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/core/utils'

const props = defineProps<ToastDescriptionProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardProps(delegatedProps)
</script>

<template>
  <ToastDescription v-bind="forwarded" :class="cn('text-sm opacity-90', props.class)">
    <slot />
  </ToastDescription>
</template>

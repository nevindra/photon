<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import { ToastViewport, type ToastViewportProps, useForwardProps } from 'reka-ui'
import { cn } from '@/lib/core/utils'

const props = defineProps<ToastViewportProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardProps(delegatedProps)
</script>

<template>
  <ToastViewport
    v-bind="forwarded"
    :class="
      cn(
        'fixed bottom-0 right-0 z-[100] flex max-h-screen w-full flex-col-reverse gap-2 p-4 sm:max-w-[380px]',
        props.class,
      )
    "
  />
</template>

<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import { ToastClose, type ToastCloseProps, useForwardProps } from 'reka-ui'
import { X } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps<ToastCloseProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardProps(delegatedProps)
</script>

<template>
  <ToastClose
    v-bind="forwarded"
    :class="
      cn(
        'absolute right-2 top-2 rounded-md p-1 text-foreground/50 opacity-0 transition-opacity hover:text-foreground focus:opacity-100 focus:outline-none focus:ring-2 focus:ring-ring group-hover:opacity-100',
        props.class,
      )
    "
  >
    <X class="h-4 w-4" />
    <span class="sr-only">Close</span>
  </ToastClose>
</template>

<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import { ToastRoot, type ToastRootEmits, type ToastRootProps, useForwardPropsEmits } from 'reka-ui'
import { cn } from '@/lib/core/utils'
import { toastVariants } from '.'

const props = defineProps<
  ToastRootProps & {
    class?: HTMLAttributes['class']
    variant?: 'default' | 'success' | 'error' | 'warning'
  }
>()
const emits = defineEmits<ToastRootEmits>()

const delegatedProps = computed(() => {
  const { class: _, variant: __, ...delegated } = props
  return delegated
})

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
  <ToastRoot
    v-bind="forwarded"
    :class="cn(toastVariants({ variant }), props.class)"
  >
    <slot />
  </ToastRoot>
</template>

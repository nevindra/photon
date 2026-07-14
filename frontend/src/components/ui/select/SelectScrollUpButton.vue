<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import { SelectScrollUpButton, type SelectScrollUpButtonProps, useForwardProps } from 'reka-ui'
import { ChevronUp } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps<SelectScrollUpButtonProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardProps(delegatedProps)
</script>

<template>
  <SelectScrollUpButton
    v-bind="forwarded"
    :class="cn('flex cursor-default items-center justify-center py-1', props.class)"
  >
    <ChevronUp class="h-4 w-4" />
  </SelectScrollUpButton>
</template>

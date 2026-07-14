<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  modelValue?: string | number
  class?: HTMLAttributes['class']
}>()

const emits = defineEmits<{
  (e: 'update:modelValue', value: string | number): void
}>()

function onInput(event: Event) {
  emits('update:modelValue', (event.target as HTMLInputElement).value)
}
</script>

<template>
  <input
    :value="modelValue"
    :class="
      cn(
        'flex h-9 w-full rounded-md border border-input bg-background shadow-sink px-3 py-1 text-sm transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50',
        props.class,
      )
    "
    @input="onInput"
  >
</template>

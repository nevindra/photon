<script setup lang="ts">
// Multi-line sibling of `ui/input`: the same flat field look (hairline, brand focus ring, no inset
// shadow). `mono` is for things that are code — compose files, keys, address lists — where a
// proportional face hides a stray space.
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  modelValue?: string
  mono?: boolean
  class?: HTMLAttributes['class']
}>()

const emits = defineEmits<{
  (e: 'update:modelValue', value: string): void
}>()

function onInput(event: Event) {
  emits('update:modelValue', (event.target as HTMLTextAreaElement).value)
}
</script>

<template>
  <textarea
    :value="modelValue"
    :class="
      cn(
        'flex min-h-[4.5rem] w-full rounded-md border border-input bg-surface-1 px-3 py-2 text-sm text-foreground transition-[border-color,box-shadow] duration-150 ease-out placeholder:text-muted-foreground hover:border-foreground/15 focus-visible:border-ring focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/20 disabled:cursor-not-allowed disabled:opacity-50',
        props.mono && 'font-mono text-xs leading-5',
        props.class,
      )
    "
    @input="onInput"
  />
</template>

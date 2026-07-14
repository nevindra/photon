<script setup lang="ts">
import { type HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

defineProps<{
  class?: HTMLAttributes['class']
  // Classes for the scroll wrapper. Defaults to `overflow-auto` (an internal horizontal
  // scroller). Pass `overflow-visible` when the table lives inside a page-level scroller and a
  // `sticky` header must stick to THAT scroller rather than being trapped in this wrapper.
  containerClass?: HTMLAttributes['class']
}>()
</script>

<template>
  <div :class="cn('relative w-full overflow-auto', $props.containerClass)">
    <table :class="cn('w-full caption-bottom text-sm', $props.class)">
      <slot />
    </table>
  </div>
</template>

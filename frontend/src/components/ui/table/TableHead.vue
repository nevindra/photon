<script setup lang="ts">
import { type HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <th :class="cn('h-9 px-3 py-2 text-left align-middle text-[10px] font-medium uppercase tracking-wider text-muted-foreground border-b border-border [&:has([role=checkbox])]:pr-0', $props.class)">
    <slot />
  </th>
</template>

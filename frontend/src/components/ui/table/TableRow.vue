<script setup lang="ts">
import { type HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <tr :class="cn('border-b border-border transition-colors hover:bg-muted data-[state=selected]:bg-brand-soft data-[state=selected]:shadow-[inset_2.5px_0_0_hsl(var(--brand))]', $props.class)">
    <slot />
  </tr>
</template>

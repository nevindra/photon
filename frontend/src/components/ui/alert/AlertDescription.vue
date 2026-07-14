<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <div :class="cn('text-sm opacity-90 [&_p]:leading-relaxed', $props.class)">
    <slot />
  </div>
</template>

<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <div
    :class="
      cn('animate-pulse rounded-md bg-muted motion-reduce:animate-none', props.class)
    "
  />
</template>

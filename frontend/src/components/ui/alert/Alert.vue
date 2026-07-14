<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'
import { alertVariants } from '.'

const props = defineProps<{
  variant?: 'default' | 'success' | 'error' | 'warning' | 'info'
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <div role="alert" :class="cn(alertVariants({ variant: props.variant }), props.class)">
    <slot />
  </div>
</template>

<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{ class?: HTMLAttributes['class'] }>()
</script>

<template>
  <div :class="cn('flex min-w-0 flex-col gap-1.5 pr-6 text-left', props.class)">
    <slot />
  </div>
</template>

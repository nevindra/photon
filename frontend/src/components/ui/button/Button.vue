<script setup lang="ts">
import { Primitive, type PrimitiveProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/core/utils'
import { buttonVariants } from '.'

const props = withDefaults(
  defineProps<
    PrimitiveProps & {
      variant?: 'default' | 'destructive' | 'outline' | 'secondary' | 'brand' | 'ghost' | 'link'
      size?: 'default' | 'sm' | 'lg' | 'icon'
      class?: HTMLAttributes['class']
    }
  >(),
  { as: 'button' },
)
</script>

<template>
  <Primitive
    data-slot="button"
    :as="as"
    :as-child="asChild"
    :class="cn(buttonVariants({ variant, size }), props.class)"
  >
    <slot />
  </Primitive>
</template>

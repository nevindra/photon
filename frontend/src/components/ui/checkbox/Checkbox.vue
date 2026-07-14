<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import {
  CheckboxIndicator,
  CheckboxRoot,
  type CheckboxRootEmits,
  type CheckboxRootProps,
  useForwardPropsEmits,
} from 'reka-ui'
import { Check, Minus } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps<CheckboxRootProps & { class?: HTMLAttributes['class'] }>()
const emits = defineEmits<CheckboxRootEmits>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
  <CheckboxRoot
    v-bind="forwarded"
    v-slot="{ state }"
    :class="
      cn(
        'peer size-4 shrink-0 rounded-md border border-input bg-background shadow-sink transition-[transform,box-shadow,background-color,border-color] duration-150 ease-out focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:border-transparent data-[state=checked]:bg-brand data-[state=checked]:text-brand-foreground data-[state=checked]:shadow-1 data-[state=indeterminate]:border-transparent data-[state=indeterminate]:bg-brand data-[state=indeterminate]:text-brand-foreground data-[state=indeterminate]:shadow-1',
        props.class,
      )
    "
  >
    <CheckboxIndicator
      class="flex h-full w-full items-center justify-center text-current"
    >
      <slot>
        <Minus v-if="state === 'indeterminate'" class="h-3.5 w-3.5" />
        <Check v-else class="h-3.5 w-3.5" />
      </slot>
    </CheckboxIndicator>
  </CheckboxRoot>
</template>

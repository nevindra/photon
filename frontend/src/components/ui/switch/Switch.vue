<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import {
  SwitchRoot,
  type SwitchRootEmits,
  type SwitchRootProps,
  SwitchThumb,
  useForwardPropsEmits,
} from 'reka-ui'
import { Check } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps<SwitchRootProps & { class?: HTMLAttributes['class'] }>()
const emits = defineEmits<SwitchRootEmits>()

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props
  return delegated
})

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
  <!-- Tactile "keycap" switch: the thumb is a rounded square TALLER than the track,
       overhanging it with an offset shadow so it reads as a physical, lifted key.
       Cyan brand track when on; a small brand check appears on the keycap. -->
  <SwitchRoot
    v-bind="forwarded"
    :class="
      cn(
        'peer relative inline-flex h-5 w-10 shrink-0 cursor-pointer items-center rounded-lg transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-brand data-[state=unchecked]:bg-input',
        props.class,
      )
    "
  >
    <SwitchThumb
      class="group pointer-events-none flex h-6 w-6 items-center justify-center rounded-lg bg-surface-1 shadow-1 ring-1 ring-border transition-transform duration-200 ease-[cubic-bezier(0.2,0,0,1)] data-[state=checked]:translate-x-4 data-[state=unchecked]:translate-x-0"
    >
      <Check
        class="size-3.5 stroke-[3] text-brand opacity-0 transition-opacity duration-150 group-data-[state=checked]:opacity-100"
      />
    </SwitchThumb>
  </SwitchRoot>
</template>

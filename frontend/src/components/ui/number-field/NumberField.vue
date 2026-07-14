<script setup lang="ts">
import { computed } from 'vue'
import {
  NumberFieldRoot,
  NumberFieldInput,
  NumberFieldIncrement,
  NumberFieldDecrement,
} from 'reka-ui'
import { ChevronUp, ChevronDown } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  modelValue?: number
  min?: number
  max?: number
  step?: number
  id?: string
  disabled?: boolean
  unit?: string
  showSteppers?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: number): void
}>()

// Calculate padding based on what we're showing
const inputPaddingClass = computed(() => {
  if (props.unit && props.showSteppers) return 'pr-14'
  if (props.unit || props.showSteppers) return 'pr-8'
  return ''
})

const actualStep = computed(() => props.step ?? 1)

const shouldShowSteppers = computed(() => props.showSteppers !== false)

// Input class matches Input.vue exactly
const inputBaseClass = 'flex h-9 w-full rounded-md border border-input bg-background shadow-sink px-3 py-1 text-sm transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50'
</script>

<template>
  <NumberFieldRoot
    :model-value="modelValue"
    :min="min"
    :max="max"
    :step="actualStep"
    :disabled="disabled"
    @update:model-value="emit('update:modelValue', $event)"
    class="relative"
  >
    <NumberFieldInput
      :id="id"
      :class="cn(inputBaseClass, inputPaddingClass)"
    />

    <!-- Unit suffix -->
    <span
      v-if="unit"
      class="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-muted-foreground"
    >
      {{ unit }}
    </span>

    <!-- Step buttons -->
    <div
      v-if="shouldShowSteppers"
      class="absolute right-1 top-1/2 -translate-y-1/2 flex flex-col gap-0.5"
    >
      <NumberFieldIncrement
        class="inline-flex h-4 w-4 items-center justify-center rounded-sm disabled:opacity-50 hover:bg-muted transition-[transform,background-color] duration-100 motion-safe:active:translate-y-px disabled:pointer-events-none"
      >
        <ChevronUp class="h-3 w-3 text-muted-foreground hover:text-foreground" />
      </NumberFieldIncrement>

      <NumberFieldDecrement
        class="inline-flex h-4 w-4 items-center justify-center rounded-sm disabled:opacity-50 hover:bg-muted transition-[transform,background-color] duration-100 motion-safe:active:translate-y-px disabled:pointer-events-none"
      >
        <ChevronDown class="h-3 w-3 text-muted-foreground hover:text-foreground" />
      </NumberFieldDecrement>
    </div>
  </NumberFieldRoot>
</template>

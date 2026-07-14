<script setup>
// Compact toolbar dropdown-select, built on the (jsdom-testable) Popover primitive rather than the
// Reka `Select` listbox — Reka Select needs pointer-capture APIs jsdom lacks, so it can't be driven
// in tests. The trigger shows an optional muted `prefix` + the current option's label + a chevron;
// opening reveals a small menu of options that emit `update:modelValue` and close on choose. One
// place for the toolbar-select look (Sort, refresh-mode, …) so they all read alike.
import { ref, computed } from 'vue'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Check, ChevronDown } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  modelValue: { type: [String, Number], default: null },
  // The choices, in display order: [{ value, label }].
  options: { type: Array, default: () => [] },
  // Optional muted label shown before the value on the trigger, e.g. "Sort:".
  prefix: { type: String, default: '' },
  align: { type: String, default: 'start' },
  contentClass: { type: String, default: 'w-40' },
  ariaLabel: { type: String, default: '' },
})
const emit = defineEmits(['update:modelValue'])

const open = ref(false)
const currentLabel = computed(
  () => props.options.find((o) => o.value === props.modelValue)?.label ?? '',
)

function choose(value) {
  if (value !== props.modelValue) emit('update:modelValue', value)
  open.value = false
}
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger
      :aria-label="ariaLabel || prefix || 'Select'"
      :class="
        cn(
          'pk inline-flex h-7 items-center gap-1.5 whitespace-nowrap rounded-md border border-input bg-surface-1 px-2.5 text-xs font-medium text-foreground hover:bg-muted focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring data-[state=open]:bg-muted',
        )
      "
    >
      <span v-if="prefix" class="text-muted-foreground">{{ prefix }}</span>
      <span class="font-mono">{{ currentLabel }}</span>
      <ChevronDown class="size-3.5 opacity-50" />
    </PopoverTrigger>
    <PopoverContent :align="align" :class="cn('p-1', contentClass)">
      <div class="flex flex-col gap-0.5">
        <button
          v-for="o in options"
          :key="o.value"
          type="button"
          :data-value="o.value"
          :data-testid="`select-option-${o.value}`"
          :aria-selected="o.value === modelValue"
          class="flex cursor-pointer items-center justify-between gap-3 rounded-sm px-2 py-1 text-left font-mono text-xs text-foreground hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:outline-none"
          @click="choose(o.value)"
        >
          <span class="truncate">{{ o.label }}</span>
          <Check v-if="o.value === modelValue" class="size-3.5 shrink-0" />
        </button>
      </div>
    </PopoverContent>
  </Popover>
</template>

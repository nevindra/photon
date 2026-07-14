<!-- frontend/src/components/metrics/MetricVizSwitcher.vue -->
<script setup lang="ts">
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import type { VizDef } from '@/lib/metrics/metricViz'

const props = withDefaults(defineProps<{
  modelValue?: string
  available?: string[]
  allViz?: VizDef[]
}>(), {
  modelValue: 'line',
  available: () => [],
  allViz: () => [],
})
const emit = defineEmits<{ 'update:modelValue': [id: string] }>()

const isOn = (id: string): boolean => props.available.includes(id)
function choose(v: VizDef): void {
  if (!isOn(v.id)) return
  emit('update:modelValue', v.id)
}
</script>

<template>
  <div data-testid="viz-switcher" class="inline-flex items-center gap-0.5 rounded-lg border border-border p-0.5">
    <Tooltip v-for="v in allViz" :key="v.id">
      <TooltipTrigger as-child>
        <button
          type="button"
          :data-testid="'viz-opt-' + v.id"
          :disabled="!isOn(v.id)"
          class="flex items-center gap-1 rounded-md px-2 py-1 text-[11px] transition-colors"
          :class="[
            modelValue === v.id ? 'bg-foreground text-background' : 'text-muted-foreground hover:text-foreground',
            !isOn(v.id) ? 'cursor-not-allowed opacity-40' : '',
          ]"
          @click="choose(v)"
        >
          <component :is="v.icon" v-if="v.icon" class="size-3.5" />
          <span>{{ v.label }}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent v-if="!isOn(v.id)">Not available for this data</TooltipContent>
    </Tooltip>
  </div>
</template>

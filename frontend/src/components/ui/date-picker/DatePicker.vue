<script setup lang="ts">
// Single-date picker — a compact, preset-first popover mirroring the
// header TimeRangePicker (common/TimeRangePicker.vue). Used by the /data
// Delete tab's "delete older than" cutoff: relative presets (7d, 30d, …)
// or a specific calendar date, resolved to an ISO YYYY-MM-DD string.
//
// Presets and the specific-date input are mutually exclusive: picking a
// preset clears the specific date, editing the specific date clears the
// pending preset. Nothing is emitted until Apply (the trigger label always
// reflects the applied modelValue, never the pending in-popover selection).
import { computed, ref, watch } from 'vue'
import { Calendar, ChevronDown } from 'lucide-vue-next'
import { Button, buttonVariants } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/core/utils'

const props = withDefaults(defineProps<{ modelValue?: string }>(), {
  modelValue: '',
})

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

// Relative cutoffs — label shown in the grid, `days` subtracted from today.
const PRESETS: { label: string; days: number }[] = [
  { label: '7d', days: 7 },
  { label: '30d', days: 30 },
  { label: '90d', days: 90 },
  { label: '6mo', days: 180 },
  { label: '1y', days: 365 },
  { label: '2y', days: 730 },
]

const open = ref(false)
// Pending selection — nothing here is emitted to the parent until Apply.
const pendingPreset = ref('')
const specificInput = ref('')

// Date -> local "YYYY-MM-DD". Uses local date parts (NOT toISOString(), which
// is UTC and can off-by-one across the local midnight boundary).
function toLocalIso(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

// ISO "YYYY-MM-DD" -> human display label. Parses the parts into a local Date
// (rather than `new Date(iso)`, which parses bare dates as UTC and can shift a
// day) so the label matches the calendar date the user picked.
function formatDisplay(iso: string): string {
  if (!iso) return ''
  const [y, m, d] = iso.split('-').map(Number)
  if (!y || !m || !d) return iso
  return new Date(y, m - 1, d).toLocaleDateString('en-US', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  })
}

// today minus N days, as a local ISO date.
function resolvePresetIso(days: number): string {
  const d = new Date()
  d.setDate(d.getDate() - days)
  return toLocalIso(d)
}

// Trigger label always reflects the applied prop, never the pending selection.
const triggerLabel = computed(() =>
  props.modelValue ? formatDisplay(props.modelValue) : 'Pick a date…',
)

// The resolved cutoff for the current pending selection (preset takes
// precedence). Empty when nothing valid is pending.
const resolvedIso = computed(() => {
  if (pendingPreset.value) {
    const preset = PRESETS.find((p) => p.label === pendingPreset.value)
    return preset ? resolvePresetIso(preset.days) : ''
  }
  return specificInput.value || ''
})

const canApply = computed(() => !!resolvedIso.value)

// Seed pending state from the applied modelValue whenever the popover opens, so
// it reflects what's actually applied rather than stale edits. The applied
// value is a bare date with no preset provenance, so it seeds the specific-date
// input with no preset highlighted.
watch(open, (isOpen) => {
  if (!isOpen) return
  pendingPreset.value = ''
  specificInput.value = props.modelValue || ''
})

function isActivePreset(preset: string): boolean {
  return pendingPreset.value === preset
}

function selectPreset(preset: string): void {
  pendingPreset.value = preset
  specificInput.value = ''
}

// Editing the specific date by hand means it takes over from any pending preset.
function clearPendingPreset(): void {
  pendingPreset.value = ''
}

function applySelection(): void {
  if (!canApply.value) return
  emit('update:modelValue', resolvedIso.value)
  open.value = false
}
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger
      :class="cn(buttonVariants({ variant: 'outline', size: 'sm' }), 'gap-1.5 font-mono text-xs')"
    >
      <Calendar class="size-3.5 text-muted-foreground" />
      {{ triggerLabel }}
      <ChevronDown class="size-3.5 text-muted-foreground" />
    </PopoverTrigger>

    <PopoverContent align="end" class="w-64 p-3">
      <div class="mb-2 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        Older than
      </div>
      <div class="grid grid-cols-3 gap-1">
        <button
          v-for="preset in PRESETS"
          :key="preset.label"
          type="button"
          :class="
            cn(
              'rounded-sm border border-transparent px-2 py-1.5 font-mono text-xs text-foreground transition-colors hover:bg-accent hover:text-accent-foreground',
              isActivePreset(preset.label) &&
                'border-border bg-neutral-200 font-semibold text-foreground dark:bg-neutral-800',
            )
          "
          @click="selectPreset(preset.label)"
        >
          {{ preset.label }}
        </button>
      </div>

      <div class="my-3 border-t border-border" />

      <div class="mb-2 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        Specific date
      </div>
      <input
        v-model="specificInput"
        type="date"
        class="w-full rounded-md border border-input bg-background px-2 py-1 font-mono text-xs text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
        @input="clearPendingPreset"
      />

      <p v-if="resolvedIso" class="mt-3 text-[11px] text-muted-foreground">
        Purges everything before
        <span class="font-medium text-foreground">{{ formatDisplay(resolvedIso) }}</span>
      </p>

      <div class="my-3 border-t border-border" />

      <Button size="sm" class="w-full" :disabled="!canApply" @click="applySelection">
        Apply
      </Button>
    </PopoverContent>
  </Popover>
</template>

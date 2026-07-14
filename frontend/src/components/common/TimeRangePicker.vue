<script setup>
// Header time-range control — compact, preset-first popover with a custom
// absolute-range escape hatch. See docs/superpowers/specs/
// 2026-07-02-search-ux-revamp-design.md §5 "Time-range picker (model A)".
//
// Presets and custom range are mutually exclusive: picking a preset clears
// the parent's customRange, applying a custom range clears the parent's
// preset. This component doesn't enforce that itself — it just emits both
// events and trusts the parent (mirrors the existing onRange/onZoom split
// in LogsView).
import { computed, ref, watch } from 'vue'
import { Clock, ChevronDown } from 'lucide-vue-next'
import { Button, buttonVariants } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  modelValue: { type: String, default: '' },
  customRange: { type: Object, default: null },
})

const emit = defineEmits(['update:modelValue', 'update:customRange'])

const PRESETS = ['5m', '15m', '30m', '1h', '3h', '6h', '12h', '24h', '7d']

const open = ref(false)
const fromInput = ref('')
const toInput = ref('')
// Pending selection — nothing here is emitted to the parent until Apply.
const pendingPreset = ref('')

// Trigger label always reflects the applied props, never the pending
// (in-popover, not-yet-applied) selection.
const triggerLabel = computed(() => {
  if (props.customRange) return 'Custom'
  if (props.modelValue) return `Last ${props.modelValue}`
  return 'Select range'
})

// datetime-local values are local-time, no offset ("YYYY-MM-DDTHH:mm") — Date
// parses that as local time, which is exactly what the input represents.
function parseLocalDateTime(value) {
  if (!value) return null
  const ms = new Date(value).getTime()
  return Number.isNaN(ms) ? null : ms
}

// Inverse of parseLocalDateTime: epoch ms -> "YYYY-MM-DDTHH:mm" in local time.
function formatLocalDateTime(ms) {
  const d = new Date(ms)
  const pad = (n) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}

// Seed pending state from the currently-applied props whenever the popover
// opens, so it reflects what's actually applied rather than stale edits.
watch(open, (isOpen) => {
  if (!isOpen) return
  if (props.customRange) {
    fromInput.value = formatLocalDateTime(props.customRange.startMs)
    toInput.value = formatLocalDateTime(props.customRange.endMs)
    pendingPreset.value = ''
  } else {
    pendingPreset.value = props.modelValue
    fromInput.value = ''
    toInput.value = ''
  }
})

function isActivePreset(preset) {
  return pendingPreset.value === preset
}

function selectPreset(preset) {
  pendingPreset.value = preset
  fromInput.value = ''
  toInput.value = ''
}

// Editing From/To by hand means custom range takes over from any pending preset.
function clearPendingPreset() {
  pendingPreset.value = ''
}

const fromMs = computed(() => parseLocalDateTime(fromInput.value))
const toMs = computed(() => parseLocalDateTime(toInput.value))

const canApplyCustomRange = computed(
  () => fromMs.value !== null && toMs.value !== null && fromMs.value <= toMs.value,
)

const canApply = computed(() => canApplyCustomRange.value || !!pendingPreset.value)

function applySelection() {
  if (canApplyCustomRange.value) {
    emit('update:customRange', { startMs: fromMs.value, endMs: toMs.value })
    open.value = false
    return
  }
  if (pendingPreset.value) {
    emit('update:modelValue', pendingPreset.value)
    open.value = false
  }
}
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger
      :class="cn(buttonVariants({ variant: 'outline', size: 'sm' }), 'gap-1.5 font-mono text-xs')"
    >
      <Clock class="size-3.5 text-muted-foreground" />
      {{ triggerLabel }}
      <ChevronDown class="size-3.5 text-muted-foreground" />
    </PopoverTrigger>

    <PopoverContent align="end" class="w-64 p-3">
      <div class="mb-2 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        Quick ranges
      </div>
      <div class="grid grid-cols-3 gap-1">
        <button
          v-for="preset in PRESETS"
          :key="preset"
          type="button"
          :class="
            cn(
              'rounded-sm border border-transparent px-2 py-1.5 font-mono text-xs text-foreground transition-colors hover:bg-accent hover:text-accent-foreground',
              isActivePreset(preset) &&
                'border-border bg-neutral-200 font-semibold text-foreground dark:bg-neutral-800',
            )
          "
          @click="selectPreset(preset)"
        >
          {{ preset }}
        </button>
      </div>

      <div class="my-3 border-t border-border" />

      <div class="mb-2 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        Custom range
      </div>
      <div class="flex flex-col gap-2">
        <label class="flex flex-col gap-1">
          <span class="text-[10px] uppercase tracking-wider text-muted-foreground">From</span>
          <input
            v-model="fromInput"
            type="datetime-local"
            class="rounded-md border border-input bg-background px-2 py-1 font-mono text-xs text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
            @input="clearPendingPreset"
          />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-[10px] uppercase tracking-wider text-muted-foreground">To</span>
          <input
            v-model="toInput"
            type="datetime-local"
            class="rounded-md border border-input bg-background px-2 py-1 font-mono text-xs text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
            @input="clearPendingPreset"
          />
        </label>
      </div>

      <div class="my-3 border-t border-border" />

      <Button size="sm" class="w-full" :disabled="!canApply" @click="applySelection">
        Apply
      </Button>
    </PopoverContent>
  </Popover>
</template>

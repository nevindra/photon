<!-- frontend/src/components/services/ApdexThresholdControl.vue -->
<script setup>
// Gear button on the service detail header: opens a small popover to view/edit this service's
// Apdex "satisfied" threshold (ms). Mirrors the Popover usage in MetricPicker/TimeRangePicker
// (`v-model:open`, `PopoverTrigger as-child`) and the NumberField + Label pairing in
// MonitorForm.vue. `useSetServiceSettings`/`useResetServiceSettings` (T10) already invalidate
// the service's queries on success, so saving here refreshes the KPI tiles/charts for free.
import { ref, watch } from 'vue'
import { Settings } from 'lucide-vue-next'
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover'
import { Button } from '@/components/ui/button'
import { NumberField } from '@/components/ui/number-field'
import { Label } from '@/components/ui/label'
import {
  useServiceSettings,
  useSetServiceSettings,
  useResetServiceSettings,
} from '@/lib/services/servicesQueries'

const props = defineProps({
  service: { type: String, required: true },
})

const settingsQuery = useServiceSettings(() => props.service)
const setMutation = useSetServiceSettings()
const resetMutation = useResetServiceSettings()

const open = ref(false)
const ms = ref(500)

// Reseed the editable draft from the loaded value whenever it changes (fresh load, or a
// successful save/reset elsewhere) so the input never shows a stale number.
watch(
  () => settingsQuery.data.value?.apdex_threshold_ms,
  (v) => {
    if (v != null) ms.value = v
  },
  { immediate: true },
)

function save() {
  setMutation.mutate(
    { service: props.service, ms: ms.value },
    { onSuccess: () => { open.value = false } },
  )
}
function resetToDefault() {
  resetMutation.mutate(props.service, { onSuccess: () => { open.value = false } })
}
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger as-child>
      <button
        type="button"
        data-testid="apdex-threshold-trigger"
        aria-label="Apdex threshold settings"
        class="inline-flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
      >
        <Settings class="size-4" />
      </button>
    </PopoverTrigger>
    <PopoverContent align="end" class="w-64">
      <div class="flex flex-col gap-3">
        <div>
          <h4 class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Apdex threshold</h4>
          <p data-testid="apdex-current" class="mt-0.5 text-[11px] text-muted-foreground/70">
            <template v-if="settingsQuery.data.value">
              Current: {{ settingsQuery.data.value.apdex_threshold_ms }}ms<template v-if="settingsQuery.data.value.is_default"> (default)</template>
            </template>
            <template v-else>Loading…</template>
          </p>
        </div>

        <div class="space-y-1.5">
          <Label for="apdex-threshold-ms">Satisfied under (T)</Label>
          <NumberField id="apdex-threshold-ms" v-model="ms" :min="1" unit="ms" />
        </div>

        <div class="flex items-center justify-between gap-2 pt-1">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            data-testid="apdex-reset"
            :disabled="resetMutation.isPending.value"
            @click="resetToDefault"
          >
            Reset to default
          </Button>
          <Button
            type="button"
            size="sm"
            data-testid="apdex-save"
            :disabled="setMutation.isPending.value"
            @click="save"
          >
            Save
          </Button>
        </div>
      </div>
    </PopoverContent>
  </Popover>
</template>

<script setup lang="ts">
// The dangerous-construct list (R8.4) with one acknowledgement per warning. Nothing ships until
// every line has been read and ticked; the ids are stable so an acknowledgement survives edits
// elsewhere in the file. Read-only mode lists what was acknowledged when the stack was created.
import { TriangleAlert, ShieldAlert } from 'lucide-vue-next'
import { Checkbox } from '@/components/ui/checkbox'
import type { ComposeWarning } from '@/lib/core/api'

const props = defineProps<{ warnings: ComposeWarning[]; acknowledged: string[]; readonly?: boolean }>()
const emit = defineEmits<{ 'update:acknowledged': [string[]] }>()

function toggle(id: string, on: boolean): void {
  const set = new Set(props.acknowledged)
  if (on) set.add(id)
  else set.delete(id)
  emit('update:acknowledged', Array.from(set))
}
</script>

<template>
  <div v-if="warnings.length" class="flex flex-col divide-y divide-border rounded-lg border border-sev-warn/30 bg-card" data-testid="compose-warnings">
    <div class="flex items-center gap-2 px-3 py-2 text-xs font-medium text-sev-warn">
      <ShieldAlert class="size-4" /> {{ warnings.length }} {{ warnings.length === 1 ? 'construct needs' : 'constructs need' }} an explicit acknowledgement before this ships to a host
    </div>
    <label v-for="w in warnings" :key="w.id" class="flex items-start gap-3 px-3 py-2" :class="readonly ? '' : 'cursor-pointer'" :data-warning="w.id">
      <Checkbox v-if="!readonly" :model-value="acknowledged.includes(w.id)" class="mt-0.5" @update:model-value="toggle(w.id, $event === true)" />
      <TriangleAlert v-else class="mt-0.5 size-4 shrink-0" :class="w.severity === 'error' ? 'text-sev-error' : 'text-sev-warn'" />
      <span class="min-w-0 flex-1 text-xs">
        <span class="flex flex-wrap items-center gap-2">
          <span class="font-mono font-medium" :class="w.severity === 'error' ? 'text-sev-error' : 'text-sev-warn'">{{ w.kind.replace(/_/g, ' ') }}</span>
          <span v-if="w.service" class="font-mono text-muted-foreground">{{ w.service }}</span>
          <span v-if="w.line" class="text-muted-foreground/70">line {{ w.line }}</span>
        </span>
        <span class="block text-foreground">{{ w.detail }}</span>
      </span>
    </label>
  </div>
</template>

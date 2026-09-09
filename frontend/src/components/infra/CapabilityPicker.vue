<script setup lang="ts">
// The four agent capabilities as a checklist, each with what it allows and the agent-side flag
// that grants it. Used when enrolling (what the install will be started with) — never to change a
// running agent, because capabilities are opt-in on the host and the server cannot widen them.
// `read` is the floor: everything else is meaningless without inventory, so it stays on.
import { Checkbox } from '@/components/ui/checkbox'
import { TriangleAlert } from 'lucide-vue-next'
import { CAPABILITIES, CAPABILITY_META } from '@/lib/deploy/agentStatus'
import type { AgentCapability } from '@/lib/core/api'

const props = defineProps<{ modelValue: AgentCapability[]; disabled?: boolean }>()
const emit = defineEmits<{ 'update:modelValue': [AgentCapability[]] }>()

function toggle(cap: AgentCapability, on: boolean): void {
  if (cap === 'read') return
  const set = new Set(props.modelValue)
  if (on) set.add(cap)
  else set.delete(cap)
  set.add('read')
  emit('update:modelValue', CAPABILITIES.filter((c) => set.has(c)))
}
</script>

<template>
  <div class="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface-1" data-testid="capability-picker">
    <label
      v-for="cap in CAPABILITIES"
      :key="cap"
      class="flex cursor-pointer items-start gap-3 px-3 py-2.5"
      :class="(disabled || cap === 'read') && 'cursor-default'"
    >
      <Checkbox
        :id="`cap-${cap}`"
        :data-cap="cap"
        class="mt-0.5"
        :model-value="cap === 'read' || modelValue.includes(cap)"
        :disabled="disabled || cap === 'read'"
        @update:model-value="toggle(cap, $event === true)"
      />
      <span class="min-w-0 flex-1">
        <span class="flex items-center gap-2 text-sm text-foreground">
          {{ CAPABILITY_META[cap].label }}
          <code class="rounded bg-muted px-1 py-px font-mono text-[10px] text-muted-foreground">{{ CAPABILITY_META[cap].flag }}</code>
          <span v-if="cap === 'read'" class="text-[10px] uppercase tracking-wider text-muted-foreground">always</span>
        </span>
        <span class="block text-xs text-muted-foreground">{{ CAPABILITY_META[cap].summary }}</span>
        <span v-if="CAPABILITY_META[cap].risk" class="mt-1 flex items-center gap-1 text-xs text-sev-warn">
          <TriangleAlert class="size-3" /> {{ CAPABILITY_META[cap].risk }}
        </span>
      </span>
    </label>
  </div>
</template>

<script setup lang="ts">
// The Apdex bands as one stacked bar: satisfied / tolerating / frustrated.
//
// The score alone hides which way a service is failing. 0.85 built from "85% satisfied, 15%
// frustrated" is an outage for one request in seven; 0.85 built from "70% satisfied, 30%
// tolerating" is a service that is merely slow. The bar shows the difference at a glance, and the
// backend has returned these three counts on `/api/red` all along without anything rendering them.
import { computed } from 'vue'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { formatCompact } from '@/lib/core/format'

const props = defineProps<{
  satisfied?: number
  tolerating?: number
  frustrated?: number
  thresholdMs?: number | null
}>()

const n = (v?: number) => (Number.isFinite(v) ? (v as number) : 0)

const bands = computed(() => {
  const s = n(props.satisfied)
  const t = n(props.tolerating)
  const f = n(props.frustrated)
  const total = s + t + f
  if (!total) return null
  return {
    total,
    parts: [
      { key: 'satisfied', label: 'Satisfied', n: s, pct: (s / total) * 100, cls: 'bg-success' },
      { key: 'tolerating', label: 'Tolerating', n: t, pct: (t / total) * 100, cls: 'bg-sev-warn' },
      { key: 'frustrated', label: 'Frustrated', n: f, pct: (f / total) * 100, cls: 'bg-sev-error' },
    ],
  }
})
</script>

<template>
  <Tooltip v-if="bands">
    <TooltipTrigger as-child>
      <div
        data-testid="apdex-band"
        class="flex h-1.5 w-full cursor-help overflow-hidden rounded-full bg-muted"
        role="img"
        :aria-label="`Apdex bands: ${bands.parts.map((p) => `${p.label} ${p.pct.toFixed(0)}%`).join(', ')}`"
      >
        <span
          v-for="p in bands.parts"
          :key="p.key"
          :data-band="p.key"
          :class="p.cls"
          :style="{ width: p.pct + '%' }"
        />
      </div>
    </TooltipTrigger>
    <TooltipContent class="text-xs">
      <p v-if="thresholdMs" class="mb-1 text-muted-foreground">T = {{ thresholdMs }}ms · 4T = {{ thresholdMs * 4 }}ms</p>
      <p v-for="p in bands.parts" :key="p.key" class="tabular-nums">
        {{ p.label }} {{ formatCompact(p.n) }} ({{ p.pct.toFixed(1) }}%)
      </p>
    </TooltipContent>
  </Tooltip>
  <span v-else class="text-[10px] text-muted-foreground/50">—</span>
</template>

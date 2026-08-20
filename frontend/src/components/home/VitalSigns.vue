<script setup lang="ts">
// The strip that catches what thresholds miss. Every Subject can be inside its band while the
// fleet quietly loses 87% of its traffic — nothing is "wrong", and everything is. These four
// numbers are read against the previous equal-length Watch Range, which is the only comparison
// basis the board has.
import type { Vital } from '@/lib/home/useAttentionBoard'

defineProps<{ vitals: Vital[] }>()

const TONE: Record<Vital['tone'], string> = {
  bad: 'text-sev-error',
  warn: 'text-sev-warn',
  flat: 'text-muted-foreground',
}
</script>

<template>
  <div
    class="grid grid-cols-2 overflow-hidden rounded-xl border border-border bg-card shadow-1 lg:grid-cols-4"
    data-testid="vital-signs"
  >
    <div
      v-for="(vital, i) in vitals"
      :key="vital.label"
      class="border-border/60 p-3.5"
      :class="[i % 2 === 0 ? 'border-r' : '', i < 2 ? 'border-b lg:border-b-0' : '', 'lg:border-r lg:last:border-r-0']"
    >
      <div class="text-[11px] text-muted-foreground">{{ vital.label }}</div>
      <div class="mt-1 flex items-baseline gap-2">
        <span class="text-xl font-semibold tabular-nums tracking-tight text-foreground">{{ vital.value }}</span>
        <span v-if="vital.delta" class="text-xs font-semibold tabular-nums" :class="TONE[vital.tone]">
          {{ vital.delta }}
        </span>
      </div>
    </div>
  </div>
</template>

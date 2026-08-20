<script setup lang="ts">
// What the five words mean, stated once on the page that ranks by them. Cheap to keep, and it is
// the difference between "Down" reading as a colour and reading as a claim.
import { StatusPill } from '@/components/ui/status-pill'
import { LEVEL_META } from '@/lib/home/attention'

const ENTRIES = [
  { level: 'down' as const, gloss: 'not answering' },
  { level: 'critical' as const, gloss: 'answering badly, user-visible' },
  { level: 'warning' as const, gloss: 'degraded, not yet breaking' },
  { level: 'ok' as const, gloss: 'inside every threshold' },
  { level: 'unknown' as const, gloss: 'no judgment — idle, paused, or never reported' },
]
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-border bg-card px-4 py-3 shadow-1">
    <span class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">Attention level</span>
    <span v-for="entry in ENTRIES" :key="entry.level" class="inline-flex items-center gap-1.5">
      <StatusPill
        :tone="LEVEL_META[entry.level].tone"
        :solid="LEVEL_META[entry.level].solid"
        class="text-[10px] font-bold uppercase tracking-wide"
      >
        {{ LEVEL_META[entry.level].label }}
      </StatusPill>
      <span class="text-[11px] text-muted-foreground">{{ entry.gloss }}</span>
    </span>
  </div>
</template>

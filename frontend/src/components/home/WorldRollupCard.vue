<script setup lang="ts">
// One World's roll-up — and the page's filter control, which is the same thing on purpose. The
// card always shows every Subject the World watches plus the per-level tallies, so the page stays
// trustworthy when nothing is wrong; clicking it narrows the attention list and the vital signs in
// place. Tabs were the first design and were dropped because they hide the tallies of every world
// you are not looking at (see the ADR).
import { computed } from 'vue'
import { StatusPill } from '@/components/ui/status-pill'
import { LEVELS, LEVEL_META, type WorldRollup } from '@/lib/home/attention'

const props = defineProps<{
  rollup: WorldRollup
  active: boolean
}>()

defineEmits<{ (e: 'toggle', key: string): void }>()

// Only the levels that actually occurred — a row of five chips reading "0 Down · 0 Critical" is
// noise, and the total already says how many are fine.
const shown = computed(() => LEVELS.filter((l) => props.rollup.tallies[l] > 0))
</script>

<template>
  <button
    type="button"
    class="group rounded-xl border border-border bg-card p-3.5 text-left shadow-1 transition-[border-color,box-shadow] duration-150 ease-out hover:border-foreground/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
    :class="active ? 'border-brand/45 ring-1 ring-inset ring-brand/30' : ''"
    :aria-pressed="active"
    :data-world="rollup.world.key"
    :data-testid="`world-${rollup.world.key}`"
    @click="$emit('toggle', rollup.world.key)"
  >
    <div class="mb-2.5 flex items-center gap-2">
      <span class="size-2 shrink-0 rounded-full" :class="LEVEL_META[rollup.worst].dot" />
      <span class="text-[13px] font-semibold" :class="active ? 'text-brand' : 'text-foreground'">
        {{ rollup.world.name }}
      </span>
      <span class="ml-auto text-[11px] tabular-nums text-muted-foreground">
        {{ rollup.total }} {{ rollup.world.noun }}
      </span>
    </div>
    <div class="flex flex-wrap gap-1.5">
      <StatusPill
        v-for="level in shown"
        :key="level"
        :tone="LEVEL_META[level].tone"
        :solid="LEVEL_META[level].solid"
        class="tabular-nums"
      >
        {{ rollup.tallies[level] }} {{ LEVEL_META[level].label }}
      </StatusPill>
      <span v-if="!shown.length" class="text-[11px] text-muted-foreground">nothing watched yet</span>
    </div>
  </button>
</template>

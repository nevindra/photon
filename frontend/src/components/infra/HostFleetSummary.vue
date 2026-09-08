<script setup lang="ts">
// The fleet in one line, and the line is the filter. "3 hosts · 1 critical · 1 warning · 1
// waiting" as clickable counts, then the readings that describe the fleet rather than judge it
// (avg CPU, GPU hosts, containers) on the right. Replaces five StatTiles that said the same
// things without being clickable. A level with a count of zero is not offered as a filter — a
// button that would empty the list is noise — except OK, which is how you hide the trouble.
import { computed } from 'vue'
import { StatusDot } from '@/components/ui/status-dot'
import { cn } from '@/lib/core/utils'
import { formatPct } from '@/lib/infra/hostStats'
import { FLEET_FILTERS, LEVEL_META, type FleetLevel, type FleetSummary } from '@/lib/infra/fleet'

const props = defineProps<{ summary: FleetSummary; filter: FleetLevel | null }>()
const emit = defineEmits<{ 'update:filter': [FleetLevel | null] }>()

const offered = computed(() => FLEET_FILTERS.filter((l) => props.summary.counts[l] > 0 || l === 'ok'))
function toggle(l: FleetLevel): void {
  emit('update:filter', props.filter === l ? null : l)
}
</script>

<template>
  <div class="flex flex-wrap items-stretch rounded-xl border border-border bg-card px-1.5 text-sm shadow-1" data-testid="infra-fleet-summary">
    <button type="button" :class="cn('my-1 flex items-center gap-2 rounded-lg px-3 py-1.5 transition-colors hover:bg-muted', !filter && 'bg-brand-soft hover:bg-brand-soft')" data-testid="fleet-filter" data-level="all" @click="emit('update:filter', null)">
      <span class="text-[15px] font-semibold tabular-nums text-foreground">{{ summary.total }}</span>
      <span class="text-muted-foreground">{{ summary.total === 1 ? 'host' : 'hosts' }}</span>
    </button>
    <button
      v-for="l in offered"
      :key="l"
      type="button"
      :class="cn('my-1 flex items-center gap-2 rounded-lg border-l border-border px-3 py-1.5 transition-colors hover:bg-muted', filter === l && 'bg-brand-soft hover:bg-brand-soft')"
      data-testid="fleet-filter"
      :data-level="l"
      :data-count="summary.counts[l]"
      @click="toggle(l)"
    >
      <StatusDot :tone="LEVEL_META[l].tone" size="md" />
      <span class="text-[15px] font-semibold tabular-nums" :class="summary.counts[l] && LEVEL_META[l].tone === 'error' ? 'text-sev-error' : summary.counts[l] && LEVEL_META[l].tone === 'warning' ? 'text-sev-warn' : 'text-foreground'">{{ summary.counts[l] }}</span>
      <span class="text-muted-foreground">{{ LEVEL_META[l].label }}</span>
    </button>
    <div class="ml-auto flex flex-wrap items-center gap-x-4 border-l border-border py-2.5 pl-4 pr-2.5 text-xs text-muted-foreground" data-testid="fleet-readings">
      <span>avg CPU <b class="font-medium tabular-nums text-foreground">{{ formatPct(summary.avgCpu) }}</b></span>
      <span>GPU <b class="font-medium tabular-nums text-foreground">{{ summary.gpuHosts }}</b> {{ summary.gpuHosts === 1 ? 'host' : 'hosts' }}</span>
      <span v-if="summary.containers">containers <b class="font-medium tabular-nums text-foreground">{{ summary.containers.running }} / {{ summary.containers.total }}</b></span>
    </div>
  </div>
</template>

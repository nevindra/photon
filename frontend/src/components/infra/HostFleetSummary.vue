<script setup lang="ts">
// The fleet in one line, and the line is the filter. "3 hosts · 1 critical · 1 warning · 1
// waiting" as a segmented control, then the readings that describe the fleet rather than judge it
// (avg CPU, GPU hosts, containers) on the right. Replaces five StatTiles that said the same
// things without being clickable. A level with a count of zero is not offered as a filter — a
// button that would empty the list is noise — except OK, which is how you hide the trouble.
//
// The counts sit on the app's one selected-segment idiom (`ui/segmented`: a muted track, the
// active segment raised as a white card) rather than a bespoke tinted pill with per-button
// dividers — the dividers cut straight through the pill's rounded corners and read as a broken
// border.
import { computed } from 'vue'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { StatusDot } from '@/components/ui/status-dot'
import { formatPct } from '@/lib/infra/hostStats'
import { FLEET_FILTERS, LEVEL_META, isFleetLevel, type FleetLevel, type FleetSummary } from '@/lib/infra/fleet'

const props = defineProps<{ summary: FleetSummary; filter: FleetLevel | null }>()
const emit = defineEmits<{ 'update:filter': [FleetLevel | null] }>()

const ALL = 'all'
const offered = computed(() => FLEET_FILTERS.filter((l) => props.summary.counts[l] > 0 || l === 'ok'))
const selected = computed(() => props.filter ?? ALL)

// A single-select toggle group reports the empty string when its active item is clicked again;
// both that and "all" mean no filter.
function select(value: unknown): void {
  emit('update:filter', typeof value === 'string' && isFleetLevel(value) ? value : null)
}

function countClass(l: FleetLevel): string {
  if (!props.summary.counts[l]) return 'text-foreground'
  if (LEVEL_META[l].tone === 'error') return 'text-sev-error'
  if (LEVEL_META[l].tone === 'warning') return 'text-sev-warn'
  return 'text-foreground'
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-border bg-card px-1.5 py-1 shadow-1" data-testid="infra-fleet-summary">
    <Segmented :model-value="selected" aria-label="Filter hosts by attention level" @update:model-value="select">
      <SegmentedItem :value="ALL" class="gap-1.5" data-testid="fleet-filter" data-level="all">
        <span class="text-[13px] font-semibold tabular-nums text-foreground">{{ summary.total }}</span>
        <span>{{ summary.total === 1 ? 'host' : 'hosts' }}</span>
      </SegmentedItem>
      <SegmentedItem
        v-for="l in offered"
        :key="l"
        :value="l"
        class="gap-1.5"
        data-testid="fleet-filter"
        :data-level="l"
        :data-count="summary.counts[l]"
      >
        <StatusDot :tone="LEVEL_META[l].tone" size="md" />
        <span class="text-[13px] font-semibold tabular-nums" :class="countClass(l)">{{ summary.counts[l] }}</span>
        <span>{{ LEVEL_META[l].label }}</span>
      </SegmentedItem>
    </Segmented>
    <div class="ml-auto flex flex-wrap items-center gap-x-4 px-1.5 text-xs text-muted-foreground" data-testid="fleet-readings">
      <span>avg CPU <b class="font-medium tabular-nums text-foreground">{{ formatPct(summary.avgCpu) }}</b></span>
      <span>GPU <b class="font-medium tabular-nums text-foreground">{{ summary.gpuHosts }}</b> {{ summary.gpuHosts === 1 ? 'host' : 'hosts' }}</span>
      <span v-if="summary.containers">containers <b class="font-medium tabular-nums text-foreground">{{ summary.containers.running }} / {{ summary.containers.total }}</b></span>
    </div>
  </div>
</template>

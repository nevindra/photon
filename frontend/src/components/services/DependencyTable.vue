<!-- frontend/src/components/services/DependencyTable.vue -->
<script setup>
// Dependency table for the service-detail Database/External panels. Rows come from
// `useServiceDependencies`: { name, system, count, error_count, rate, error_rate, p50, p95, p99 }
// (percentiles are ns STRINGS). Rows are clickable and emit `open-traces` so the view can pivot to
// the Traces explorer.
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { StatusDot } from '@/components/ui/status-dot'
import { formatDuration, formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

defineProps({
  title: { type: String, required: true },
  rows: { type: Array, default: () => [] },
})
const emit = defineEmits(['open-traces'])

function healthTone(row) {
  const er = row.error_rate ?? 0
  if ((row.error_count ?? 0) === 0 || er === 0) return 'neutral'
  return er < 0.05 ? 'warning' : 'error'
}
function fmtRate(r) {
  if (r == null) return '—'
  if (r >= 100) return Math.round(r).toLocaleString('en-US') + '/s'
  return r.toFixed(r >= 10 ? 1 : 2) + '/s'
}
function fmtErrorRate(r) {
  return ((r ?? 0) * 100).toFixed(2) + '%'
}
</script>

<template>
  <div class="flex flex-col overflow-hidden rounded-lg border border-border bg-card shadow-1">
    <header class="px-4 pb-2 pt-3.5">
      <h3 class="text-xs font-medium uppercase tracking-wider text-muted-foreground">{{ title }}</h3>
    </header>

    <Table v-if="rows.length" container-class="overflow-visible" class="border-collapse font-mono text-xs">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead class="w-6" aria-label="Health" />
          <TableHead>Name</TableHead>
          <TableHead class="text-right">Calls</TableHead>
          <TableHead class="text-right">Calls/s</TableHead>
          <TableHead class="text-right">Errors</TableHead>
          <TableHead class="text-right">p50</TableHead>
          <TableHead class="text-right">p95</TableHead>
          <TableHead class="text-right">p99</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="row in rows"
          :key="row.name + '::' + (row.system ?? '')"
          data-testid="dependency-row"
          role="button"
          tabindex="0"
          class="cursor-pointer border-border/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
          @click="emit('open-traces', row)"
          @keydown.enter.prevent="emit('open-traces', row)"
          @keydown.space.prevent="emit('open-traces', row)"
        >
          <TableCell class="py-1.5 pl-3 pr-0">
            <StatusDot :tone="healthTone(row)" />
          </TableCell>
          <TableCell class="py-1.5 text-foreground">
            {{ row.name }}
            <span v-if="row.system" class="ml-1.5 rounded border border-border px-1 py-px text-[10px] text-muted-foreground">{{ row.system }}</span>
          </TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatNumber(row.count ?? 0) }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ fmtRate(row.rate) }}</TableCell>
          <TableCell :class="cn('py-1.5 text-right tabular-nums', (row.error_count ?? 0) > 0 ? 'text-sev-error' : 'text-muted-foreground')">
            {{ fmtErrorRate(row.error_rate) }}
          </TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatDuration(Number(row.p50 ?? 0)) }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatDuration(Number(row.p95 ?? 0)) }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-foreground/80">{{ formatDuration(Number(row.p99 ?? 0)) }}</TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <EmptyState
      v-else
      title="No dependencies in range"
      description="Widen the time range to see downstream calls."
      class="h-auto py-8"
    />
  </div>
</template>

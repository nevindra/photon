<script setup>
import { computed, ref } from 'vue'
import { ArrowUp, ArrowDown } from 'lucide-vue-next'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { StatusDot } from '@/components/ui/status-dot'
import { Meter } from '@/components/ui/meter'
import { formatDuration, formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

// A small, client-sortable RED table (bounded rows — no virtualization needed, unlike TraceTable).
// Rows come from `api.red`: { service, operation, count, rate, error_count, error_rate, p50/p90/p99 }
// (percentiles are decimal-nanosecond STRINGS). Default sort = worst error rate first. Clicking a
// row pivots to that service/operation's exemplar traces (parent handles the navigation). Each row
// carries a health dot (tone by error rate) and an inline Rate meter scaled to the max rate shown.
const props = defineProps({
  rows: { type: Array, default: () => [] },
  group: { type: String, default: 'operation' }, // 'operation' | 'service'
  loading: { type: Boolean, default: false },
})
const emit = defineEmits(['open-exemplars'])

// Column key → accessor for sorting. Numeric columns sort on Number(); string columns on the raw
// string. Percentiles are strings, so Number() is applied (safe for realistic nanosecond durations).
const ACCESSORS = {
  service: (r) => r.service ?? '',
  operation: (r) => r.operation ?? '',
  rate: (r) => r.rate ?? 0,
  error_rate: (r) => r.error_rate ?? 0,
  p50: (r) => Number(r.p50 ?? 0),
  p90: (r) => Number(r.p90 ?? 0),
  p99: (r) => Number(r.p99 ?? 0),
}

const sortKey = ref('error_rate')
const sortDir = ref('desc') // 'asc' | 'desc'

function onSort(key) {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 'desc' ? 'asc' : 'desc'
  } else {
    sortKey.value = key
    sortDir.value = key === 'service' || key === 'operation' ? 'asc' : 'desc'
  }
}

const sortedRows = computed(() => {
  const acc = ACCESSORS[sortKey.value] ?? ACCESSORS.error_rate
  const dir = sortDir.value === 'asc' ? 1 : -1
  return [...props.rows].sort((a, b) => {
    const av = acc(a)
    const bv = acc(b)
    if (typeof av === 'string' || typeof bv === 'string') {
      return String(av).localeCompare(String(bv)) * dir
    }
    return (av - bv) * dir
  })
})

// Largest rate among the displayed rows — the inline Rate meter is scaled against it so the
// busiest row reads as a full bar. Floored at 1 to avoid divide-by-zero on an all-idle window.
const maxRate = computed(() => Math.max(1, ...props.rows.map((r) => r.rate ?? 0)))

// Health dot tone: neutral when error-free (no signal), warning below 5%, error at/above 5%.
function healthTone(row) {
  const er = row.error_rate ?? 0
  if ((row.error_count ?? 0) === 0 || er === 0) return 'neutral'
  return er < 0.05 ? 'warning' : 'error'
}

// Rate: compact "N/s" — whole numbers at scale, 1-2 decimals when small.
function fmtRate(r) {
  if (r == null) return '—'
  if (r >= 100) return formatNumber(Math.round(r)) + '/s'
  return r.toFixed(r >= 10 ? 1 : 2) + '/s'
}

// Error rate as a percentage; the cell is colored red when any error occurred.
function fmtErrorRate(r) {
  return (r * 100).toFixed(2) + '%'
}

function openExemplars(row) {
  emit('open-exemplars', { service: row.service, operation: row.operation ?? null })
}

const COLUMNS = computed(() =>
  [
    { key: 'service', label: 'Service', align: 'left' },
    props.group === 'operation' ? { key: 'operation', label: 'Operation', align: 'left' } : null,
    { key: 'rate', label: 'Rate', align: 'right' },
    { key: 'error_rate', label: 'Errors', align: 'right' },
    { key: 'p50', label: 'p50', align: 'right' },
    { key: 'p90', label: 'p90', align: 'right' },
    { key: 'p99', label: 'p99', align: 'right' },
  ].filter(Boolean),
)
</script>

<template>
  <div class="flex flex-col">
    <Table container-class="overflow-visible" class="border-collapse font-mono text-xs">
      <TableHeader class="sticky top-0 z-10 bg-background">
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead class="w-6" aria-label="Health" />
          <TableHead
            v-for="col in COLUMNS"
            :key="col.key"
            :data-testid="'sort-' + col.key"
            :class="cn('cursor-pointer select-none', col.align === 'right' && 'text-right')"
            @click="onSort(col.key)"
          >
            <span class="inline-flex items-center gap-1" :class="col.align === 'right' && 'flex-row-reverse'">
              {{ col.label }}
              <component
                :is="sortDir === 'desc' ? ArrowDown : ArrowUp"
                v-if="sortKey === col.key"
                class="size-3"
              />
            </span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="row in sortedRows"
          :key="row.service + '::' + (row.operation ?? '')"
          data-testid="red-row"
          :data-service="row.service"
          role="button"
          tabindex="0"
          class="cursor-pointer border-border/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
          @click="openExemplars(row)"
          @keydown.enter.prevent="openExemplars(row)"
          @keydown.space.prevent="openExemplars(row)"
        >
          <TableCell class="py-1.5 pl-3 pr-0">
            <StatusDot :tone="healthTone(row)" />
          </TableCell>
          <TableCell class="py-1.5 text-foreground">{{ row.service }}</TableCell>
          <TableCell v-if="group === 'operation'" data-testid="col-operation" class="py-1.5 text-muted-foreground">
            {{ row.operation ?? '—' }}
          </TableCell>
          <TableCell class="py-1.5">
            <div class="flex items-center gap-2">
              <Meter :value="(row.rate ?? 0) / maxRate" class="w-14 shrink-0" />
              <span class="ml-auto tabular-nums text-muted-foreground">{{ fmtRate(row.rate) }}</span>
            </div>
          </TableCell>
          <TableCell
            :class="cn('py-1.5 text-right tabular-nums', (row.error_count ?? 0) > 0 ? 'text-sev-error' : 'text-muted-foreground')"
          >
            {{ fmtErrorRate(row.error_rate) }}
          </TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatDuration(Number(row.p50 ?? 0)) }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatDuration(Number(row.p90 ?? 0)) }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-foreground/80">{{ formatDuration(Number(row.p99 ?? 0)) }}</TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <EmptyState
      v-if="!loading && !rows.length"
      title="No metrics in range"
      description="Widen the time range or clear a filter."
      class="h-auto flex-1"
    />
  </div>
</template>

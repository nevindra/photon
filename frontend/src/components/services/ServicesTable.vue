<script setup>
import { computed, ref } from 'vue'
import { ArrowUp, ArrowDown } from 'lucide-vue-next'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { Meter } from '@/components/ui/meter'
import ApdexBadge from './ApdexBadge.vue'
import HealthPill from './HealthPill.vue'
import { serviceHealth, byWorstFirst } from '@/lib/services/serviceHealth'
import { formatDuration, formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

// The Services (APM) list table. Rows come from `useServicesList` (`api.red(..., 'service')`):
// { service, count, rate, error_count, error_rate, p50/p90/p99 (ns STRINGS), apdex }. Default sort
// is health worst-first (critical → degraded → healthy → idle); other columns remain click-sortable.
// `prevRows` (previous window) drives a small error-rate trend chip. Clicking a row emits
// `open-service`; the parent owns navigation.
const props = defineProps({
  rows: { type: Array, default: () => [] },
  prevRows: { type: Array, default: () => [] },
  loading: { type: Boolean, default: false },
})
const emit = defineEmits(['open-service'])

const ACCESSORS = {
  service: (r) => r.service ?? '',
  count: (r) => r.count ?? 0,
  rate: (r) => r.rate ?? 0,
  error_rate: (r) => r.error_rate ?? 0,
  p99: (r) => Number(r.p99 ?? 0),
  apdex: (r) => (r.apdex == null ? -1 : r.apdex),
}

// 'health' is the default; sorted via the shared worst-first comparator rather than a scalar
// accessor, so ties break the same way everywhere (error_rate, then apdex, then rate).
const sortKey = ref('health')
const sortDir = ref('asc') // 'asc' = worst-first for health

function onSort(key) {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 'desc' ? 'asc' : 'desc'
  } else {
    sortKey.value = key
    sortDir.value = key === 'service' || key === 'health' ? 'asc' : 'desc'
  }
}

const sortedRows = computed(() => {
  if (sortKey.value === 'health') {
    const worst = byWorstFirst(props.rows)
    return sortDir.value === 'asc' ? worst : worst.reverse()
  }
  const acc = ACCESSORS[sortKey.value] ?? ACCESSORS.error_rate
  const dir = sortDir.value === 'asc' ? 1 : -1
  return [...props.rows].sort((a, b) => {
    const av = acc(a)
    const bv = acc(b)
    if (typeof av === 'string' || typeof bv === 'string') return String(av).localeCompare(String(bv)) * dir
    return (av - bv) * dir
  })
})

const maxRate = computed(() => Math.max(1, ...props.rows.map((r) => r.rate ?? 0)))

// Previous-window error_rate per service, for the trend chip.
const prevErrByService = computed(() =>
  Object.fromEntries(props.prevRows.map((r) => [r.service, r.error_rate ?? 0])),
)
// Signed fraction (cur - prev)/prev; null when no comparable previous value.
function errTrend(row) {
  const prev = prevErrByService.value[row.service]
  const cur = row.error_rate ?? 0
  if (prev == null || prev === 0 || !Number.isFinite(prev)) return null
  return (cur - prev) / prev
}

function fmtRate(r) {
  if (r == null) return '—'
  if (r >= 100) return formatNumber(Math.round(r)) + '/s'
  return r.toFixed(r >= 10 ? 1 : 2) + '/s'
}
function fmtErrorRate(r) {
  return ((r ?? 0) * 100).toFixed(2) + '%'
}
function openService(row) {
  emit('open-service', row.service)
}

const COLUMNS = [
  { key: 'health', label: 'Health', align: 'left' },
  { key: 'service', label: 'Service', align: 'left' },
  { key: 'count', label: 'Requests', align: 'right' },
  { key: 'rate', label: 'Rate', align: 'right' },
  { key: 'error_rate', label: 'Errors', align: 'right' },
  { key: 'p99', label: 'p99', align: 'right' },
  { key: 'apdex', label: 'Apdex', align: 'right' },
]
</script>

<template>
  <div class="flex flex-col rounded-lg border border-border bg-card shadow-1">
    <Table container-class="overflow-visible" class="border-collapse font-mono text-xs">
      <TableHeader class="sticky top-0 z-10 rounded-t-lg bg-card">
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead
            v-for="col in COLUMNS"
            :key="col.key"
            :data-testid="'sort-' + col.key"
            :class="cn('cursor-pointer select-none', col.align === 'right' && 'text-right')"
            @click="onSort(col.key)"
          >
            <span class="inline-flex items-center gap-1" :class="col.align === 'right' && 'flex-row-reverse'">
              {{ col.label }}
              <component :is="sortDir === 'desc' ? ArrowDown : ArrowUp" v-if="sortKey === col.key" class="size-3" />
            </span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="row in sortedRows"
          :key="row.service"
          data-testid="service-row"
          :data-service="row.service"
          role="button"
          tabindex="0"
          class="cursor-pointer border-border/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
          @click="openService(row)"
          @keydown.enter.prevent="openService(row)"
          @keydown.space.prevent="openService(row)"
        >
          <TableCell class="py-1.5" :title="serviceHealth(row).reasons.join(' · ')">
            <HealthPill :status="serviceHealth(row).status" />
          </TableCell>
          <TableCell class="py-1.5 text-foreground">{{ row.service }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatNumber(row.count ?? 0) }}</TableCell>
          <TableCell class="py-1.5">
            <div class="flex items-center gap-2">
              <Meter :value="(row.rate ?? 0) / maxRate" class="w-14 shrink-0" />
              <span class="ml-auto tabular-nums text-muted-foreground">{{ fmtRate(row.rate) }}</span>
            </div>
          </TableCell>
          <TableCell class="py-1.5 text-right tabular-nums">
            <span :class="(row.error_count ?? 0) > 0 ? 'text-sev-error' : 'text-muted-foreground'">{{ fmtErrorRate(row.error_rate) }}</span>
            <span
              v-if="errTrend(row) != null && Math.abs(errTrend(row)) >= 0.1"
              data-testid="err-trend"
              class="ml-1 text-[10px]"
              :class="errTrend(row) > 0 ? 'text-sev-error' : 'text-success'"
            >{{ errTrend(row) > 0 ? '▲' : '▼' }}{{ Math.abs(errTrend(row) * 100).toFixed(0) }}%</span>
          </TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-foreground/80">{{ formatDuration(Number(row.p99 ?? 0)) }}</TableCell>
          <TableCell class="py-1.5 text-right">
            <ApdexBadge :value="row.apdex" />
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <EmptyState
      v-if="!loading && !rows.length"
      title="No services in range"
      description="Widen the time range or clear a filter."
      class="h-auto flex-1"
    />
  </div>
</template>

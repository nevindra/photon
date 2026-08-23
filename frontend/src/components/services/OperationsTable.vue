<script setup lang="ts">
// The operations table. Same column vocabulary as the index's `ServicesTable`
// (`Health · Name · Traffic · Errors · Latency p99 · Apdex`) so one reading skill covers both — a
// table that renames the same numbers per page makes you re-learn it every time you drill in.
//
// The one column the index has and this does not is the window sparkline: it needs a bucketed read
// per operation, which the shared `/api/red` route does not return. Task 13's dedicated
// `/api/services/:service/operations` adds it.
import { computed, ref } from 'vue'
import { ArrowUp, ArrowDown } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { Meter } from '@/components/ui/meter'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import HealthPill from './HealthPill.vue'
import ApdexBandBar from './ApdexBandBar.vue'
import { serviceLevel, LEVEL_RANK } from '@/lib/services/serviceHealth'
import { OTHER_OPERATION } from '@/lib/services/servicesOverview'
import { formatCompact, formatDuration } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'
import type { RedRow } from '@/lib/core/api'

const props = withDefaults(
  defineProps<{ rows?: RedRow[]; loading?: boolean; windowSecs?: number }>(),
  { rows: () => [], loading: false, windowSecs: 1 },
)
const emit = defineEmits<{ open: [row: RedRow]; 'open-exemplars': [row: RedRow] }>()

const COLUMNS = [
  { key: 'health', label: 'Health', align: 'left' },
  { key: 'operation', label: 'Operation', align: 'left' },
  { key: 'count', label: 'Traffic', align: 'right' },
  { key: 'error_rate', label: 'Errors', align: 'right' },
  { key: 'p99', label: 'Latency p99', align: 'right' },
  { key: 'apdex', label: 'Apdex', align: 'right' },
] as const

const levelOf = (r: RedRow) => serviceLevel({ count: r.count, rate: r.rate, error_rate: r.error_rate, apdex: r.apdex ?? null }).level

const ACCESSORS: Record<string, (r: RedRow) => number | string> = {
  operation: (r) => r.operation ?? '',
  count: (r) => r.count ?? 0,
  error_rate: (r) => r.error_rate ?? 0,
  p99: (r) => Number(r.p99 ?? 0),
  apdex: (r) => (r.apdex == null ? -1 : r.apdex),
}

const sortKey = ref<string>('health')
const sortDir = ref<'asc' | 'desc'>('asc')

function onSort(key: string): void {
  if (sortKey.value === key) sortDir.value = sortDir.value === 'desc' ? 'asc' : 'desc'
  else {
    sortKey.value = key
    sortDir.value = key === 'operation' || key === 'health' ? 'asc' : 'desc'
  }
}

const sorted = computed<RedRow[]>(() => {
  const rows = [...props.rows]
  if (sortKey.value === 'health') {
    rows.sort((a, b) => {
      const d = LEVEL_RANK[levelOf(a)] - LEVEL_RANK[levelOf(b)]
      if (d !== 0) return d
      return (b.count ?? 0) - (a.count ?? 0)
    })
    return sortDir.value === 'asc' ? rows : rows.reverse()
  }
  const acc = ACCESSORS[sortKey.value] ?? ACCESSORS.count
  const dir = sortDir.value === 'asc' ? 1 : -1
  return rows.sort((a, b) => {
    const av = acc(a)
    const bv = acc(b)
    if (typeof av === 'string' || typeof bv === 'string') return String(av).localeCompare(String(bv)) * dir
    return (av - bv) * dir
  })
})

const maxCount = computed(() => Math.max(1, ...props.rows.map((r) => r.count ?? 0)))

const isOther = (r: RedRow) => r.operation === OTHER_OPERATION
// A non-entry operation's `count` is spans it recorded, not requests it served — the badge says so
// rather than letting an outbound database call read as inbound traffic (ADR-0003).
const isEntry = (r: RedRow) => (r.basis ?? 'entry') === 'entry'

function rateText(r: RedRow): string {
  const n = (r.count ?? 0) / Math.max(1, props.windowSecs)
  return (n >= 100 ? formatCompact(Math.round(n)) : n.toFixed(n >= 10 ? 1 : 2)) + '/s'
}
function open(r: RedRow): void {
  if (!isOther(r)) emit('open', r)
}
</script>

<template>
  <div class="flex flex-col overflow-hidden rounded-xl border border-border bg-card shadow-1">
    <Table container-class="overflow-visible" class="border-collapse font-mono text-xs">
      <TableHeader class="sticky top-0 z-10 bg-card">
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
          v-for="row in sorted"
          :key="row.operation ?? ''"
          data-testid="operation-row"
          :data-operation="row.operation"
          :data-level="levelOf(row)"
          :data-other="isOther(row) ? 'true' : 'false'"
          :role="isOther(row) ? undefined : 'button'"
          :tabindex="isOther(row) ? undefined : 0"
          :class="
            cn(
              'border-border/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring',
              isOther(row) ? 'cursor-default text-muted-foreground' : 'cursor-pointer',
            )
          "
          @click="open(row)"
          @keydown.enter.prevent="open(row)"
        >
          <TableCell class="py-1.5">
            <HealthPill :level="levelOf(row)" />
          </TableCell>

          <TableCell class="py-1.5 text-foreground">
            <span class="inline-flex flex-wrap items-center gap-1.5">
              <!-- `__other__` is a REMAINDER, not an endpoint: it gets a sentence, not a name you
                   could click into and expect to find something. -->
              <Tooltip v-if="isOther(row)">
                <TooltipTrigger as-child>
                  <span class="cursor-help italic">everything else</span>
                </TooltipTrigger>
                <TooltipContent class="max-w-72 text-xs">
                  Every operation outside this service's busiest, summed into one row by the rollup.
                  It is a remainder, not an endpoint — there is nothing to open.
                </TooltipContent>
              </Tooltip>
              <span v-else class="truncate">{{ row.operation }}</span>
              <Tooltip v-if="!isEntry(row) && !isOther(row)">
                <TooltipTrigger as-child>
                  <Badge variant="secondary" data-testid="not-entry-badge" class="cursor-help px-1.5 py-0 text-[10px]">
                    not an entry point
                  </Badge>
                </TooltipTrigger>
                <TooltipContent class="max-w-72 text-xs">
                  No server or consumer spans here, so this count is work the service DID (an outbound
                  call, an internal stage), not requests it served.
                </TooltipContent>
              </Tooltip>
            </span>
          </TableCell>

          <TableCell class="py-1.5">
            <div class="flex items-center justify-end gap-2">
              <Meter :value="(row.count ?? 0) / maxCount" class="w-14 shrink-0" />
              <div class="text-right">
                <div class="tabular-nums text-foreground/80">{{ rateText(row) }}</div>
                <div class="text-[10px] tabular-nums text-muted-foreground">{{ formatCompact(row.count ?? 0) }} total</div>
              </div>
            </div>
          </TableCell>

          <TableCell class="py-1.5 text-right">
            <div class="tabular-nums" :class="(row.error_count ?? 0) > 0 ? 'text-sev-error' : 'text-muted-foreground'">
              {{ ((row.error_rate ?? 0) * 100).toFixed(2) }}%
            </div>
            <div v-if="row.http_5xx || row.http_4xx" class="text-[10px] tabular-nums text-muted-foreground">
              <span v-if="row.http_5xx" class="text-sev-error/80">{{ formatCompact(row.http_5xx) }} 5xx</span>
              <span v-if="row.http_5xx && row.http_4xx"> · </span>
              <span v-if="row.http_4xx">{{ formatCompact(row.http_4xx) }} 4xx</span>
            </div>
          </TableCell>

          <TableCell class="py-1.5 text-right">
            <div class="tabular-nums text-foreground/80">{{ formatDuration(Number(row.p99 ?? 0)) }}</div>
            <div class="text-[10px] tabular-nums text-muted-foreground">
              {{ formatDuration(Number(row.p50 ?? 0)) }} / {{ formatDuration(Number(row.p90 ?? 0)) }}
            </div>
          </TableCell>

          <TableCell class="w-24 py-1.5">
            <div class="flex flex-col items-end gap-1">
              <span class="tabular-nums text-foreground/80">{{ row.apdex == null ? '—' : row.apdex.toFixed(2) }}</span>
              <ApdexBandBar :satisfied="row.satisfied" :tolerating="row.tolerating" :frustrated="row.frustrated" />
            </div>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <EmptyState
      v-if="!loading && !rows.length"
      title="No operations in range"
      description="Widen the time range or switch to All operations."
      class="h-auto flex-1"
    />
  </div>
</template>

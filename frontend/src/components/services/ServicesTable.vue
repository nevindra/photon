<script setup lang="ts">
// The Services index table: one dense row per `(service, environment)`, across every Site.
//
// Seven columns, ALL on the active Window: Health · Service · sparkline · Traffic · Errors ·
// Latency p99 · Apdex. No 30-day error budget, no cumulative data usage — a row that mixes three
// clocks with none of them labelled is how a table starts lying quietly (ADR-0004).
//
// Clicking a row EXPANDS it (per-Site children + a mini-dashboard); the service name is the link
// to the detail page. That split matters: the row's job on this page is comparison, and making the
// whole row navigate away meant you could never actually compare anything.
import { computed, ref } from 'vue'
import { ArrowUp, ArrowDown, ChevronRight } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { Meter } from '@/components/ui/meter'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import HealthPill from './HealthPill.vue'
import RowSparkline from './RowSparkline.vue'
import ApdexBandBar from './ApdexBandBar.vue'
import ServiceRowExpansion from './ServiceRowExpansion.vue'
import { LEVEL_RANK, reasonText } from '@/lib/services/serviceHealth'
import { compareRows, type FleetRow, type SiteSlice } from '@/lib/services/servicesOverview'
import { formatCompact, formatDuration } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

const props = withDefaults(
  defineProps<{
    rows?: FleetRow[]
    loading?: boolean
    /** Window length, for the requests/s denominator. */
    windowSecs?: number
    startNs?: string
    endNs?: string
  }>(),
  { rows: () => [], loading: false, windowSecs: 1, startNs: '0', endNs: '0' },
)
const emit = defineEmits<{
  'open-service': [row: FleetRow]
  'open-site': [slice: SiteSlice]
  'open-operation': [payload: { service: string; operation: string }]
}>()

const COLUMNS = [
  { key: 'health', label: 'Health', align: 'left' },
  { key: 'service', label: 'Service', align: 'left' },
  { key: 'spark', label: 'Window', align: 'left', sortable: false },
  { key: 'rate', label: 'Traffic', align: 'right' },
  { key: 'error_rate', label: 'Errors', align: 'right' },
  { key: 'p99', label: 'Latency p99', align: 'right' },
  { key: 'apdex', label: 'Apdex', align: 'right' },
] as const

// +1 for the chevron column.
const COLSPAN = COLUMNS.length + 1

const ACCESSORS: Record<string, (r: FleetRow) => number | string> = {
  service: (r) => r.service,
  rate: (r) => r.count,
  error_rate: (r) => r.error_rate,
  p99: (r) => r.p99,
  apdex: (r) => (r.apdex == null ? -1 : r.apdex),
}

const sortKey = ref<string>('health')
const sortDir = ref<'asc' | 'desc'>('asc') // 'asc' = worst-first for health

function onSort(key: string, sortable = true): void {
  if (!sortable) return
  if (sortKey.value === key) sortDir.value = sortDir.value === 'desc' ? 'asc' : 'desc'
  else {
    sortKey.value = key
    sortDir.value = key === 'service' || key === 'health' ? 'asc' : 'desc'
  }
}

const sortedRows = computed<FleetRow[]>(() => {
  if (sortKey.value === 'health') {
    const worst = [...props.rows].sort(compareRows)
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

// The Traffic meter is relative to the busiest row: absolute rates say nothing about which service
// carries the fleet, and that is the only thing a 56px bar can usefully show.
const maxCount = computed(() => Math.max(1, ...props.rows.map((r) => r.count)))

// One row open at a time. Two open expansions stack four extra queries and push the row you were
// comparing against off the screen — which is the opposite of what expanding is for.
const expanded = ref<string | null>(null)
function toggle(row: FleetRow): void {
  expanded.value = expanded.value === row.id ? null : row.id
}

function rateOf(row: FleetRow): number {
  return row.count / Math.max(1, props.windowSecs)
}
function rateText(row: FleetRow): string {
  const r = rateOf(row)
  return (r >= 100 ? formatCompact(Math.round(r)) : r.toFixed(r >= 10 ? 1 : 2)) + '/s'
}
function spreadText(row: FleetRow): string | null {
  const { unhealthy, total } = row.spread
  return total > 1 && unhealthy > 0 ? `${unhealthy} of ${total}` : null
}
/** Minutes since this row last reported anything, from the Silence reason. */
function reasonsTitle(row: FleetRow): string {
  return row.reasons.map(reasonText).join(' · ')
}
</script>

<template>
  <div class="flex flex-col overflow-hidden rounded-xl border border-border bg-card shadow-1">
    <Table container-class="overflow-visible" class="border-collapse font-mono text-xs">
      <TableHeader class="sticky top-0 z-10 bg-card">
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead class="w-6" aria-label="Expand" />
          <TableHead
            v-for="col in COLUMNS"
            :key="col.key"
            :data-testid="'sort-' + col.key"
            :class="
              cn(
                col.align === 'right' && 'text-right',
                (col as { sortable?: boolean }).sortable !== false && 'cursor-pointer select-none',
              )
            "
            @click="onSort(col.key, (col as { sortable?: boolean }).sortable !== false)"
          >
            <span class="inline-flex items-center gap-1" :class="col.align === 'right' && 'flex-row-reverse'">
              {{ col.label }}
              <component :is="sortDir === 'desc' ? ArrowDown : ArrowUp" v-if="sortKey === col.key" class="size-3" />
            </span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <template v-for="row in sortedRows" :key="row.id">
          <TableRow
            data-testid="service-row"
            :data-service="row.service"
            :data-level="row.level"
            :data-expanded="expanded === row.id ? 'true' : 'false'"
            role="button"
            tabindex="0"
            :aria-expanded="expanded === row.id"
            class="cursor-pointer border-border/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
            @click="toggle(row)"
            @keydown.enter.prevent="toggle(row)"
            @keydown.space.prevent="toggle(row)"
          >
            <TableCell class="w-6 py-1.5 pr-0 text-muted-foreground">
              <ChevronRight
                class="size-3.5 transition-transform"
                :class="expanded === row.id && 'rotate-90'"
              />
            </TableCell>

            <TableCell class="py-1.5" :title="reasonsTitle(row)">
              <HealthPill :level="row.level" />
            </TableCell>

            <TableCell class="py-1.5 text-foreground">
              <span class="inline-flex flex-wrap items-center gap-1.5">
                <!-- The NAME navigates; the row expands. -->
                <button
                  type="button"
                  data-testid="service-link"
                  class="font-medium hover:text-primary hover:underline"
                  @click.stop="emit('open-service', row)"
                >
                  {{ row.service }}
                </button>
                <Badge v-if="row.environment" variant="outline" class="px-1.5 py-0 text-[10px]">{{ row.environment }}</Badge>
                <Badge
                  v-if="spreadText(row)"
                  variant="warning"
                  data-testid="spread-badge"
                  class="px-1.5 py-0 text-[10px]"
                  title="How many of the reporting Sites this service is unhealthy in"
                >{{ spreadText(row) }} sites</Badge>
                <Badge v-if="row.incidents.length" variant="error" data-testid="incident-badge" class="px-1.5 py-0 text-[10px]">
                  {{ row.incidents.length }} firing
                </Badge>
                <Tooltip v-if="row.spanBased">
                  <TooltipTrigger as-child>
                    <Badge variant="secondary" data-testid="span-based-badge" class="cursor-help px-1.5 py-0 text-[10px]">span-based</Badge>
                  </TooltipTrigger>
                  <TooltipContent class="max-w-64 text-xs">
                    This service emits no server or consumer spans, so its request count is every span it
                    recorded — work done, not requests served.
                  </TooltipContent>
                </Tooltip>
              </span>
            </TableCell>

            <TableCell class="py-1.5">
              <RowSparkline :points="row.spark" :tone="row.level === 'ok' ? 'brand' : 'warning'" />
            </TableCell>

            <TableCell class="py-1.5">
              <div class="flex items-center justify-end gap-2">
                <Meter :value="row.count / maxCount" class="w-14 shrink-0" />
                <div class="text-right">
                  <div class="tabular-nums text-foreground/80">{{ rateText(row) }}</div>
                  <div class="text-[10px] tabular-nums text-muted-foreground">{{ formatCompact(row.span_count) }} spans</div>
                </div>
              </div>
            </TableCell>

            <TableCell class="py-1.5">
              <div class="flex items-center justify-end gap-2">
                <RowSparkline v-if="row.error_count" :points="row.error_spark" tone="error" :width="48" :height="18" />
                <div class="text-right">
                  <div
                    class="tabular-nums"
                    :class="row.error_count > 0 ? 'text-sev-error' : 'text-muted-foreground'"
                  >
                    {{ (row.error_rate * 100).toFixed(2) }}%
                  </div>
                  <div v-if="row.http_5xx || row.http_4xx" class="text-[10px] tabular-nums text-muted-foreground">
                    <span v-if="row.http_5xx" class="text-sev-error/80">{{ formatCompact(row.http_5xx) }} 5xx</span>
                    <span v-if="row.http_5xx && row.http_4xx"> · </span>
                    <span v-if="row.http_4xx">{{ formatCompact(row.http_4xx) }} 4xx</span>
                  </div>
                </div>
              </div>
            </TableCell>

            <TableCell class="py-1.5 text-right">
              <div class="tabular-nums text-foreground/80">{{ formatDuration(row.p99) }}</div>
              <div class="text-[10px] tabular-nums text-muted-foreground">
                {{ formatDuration(row.p50) }} / {{ formatDuration(row.p90) }}
              </div>
            </TableCell>

            <TableCell class="w-24 py-1.5">
              <div class="flex flex-col items-end gap-1">
                <span class="tabular-nums text-foreground/80">{{ row.apdex == null ? '—' : row.apdex.toFixed(2) }}</span>
                <ApdexBandBar
                  :satisfied="row.satisfied"
                  :tolerating="row.tolerating"
                  :frustrated="row.frustrated"
                />
              </div>
            </TableCell>
          </TableRow>

          <ServiceRowExpansion
            v-if="expanded === row.id"
            :row="row"
            :start-ns="startNs"
            :end-ns="endNs"
            :colspan="COLSPAN"
            @open-site="emit('open-site', $event)"
            @open-operation="emit('open-operation', { service: row.service, operation: $event })"
          />
        </template>
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

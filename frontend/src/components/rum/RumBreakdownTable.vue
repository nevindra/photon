<script setup>
// Hand-rolled sortable breakdown table (cloned from `services/ServicesTable.vue`) for RUM
// vitals-by-dimension rows: { key, pageviews, lcp_p75, inp_p75, cls_p75 } (from
// `useRumBreakdown`/`useRumPages` — `key` is the dimension value, e.g. a route or device
// type). `keyLabel` names the first column ("Route", "Device", …). LCP/INP/CLS cells are
// coloured by rating using the standard CWV thresholds (breakdown rows don't carry their
// own good_max/poor_min the way `/api/rum/vitals` does, so these are hardcoded here:
// LCP 2500/4000ms, INP 200/500ms, CLS 0.1/0.25).
import { computed, ref } from 'vue'
import { ArrowUp, ArrowDown } from 'lucide-vue-next'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  rows: { type: Array, default: () => [] },
  keyLabel: { type: String, default: 'Segment' },
  loading: { type: Boolean, default: false },
  // When set, rows become clickable (cursor + hover) and emit `row-click` with the row object —
  // used by the vitals (route dimension) + pages views to drill into `/rum/:appId/pages/:route`.
  clickable: { type: Boolean, default: false },
})

const emit = defineEmits(['row-click'])

const THRESHOLDS = {
  lcp_p75: { good: 2500, poor: 4000 },
  inp_p75: { good: 200, poor: 500 },
  cls_p75: { good: 0.1, poor: 0.25 },
}

function ratingFor(field, value) {
  if (value == null || !Number.isFinite(value)) return null
  const t = THRESHOLDS[field]
  if (!t) return null
  if (value <= t.good) return 'good'
  if (value <= t.poor) return 'needs'
  return 'poor'
}

const CELL_TONE = {
  good: 'text-success bg-success-soft',
  needs: 'text-sev-warn bg-sev-warn-soft',
  poor: 'text-sev-error bg-sev-error-soft',
}
function cellClass(field, value) {
  const r = ratingFor(field, value)
  return r ? CELL_TONE[r] : 'text-muted-foreground'
}

function fmtMs(value) {
  if (value == null || !Number.isFinite(value)) return '—'
  if (value >= 1000) return (value / 1000).toFixed(1) + 's'
  return Math.round(value) + 'ms'
}
function fmtCls(value) {
  return value == null || !Number.isFinite(value) ? '—' : value.toFixed(2)
}

const ACCESSORS = {
  key: (r) => String(r.key ?? ''),
  pageviews: (r) => r.pageviews ?? 0,
  lcp_p75: (r) => r.lcp_p75 ?? -1,
  inp_p75: (r) => r.inp_p75 ?? -1,
  cls_p75: (r) => r.cls_p75 ?? -1,
}

// Default sort is busiest-first (pageviews desc); other columns remain click-sortable.
const sortKey = ref('pageviews')
const sortDir = ref('desc')

function onSort(key) {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 'desc' ? 'asc' : 'desc'
  } else {
    sortKey.value = key
    sortDir.value = key === 'key' ? 'asc' : 'desc'
  }
}

const sortedRows = computed(() => {
  const acc = ACCESSORS[sortKey.value] ?? ACCESSORS.pageviews
  const dir = sortDir.value === 'asc' ? 1 : -1
  return [...props.rows].sort((a, b) => {
    const av = acc(a)
    const bv = acc(b)
    if (typeof av === 'string' || typeof bv === 'string') return String(av).localeCompare(String(bv)) * dir
    return (av - bv) * dir
  })
})

const COLUMNS = computed(() => [
  { key: 'key', label: props.keyLabel, align: 'left' },
  { key: 'pageviews', label: 'Pageviews', align: 'right' },
  { key: 'lcp_p75', label: 'LCP p75', align: 'right' },
  { key: 'inp_p75', label: 'INP p75', align: 'right' },
  { key: 'cls_p75', label: 'CLS p75', align: 'right' },
])
</script>

<template>
  <div class="flex flex-col">
    <Table container-class="overflow-visible" class="border-collapse font-mono text-xs">
      <TableHeader class="sticky top-0 z-10 bg-background">
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
          :key="row.key"
          data-testid="rum-breakdown-row"
          :data-key="row.key"
          :class="cn('border-border/60', clickable && 'cursor-pointer hover:bg-accent/40')"
          @click="clickable && emit('row-click', row)"
        >
          <TableCell class="py-1.5 text-foreground">{{ row.key }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatNumber(row.pageviews ?? 0) }}</TableCell>
          <TableCell class="py-1.5 text-right">
            <span
              :data-rating="ratingFor('lcp_p75', row.lcp_p75)"
              :class="cn('inline-flex items-center rounded-full px-2 py-0.5 tabular-nums', cellClass('lcp_p75', row.lcp_p75))"
            >{{ fmtMs(row.lcp_p75) }}</span>
          </TableCell>
          <TableCell class="py-1.5 text-right">
            <span
              :data-rating="ratingFor('inp_p75', row.inp_p75)"
              :class="cn('inline-flex items-center rounded-full px-2 py-0.5 tabular-nums', cellClass('inp_p75', row.inp_p75))"
            >{{ fmtMs(row.inp_p75) }}</span>
          </TableCell>
          <TableCell class="py-1.5 text-right">
            <span
              :data-rating="ratingFor('cls_p75', row.cls_p75)"
              :class="cn('inline-flex items-center rounded-full px-2 py-0.5 tabular-nums', cellClass('cls_p75', row.cls_p75))"
            >{{ fmtCls(row.cls_p75) }}</span>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <EmptyState
      v-if="!loading && !rows.length"
      title="No data in range"
      description="Widen the time range or clear a filter."
      class="h-auto flex-1"
    />
  </div>
</template>

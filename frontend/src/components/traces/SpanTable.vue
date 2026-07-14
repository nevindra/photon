<script setup>
import { computed, ref, watch } from 'vue'
import { useVueTable, getCoreRowModel } from '@tanstack/vue-table'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import { StatusPill } from '@/components/ui/status-pill'
import ColumnPicker from '@/components/common/ColumnPicker.vue'
import { Filter, Copy } from 'lucide-vue-next'
import { serviceColorClass } from '@/lib/services/serviceColor'
import { formatDuration, formatClock } from '@/lib/core/format'
import { pct } from '@/lib/traces/traceTree'
import { useTableColumns } from '@/lib/core/useTableColumns'
import { useCopy } from '@/lib/core/useCopy'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  spans: { type: Array, default: () => [] },
  loading: { type: Boolean, default: false },
  // Field catalog for the header ColumnPicker's addable list — [{ name, kind }], the same raw
  // shape TraceTable.vue consumes (what `useTracesFields` returns; kinds `fixed`/`promoted`/
  // `attribute`). Only `attribute`/`promoted` kinds are offered as addable columns (the `fixed`
  // ones are already covered by the built-ins below) — kept identical to TraceTable's transform
  // so a single catalog object can feed both tables.
  attrCatalog: { type: Array, default: () => [] },
  // Currently-selected span (from the waterfall / peek drawer) — matched against `span_id` to
  // highlight the row and scroll it into view. TraceTable.vue does the identical thing keyed on
  // `trace_id`.
  selectedId: { type: String, default: null },
})
const emit = defineEmits(['open-span', 'toggle-value'])

const { copy } = useCopy()

// Built-in columns, in display order. Widths drive the grid template below; `1fr` (Operation)
// is the only column that flexes with the container.
const BUILTINS = [
  { key: 'start', label: 'Start', width: '104px' },
  { key: 'service', label: 'Service', width: '140px' },
  { key: 'operation', label: 'Operation', width: '1fr' },
  { key: 'duration', label: 'Duration', width: '140px' },
  { key: 'status', label: 'Status', width: '90px' },
  { key: 'trace', label: 'Trace', width: '110px' },
]
// Reserved trailing track for the hover action cluster (Filter by service / Copy id).
const ACTIONS_WIDTH = '64px'
const BUILTIN_KEYS = new Set(BUILTINS.map((b) => b.key))

const { visibleKeys, attrColumns, toggleBuiltin, addAttr, removeAttr } = useTableColumns('spans', {
  builtins: BUILTINS,
})

const visibleBuiltins = computed(() => BUILTINS.filter((b) => visibleKeys.value.includes(b.key)))

// Addable attribute columns, derived from the raw `attrCatalog` the same way TraceTable.vue does:
// only `attribute`/`promoted` kinds, excluding anything that collides with a built-in key, mapped
// into the ColumnPicker's { key, label, group } shape.
const attrCatalogColumns = computed(() =>
  props.attrCatalog
    .filter((f) => (f.kind === 'attribute' || f.kind === 'promoted') && !BUILTIN_KEYS.has(f.name))
    .map((f) => ({ key: f.name, label: f.name, group: 'Attributes' })),
)

function attrLabel(key) {
  return attrCatalogColumns.value.find((f) => f.key === key)?.label ?? key
}

// Column-picker wiring: built-ins (tagged into their own group) plus the derived attribute
// catalog make up everything that can be toggled; `selected` is builtins currently shown union
// added attribute columns.
const columnPickerAvailable = computed(() => [
  ...BUILTINS.map((b) => ({ key: b.key, label: b.label, group: 'Columns' })),
  ...attrCatalogColumns.value,
])
const columnPickerSelected = computed(() => new Set([...visibleKeys.value, ...attrColumns.value]))
function toggleColumn(key) {
  if (BUILTINS.some((b) => b.key === key)) toggleBuiltin(key)
  else if (attrColumns.value.includes(key)) removeAttr(key)
  else addAttr(key)
}

const GRID_TEMPLATE_COLUMNS = computed(() => {
  const widths = visibleBuiltins.value.map((b) => b.width)
  const attrWidths = attrColumns.value.map(() => 'minmax(120px, 200px)')
  return [...widths, ...attrWidths, ACTIONS_WIDTH].join(' ')
})
const ROW_HEIGHT = 31 // matches the h-[31px] rows below (and the skeleton rows)

// vue-table drives the row model; bespoke per-cell markup (duration bar, service dot, status
// pill) reads straight off `row.original` in the template, same idiom as TraceTable/LogTable.
const columns = computed(() => [
  ...visibleBuiltins.value.map((b) => ({ accessorKey: b.key, header: b.label })),
  ...attrColumns.value.map((k) => ({ id: `attr:${k}`, accessorFn: (r) => r.attributes?.[k] })),
])
const table = useVueTable({
  get data() {
    return props.spans
  },
  get columns() {
    return columns.value
  },
  getCoreRowModel: getCoreRowModel(),
})
const rows = computed(() => {
  void columns.value
  return table.getRowModel().rows
})

// Virtualize the row list (fixed 31px rows) so a page of hundreds of spans only mounts the
// on-screen slice. The scroll element is measured via its offsetWidth/offsetHeight; a headless
// (jsdom) environment reports 0 unless the size is stubbed — see the table's tests.
const scrollEl = ref(null)
const rowVirtualizer = useVirtualizer(
  computed(() => ({
    count: rows.value.length,
    getScrollElement: () => scrollEl.value,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  })),
)
const visibleRows = computed(() =>
  rowVirtualizer.value
    .getVirtualItems()
    .map((vr) => ({ key: vr.key, start: vr.start, size: vr.size, span: rows.value[vr.index]?.original }))
    .filter((r) => r.span),
)
const totalSize = computed(() => rowVirtualizer.value.getTotalSize())

// Follow the selection: when the drawer/waterfall selects a span (by id), scroll it into view.
// A no-op when the id isn't on the current page (e.g. it's on a page not yet fetched).
watch(
  () => props.selectedId,
  (id) => {
    if (!id) return
    const i = props.spans.findIndex((s) => s.span_id === id)
    if (i >= 0) rowVirtualizer.value.scrollToIndex(i, { align: 'auto' })
  },
)

// Longest bar is pinned to the first page of a search and held stable thereafter, so bars don't
// re-scale (silently shrinking every already-rendered row) as later pages append shorter or
// longer spans — durations are only comparable within that first page (mirrors TraceTable's
// page-scoped mini-bar, but pinned instead of recomputed on every append). This table and
// TraceTable.vue are v-if/v-else siblings swapped without a remount on a query change (a facet
// narrow, a new search), so `pinnedMaxNs === 0n` alone can't distinguish a fresh page from an
// append — track the previous first row's id instead: the SAME first id with a longer list is an
// append (leave the pin alone); a DIFFERENT first id is a fresh page (recompute and re-pin).
const pinnedMaxNs = ref(0n)
let pinnedFirstId = null
watch(
  () => props.spans,
  (list) => {
    if (!list.length) return
    const firstId = list[0]?.span_id
    if (pinnedMaxNs.value === 0n || firstId !== pinnedFirstId) {
      pinnedMaxNs.value = list.reduce(
        (m, s) => (s.duration_nanos != null && s.duration_nanos > m ? s.duration_nanos : m),
        0n,
      )
      pinnedFirstId = firstId
    }
  },
  { immediate: true },
)
function durationPct(durationNs) {
  if (durationNs == null) return 0
  return Math.max(pct(durationNs, pinnedMaxNs.value), durationNs > 0n ? 1 : 0)
}

function statusTone(code) {
  if (code === 2) return 'error'
  if (code === 1) return 'success'
  return 'neutral'
}
function statusLabel(code) {
  if (code === 2) return 'Error'
  if (code === 1) return 'Ok'
  return 'Unset'
}

// Trace ids are long hex strings; show a short prefix with the full id available on hover/title.
function shortTraceId(id) {
  if (!id) return '—'
  return id.length > 10 ? id.slice(0, 8) + '…' : id
}

function openSpan(row) {
  emit('open-span', { traceId: row.trace_id, spanId: row.span_id, timeHintNs: row.start_time_nanos })
}
function filterByService(row) {
  emit('toggle-value', { field: 'service', value: row.service })
}
function copySpanId(row) {
  copy(row.span_id, 'span ID')
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div
      class="grid flex-none items-center gap-x-3 border-b border-border px-3 py-2 font-mono text-[10px] font-medium uppercase tracking-wider text-muted-foreground"
      :style="{ gridTemplateColumns: GRID_TEMPLATE_COLUMNS }"
    >
      <span v-for="b in visibleBuiltins" :key="b.key">{{ b.label }}</span>
      <span v-for="k in attrColumns" :key="k" class="truncate">{{ attrLabel(k) }}</span>
      <span class="flex justify-end">
        <ColumnPicker
          :available="columnPickerAvailable"
          :selected="columnPickerSelected"
          @toggle="toggleColumn"
        />
      </span>
    </div>

    <div
      ref="scrollEl"
      :class="cn('min-h-0 flex-1 overflow-y-auto', loading && !spans.length && 'opacity-50')"
    >
      <template v-if="loading && !spans.length">
        <div
          v-for="i in 8"
          :key="'skeleton-' + i"
          data-testid="span-row-skeleton"
          class="grid h-[31px] items-center gap-x-3 border-b border-border/60 px-3"
          :style="{ gridTemplateColumns: GRID_TEMPLATE_COLUMNS }"
        >
          <Skeleton
            v-for="i2 in visibleBuiltins.length + attrColumns.length"
            :key="i2"
            class="h-3 w-16 rounded-sm"
          />
          <span />
        </div>
      </template>

      <template v-else-if="spans.length">
        <!-- Virtualized rows: a full-height spacer with only the on-screen slice absolutely
             positioned inside it. -->
        <div :style="{ height: totalSize + 'px', width: '100%', position: 'relative' }">
          <div
            v-for="row in visibleRows"
            :key="row.key"
            data-testid="span-row"
            :data-span-id="row.span.span_id"
            role="button"
            tabindex="0"
            :class="
              cn(
                'group grid cursor-pointer items-center gap-x-3 border-b border-border/60 px-3 font-mono text-xs transition-colors duration-[var(--motion-base)] hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-inset motion-reduce:transition-none',
                row.span.span_id === selectedId && 'bg-muted',
              )
            "
            :style="{
              gridTemplateColumns: GRID_TEMPLATE_COLUMNS,
              position: 'absolute',
              top: 0,
              left: 0,
              width: '100%',
              height: row.size + 'px',
              transform: `translateY(${row.start}px)`,
            }"
            @click="openSpan(row.span)"
            @keydown.enter.prevent="openSpan(row.span)"
            @keydown.space.prevent="openSpan(row.span)"
          >
            <span
              v-if="row.span.status_code === 2"
              data-testid="span-error-accent"
              class="absolute inset-y-1 left-0 w-0.5 rounded-full bg-sev-error"
            />

            <template v-for="b in visibleBuiltins" :key="b.key">
              <span v-if="b.key === 'start'" class="truncate text-muted-foreground">
                {{ formatClock(row.span.start_time_nanos) }}
              </span>

              <span v-else-if="b.key === 'service'" class="flex min-w-0 items-center gap-1.5">
                <span
                  v-if="row.span.service"
                  :class="cn('size-2 shrink-0 rounded-full', serviceColorClass(row.span.service))"
                  :title="row.span.service"
                />
                <span class="truncate text-foreground">{{ row.span.service ?? '—' }}</span>
              </span>

              <span v-else-if="b.key === 'operation'" class="truncate text-foreground">
                {{ row.span.name ?? '—' }}
              </span>

              <span v-else-if="b.key === 'duration'" class="flex items-center gap-2">
                <span class="relative h-3 w-16 shrink-0 overflow-hidden rounded-sm bg-muted">
                  <span
                    data-testid="span-duration-bar"
                    class="absolute inset-y-0 left-0 rounded-sm bg-foreground/60"
                    :style="{ width: durationPct(row.span.duration_nanos) + '%' }"
                  />
                </span>
                <span class="shrink-0 truncate text-muted-foreground">{{ formatDuration(row.span.duration_nanos) }}</span>
              </span>

              <span v-else-if="b.key === 'status'">
                <StatusPill :tone="statusTone(row.span.status_code)">
                  {{ statusLabel(row.span.status_code) }}
                </StatusPill>
              </span>

              <span
                v-else-if="b.key === 'trace'"
                class="truncate text-muted-foreground"
                :title="row.span.trace_id"
              >
                {{ shortTraceId(row.span.trace_id) }}
              </span>
            </template>

            <span
              v-for="k in attrColumns"
              :key="k"
              class="truncate text-muted-foreground"
            >
              {{ row.span.attributes?.[k] ?? '—' }}
            </span>

            <!-- Right-aligned hover action cluster: hidden until the row is hovered or one of
                 its actions has keyboard focus. @click.stop keeps these from also opening the
                 row (they occupy the reserved trailing grid track, so they never overlap the
                 Trace column). -->
            <span
              class="flex items-center justify-end gap-1 opacity-0 transition-opacity motion-reduce:transition-none group-hover:opacity-100 group-focus-within:opacity-100"
              @click.stop
            >
              <button
                type="button"
                data-testid="filter-by-service"
                aria-label="Filter by service"
                title="Filter by service"
                class="rounded-sm p-1 text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                @click="filterByService(row.span)"
                @keydown.enter.stop.prevent="filterByService(row.span)"
                @keydown.space.stop.prevent="filterByService(row.span)"
              >
                <Filter class="size-3" />
              </button>
              <button
                type="button"
                data-testid="copy-span-id"
                aria-label="Copy span ID"
                title="Copy id"
                class="rounded-sm p-1 text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                @click="copySpanId(row.span)"
                @keydown.enter.stop.prevent="copySpanId(row.span)"
                @keydown.space.stop.prevent="copySpanId(row.span)"
              >
                <Copy class="size-3" />
              </button>
            </span>
          </div>
        </div>

        <!-- Infinite-scroll sentinel goes here (below the list, inside the scroll area) so the
             parent can observe it and fetch the next page as it scrolls into view. -->
        <slot name="footer" />
      </template>

      <EmptyState
        v-else
        title="No spans match"
        description="Widen the time range, clear a filter, or change your search."
      />
    </div>
  </div>
</template>

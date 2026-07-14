<script setup>
import { computed, onMounted, ref, watch } from 'vue'
import { useVueTable, getCoreRowModel } from '@tanstack/vue-table'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { Filter, Copy } from 'lucide-vue-next'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import ColumnPicker from '@/components/common/ColumnPicker.vue'
import { serviceColorClass } from '@/lib/services/serviceColor'
import { formatDuration, formatClock, formatNumber } from '@/lib/core/format'
import { pct } from '@/lib/traces/traceTree'
import { useTableColumns } from '@/lib/core/useTableColumns'
import { useCopy } from '@/lib/core/useCopy'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  traces: { type: Array, default: () => [] },
  loading: { type: Boolean, default: false },
  // Field catalog for the header ColumnPicker's addable list — [{ name, kind }], same shape
  // `useTracesFields` returns. Only `attribute`/`promoted` kinds are offered (the `fixed` ones are
  // already covered by the built-ins below).
  attrCatalog: { type: Array, default: () => [] },
  // trace_id of the row the peek drawer currently has open — highlighted (`bg-muted`) and kept
  // in view (scrollToIndex) as prev/next moves it, so the list visibly tracks the drawer's cursor.
  selectedId: { type: String, default: null },
})
const emit = defineEmits(['open-trace', 'toggle-value', 'columns-changed'])

// Built-in columns: key/label/width, in display order. Width lookup stays local to this file —
// it's a rendering concern `useTableColumns` doesn't need to know about.
const BUILTINS = [
  { key: 'start_ts', label: 'Start', width: '104px' },
  { key: 'root_service', label: 'Service', width: '140px' },
  { key: 'root_name', label: 'Root operation', width: '1fr' },
  { key: 'duration_ns', label: 'Duration', width: '170px' },
  { key: 'span_count', label: 'Spans', width: '64px' },
  { key: 'error_count', label: 'Errors', width: '64px' },
  { key: 'services', label: 'Services', width: '90px' },
]
const BUILTIN_WIDTH = Object.fromEntries(BUILTINS.map((b) => [b.key, b.width]))
const BUILTIN_LABEL = Object.fromEntries(BUILTINS.map((b) => [b.key, b.label]))
const BUILTIN_KEYS = new Set(BUILTINS.map((b) => b.key))
const ATTR_COL_WIDTH = '140px'
const ACTIONS_COL_WIDTH = '68px'

const { visibleKeys, attrColumns, toggleBuiltin, addAttr, removeAttr } = useTableColumns('traces', {
  builtins: BUILTINS.map(({ key, label }) => ({ key, label })),
})

// T10 learns which attribute keys to request from the backend off this event — the attribute
// column list IS the set of `columns` the trace search needs to fetch, so rows can carry
// `root_attributes` for them.
watch(attrColumns, (keys) => emit('columns-changed', keys))
// ...and also on mount: after a reload, `useTableColumns` seeds `attrColumns` from localStorage
// synchronously, but the watch above is non-immediate, so a parent that only learns the attribute
// set from `columns-changed` would never (re-)request the persisted columns until the user
// manually re-toggled one. Emit once with whatever came out of localStorage as soon as we're
// mounted (not synchronously in setup — emitting before the parent's listener is wired up can
// warn / be missed).
onMounted(() => emit('columns-changed', attrColumns.value))

// Addable columns for the header picker: built-ins (always toggleable) + attribute/promoted
// fields from the catalog, minus any that collide with a built-in key.
const availableColumns = computed(() => [
  ...BUILTINS.map((b) => ({ key: b.key, label: b.label, group: 'Built-in' })),
  ...props.attrCatalog
    .filter((f) => (f.kind === 'attribute' || f.kind === 'promoted') && !BUILTIN_KEYS.has(f.name))
    .map((f) => ({ key: f.name, label: f.name, group: 'Attributes' })),
])
const selectedColumns = computed(() => new Set([...visibleKeys.value, ...attrColumns.value]))
function onToggleColumn(key) {
  if (BUILTIN_KEYS.has(key)) toggleBuiltin(key)
  else if (attrColumns.value.includes(key)) removeAttr(key)
  else addAttr(key)
}

// Grid template driven by whichever built-ins are visible + any attribute columns + a fixed
// trailing slot for the per-row hover actions. Inline style (not a static Tailwind class) so the
// same rule drives both the header and every row — same idiom as LogTable's gridTemplateColumns.
const GRID_TEMPLATE_COLUMNS = computed(() =>
  [
    ...visibleKeys.value.map((k) => BUILTIN_WIDTH[k] ?? '120px'),
    ...attrColumns.value.map(() => ATTR_COL_WIDTH),
    ACTIONS_COL_WIDTH,
  ].join(' '),
)
const ROW_HEIGHT = 31 // matches the h-[31px] rows below (and the skeleton rows)

const { copy } = useCopy()
function copyTraceId(traceId) {
  copy(traceId, 'trace ID')
}

// vue-table drives the row model. Server order is preserved (no sorting model registered — the
// backend already returns rows in the requested `sort` order). We read `row.original` for the
// bespoke per-cell markup (duration bar, service dot, coloured error count) rather than
// FlexRender, which only pays off for plain-text cells.
const columns = BUILTINS.map((b) => ({ accessorKey: b.key, header: b.label }))
const table = useVueTable({
  get data() {
    return props.traces
  },
  columns,
  getCoreRowModel: getCoreRowModel(),
})
const rows = computed(() => table.getRowModel().rows)

// Virtualize the row list (fixed 31px rows) so a page of hundreds of traces only mounts the
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
// Pair each virtual item with its trace so the template stays terse.
const visibleRows = computed(() =>
  rowVirtualizer.value
    .getVirtualItems()
    .map((vr) => ({ key: vr.key, start: vr.start, size: vr.size, trace: rows.value[vr.index]?.original }))
    .filter((r) => r.trace),
)
const totalSize = computed(() => rowVirtualizer.value.getTotalSize())

// Keep the peek drawer's selection in view: when selectedId changes (prev/next in the drawer),
// scroll the virtualized list so that row is visible, mirroring the drawer's cursor.
watch(
  () => props.selectedId,
  (id) => {
    if (!id) return
    const i = props.traces.findIndex((t) => t.trace_id === id)
    if (i >= 0) rowVirtualizer.value.scrollToIndex(i, { align: 'auto' })
  },
)

// Longest bar on the first page of a search pins 100% width for the rest of that search's
// lifetime — recomputing over the whole (growing) array on every page load would make an
// already-rendered bar visibly shrink once a later page brings in a shorter max, so an appended
// page must NOT reset the pin. But this table and SpanTable.vue are v-if/v-else siblings swapped
// without a remount on a query change (a facet narrow, a new search), so `pinnedMaxNs === 0n`
// alone can't distinguish a fresh page from an append. Track the previous first row's id instead:
// the SAME first id with a longer list is an append (leave the pin alone); a DIFFERENT first id
// is a fresh page (recompute and re-pin).
const pinnedMaxNs = ref(0n)
let pinnedFirstId = null
watch(
  () => props.traces,
  (list) => {
    if (!list.length) return
    const firstId = list[0]?.trace_id
    if (pinnedMaxNs.value === 0n || firstId !== pinnedFirstId) {
      let max = 0n
      for (const t of list) {
        if (t.duration_ns != null && t.duration_ns > max) max = t.duration_ns
      }
      pinnedMaxNs.value = max
      pinnedFirstId = firstId
    }
  },
  { immediate: true },
)

// Percent width for the duration bar, floored so a non-zero duration stays visible. Falls back to
// the row's own duration as the reference (100%) if no page has produced a pinned max yet (e.g.
// every duration on the first page was null).
function durationPct(durationNs) {
  if (durationNs == null) return 0
  const max = pinnedMaxNs.value > 0n ? pinnedMaxNs.value : durationNs
  return Math.max(pct(durationNs, max), durationNs > 0n ? 1 : 0)
}

function openTrace(row) {
  emit('open-trace', { traceId: row.trace_id, timeHintNs: String(row.start_ts) })
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div
      class="grid flex-none items-center gap-x-3 border-b border-border px-3 py-2 font-mono text-[10px] font-medium uppercase tracking-wider text-muted-foreground"
      :style="{ gridTemplateColumns: GRID_TEMPLATE_COLUMNS }"
    >
      <span v-for="k in visibleKeys" :key="k" class="truncate">{{ BUILTIN_LABEL[k] }}</span>
      <span v-for="k in attrColumns" :key="k" class="truncate normal-case tracking-normal">{{ k }}</span>
      <span class="flex justify-end">
        <ColumnPicker :available="availableColumns" :selected="selectedColumns" @toggle="onToggleColumn" />
      </span>
    </div>

    <div
      ref="scrollEl"
      :class="cn('min-h-0 flex-1 overflow-y-auto', loading && !traces.length && 'opacity-50')"
    >
      <template v-if="loading && !traces.length">
        <div
          v-for="i in 8"
          :key="'skeleton-' + i"
          data-testid="trace-row-skeleton"
          class="grid h-[31px] items-center gap-x-3 border-b border-border/60 px-3"
          :style="{ gridTemplateColumns: GRID_TEMPLATE_COLUMNS }"
        >
          <Skeleton v-for="n in visibleKeys.length + attrColumns.length" :key="n" class="h-3 w-16 rounded-sm" />
          <span />
        </div>
      </template>

      <template v-else-if="traces.length">
        <!-- Virtualized rows: a full-height spacer with only the on-screen slice absolutely
             positioned inside it. -->
        <div :style="{ height: totalSize + 'px', width: '100%', position: 'relative' }">
          <div
            v-for="row in visibleRows"
            :key="row.key"
            data-testid="trace-row"
            :data-trace-id="row.trace.trace_id"
            role="button"
            tabindex="0"
            :class="
              cn(
                'group grid cursor-pointer items-center gap-x-3 border-b border-border/60 px-3 font-mono text-xs transition-colors duration-[var(--motion-base)] hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-inset',
                row.trace.trace_id === selectedId && 'bg-muted',
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
            @click="openTrace(row.trace)"
            @keydown.enter.prevent="openTrace(row.trace)"
            @keydown.space.prevent="openTrace(row.trace)"
          >
            <span
              v-if="(row.trace.error_count ?? 0) > 0"
              data-testid="trace-error-accent"
              class="absolute inset-y-1 left-0 w-0.5 rounded-full bg-sev-error"
            />

            <template v-for="k in visibleKeys" :key="k">
              <span v-if="k === 'start_ts'" class="truncate text-muted-foreground">{{ formatClock(row.trace.start_ts) }}</span>

              <span v-else-if="k === 'root_service'" class="flex min-w-0 items-center gap-1.5">
                <span
                  v-if="row.trace.root_service"
                  :class="cn('size-2 shrink-0 rounded-full', serviceColorClass(row.trace.root_service))"
                  :title="row.trace.root_service"
                />
                <span class="truncate text-foreground">{{ row.trace.root_service ?? '—' }}</span>
              </span>

              <span v-else-if="k === 'root_name'" class="truncate text-foreground">{{ row.trace.root_name ?? '—' }}</span>

              <span v-else-if="k === 'duration_ns'" class="flex items-center gap-2">
                <span class="relative h-3 w-16 shrink-0 overflow-hidden rounded-sm bg-muted">
                  <span
                    data-testid="duration-bar-fill"
                    class="absolute inset-y-0 left-0 rounded-sm bg-foreground/60"
                    :style="{ width: durationPct(row.trace.duration_ns) + '%' }"
                  />
                </span>
                <span class="shrink-0 truncate text-muted-foreground">{{ formatDuration(row.trace.duration_ns) }}</span>
              </span>

              <span v-else-if="k === 'span_count'" class="text-muted-foreground">{{ formatNumber(row.trace.span_count ?? 0) }}</span>

              <span
                v-else-if="k === 'error_count'"
                data-testid="error-count"
                :class="(row.trace.error_count ?? 0) > 0 ? 'text-sev-error' : 'text-muted-foreground'"
              >
                {{ formatNumber(row.trace.error_count ?? 0) }}
              </span>

              <span
                v-else-if="k === 'services'"
                class="truncate text-muted-foreground"
                :title="(row.trace.services ?? []).join(', ')"
              >
                {{ (row.trace.services ?? []).length }}
              </span>
            </template>

            <span
              v-for="k in attrColumns"
              :key="k"
              class="truncate text-muted-foreground"
              :title="row.trace.root_attributes?.[k] ?? ''"
            >{{ row.trace.root_attributes?.[k] ?? '—' }}</span>

            <span
              class="flex items-center justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
            >
              <button
                type="button"
                data-testid="action-filter-service"
                title="Filter by service"
                aria-label="Filter by service"
                class="rounded p-1 text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                @click.stop="emit('toggle-value', { field: 'service', value: row.trace.root_service })"
                @keydown.enter.stop.prevent="emit('toggle-value', { field: 'service', value: row.trace.root_service })"
                @keydown.space.stop.prevent="emit('toggle-value', { field: 'service', value: row.trace.root_service })"
              >
                <Filter class="size-3.5" />
              </button>
              <button
                type="button"
                data-testid="action-copy-id"
                title="Copy id"
                aria-label="Copy trace id"
                class="rounded p-1 text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                @click.stop="copyTraceId(row.trace.trace_id)"
                @keydown.enter.stop.prevent="copyTraceId(row.trace.trace_id)"
                @keydown.space.stop.prevent="copyTraceId(row.trace.trace_id)"
              >
                <Copy class="size-3.5" />
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
        title="No traces match"
        description="Widen the time range, clear a filter, or change your search."
      />
    </div>
  </div>
</template>

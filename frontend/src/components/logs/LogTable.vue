<script setup>
import { computed, ref, nextTick, watch } from 'vue'
import { useVueTable, getCoreRowModel } from '@tanstack/vue-table'
import { useVirtualizer } from '@tanstack/vue-virtual'
import SeverityTag from '@/components/logs/SeverityTag.vue'
import EmptyState from '@/components/ui/empty-state/EmptyState.vue'
import { severity, severityClasses, formatClock } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  rows: { type: Array, default: () => [] },
  selectedId: { type: [Number, String, null], default: null },
  loading: { type: Boolean, default: false },
  columns: { type: Array, default: () => [] }, // attribute column names to show after message
})
const emit = defineEmits(['select', 'filter-severity', 'scroll-top-change'])

// The scrollable viewport the virtualizer measures + windows within.
const scrollRef = ref(null)

// Pause-on-scroll signal for live tail (LogsView pauses/resumes prepend off this): a small pixel
// tolerance treats "basically at the top" as at-top so a couple of px of rubber-banding/rounding
// doesn't spuriously read as "scrolled away".
const AT_TOP_PX = 4
function onScroll() {
  emit('scroll-top-change', (scrollRef.value?.scrollTop ?? 0) <= AT_TOP_PX)
}

// The fixed time/level/service/message columns, plus one flexible-width track per configured
// attribute column. Tailwind's static `grid-cols-[...]` utility can't express a runtime-variable
// column count, so the template columns are driven by an inline style instead.
const gridTemplateColumns = computed(() => {
  const extra = props.columns.map(() => 'minmax(120px, 200px)').join(' ')
  return `104px 70px 130px 1fr${extra ? ' ' + extra : ''}`
})

// TanStack Table column defs: the four fixed columns plus one dynamic column per attribute in the
// `columns` prop. The cells themselves are rendered by the template below (they carry SeverityTag +
// click handlers, so a plain cell renderer won't do); these defs give the table a row model to
// virtualize over and keep column identity aligned with the grid tracks.
const columnDefs = computed(() => [
  { id: 'time', accessorFn: (r) => r.timestamp },
  { id: 'level', accessorFn: (r) => r.severity },
  { id: 'service', accessorFn: (r) => r.service },
  { id: 'message', accessorFn: (r) => r.body },
  ...props.columns.map((c) => ({ id: `attr:${c}`, accessorFn: (r) => r.attributes?.[c] ?? '' })),
])

const table = useVueTable({
  get data() {
    return props.rows
  },
  get columns() {
    return columnDefs.value
  },
  getRowId: (row) => String(row.id),
  getCoreRowModel: getCoreRowModel(),
})

// Reactive row model. Touching props.rows / columnDefs here guarantees the computed re-evaluates
// when they change; getCoreRowModel then re-derives (the core model is a 1:1 passthrough of data).
const tableRows = computed(() => {
  void props.rows
  void columnDefs.value
  return table.getRowModel().rows
})

// Row windowing: only the rows near the viewport are kept in the DOM. Rows are a fixed 31px tall
// (Tailwind border-box makes `h-[31px]` the full box height incl. border), so a constant estimate
// is exact — no per-row measurement needed.
const rowVirtualizer = useVirtualizer(
  computed(() => ({
    count: tableRows.value.length,
    getScrollElement: () => scrollRef.value,
    estimateSize: () => 31,
    overscan: 10,
  })),
)

const totalSize = computed(() => rowVirtualizer.value.getTotalSize())
// Flatten each virtual slot to { key, start, row } so the template stays readable. Guard against a
// transient index past the current row model (during data swaps).
const virtualRows = computed(() =>
  rowVirtualizer.value
    .getVirtualItems()
    .map((v) => ({ key: v.key, start: v.start, row: tableRows.value[v.index]?.original }))
    .filter((v) => v.row != null),
)

function selectRow(id) {
  emit('select', id)
}

function currentIndex() {
  return props.rows.findIndex((r) => r.id === props.selectedId)
}

function moveSelection(delta) {
  if (!props.rows.length) return
  const idx = currentIndex()
  const next =
    idx === -1
      ? delta > 0
        ? 0
        : props.rows.length - 1
      : Math.min(props.rows.length - 1, Math.max(0, idx + delta))
  emit('select', props.rows[next].id)
}

function onKeydown(e) {
  if (e.key === 'ArrowDown' || e.key === 'j') {
    e.preventDefault()
    moveSelection(1)
  } else if (e.key === 'ArrowUp' || e.key === 'k') {
    e.preventDefault()
    moveSelection(-1)
  } else if (e.key === 'Enter') {
    const idx = currentIndex()
    if (idx !== -1) {
      e.preventDefault()
      emit('select', props.rows[idx].id)
    }
  }
}

// Keep the selected row on screen. The virtualizer's scrollToIndex replaces scrollIntoView — an
// off-screen row isn't in the DOM to scroll to, so we drive the viewport by index instead.
watch(
  () => props.selectedId,
  async (id) => {
    if (id == null) return
    const idx = props.rows.findIndex((r) => r.id === id)
    if (idx === -1) return
    await nextTick()
    rowVirtualizer.value.scrollToIndex(idx)
  },
)
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div
      class="grid flex-none gap-x-3 border-b border-border px-3 py-2 font-mono text-[10px] font-medium uppercase tracking-wider text-muted-foreground"
      :style="{ gridTemplateColumns }"
    >
      <span>Time</span>
      <span>Level</span>
      <span>Service</span>
      <span>Message</span>
      <span v-for="c in columns" :key="c" class="truncate">{{ c }}</span>
    </div>

    <div
      ref="scrollRef"
      tabindex="0"
      role="listbox"
      :class="cn('min-h-0 flex-1 overflow-y-auto outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset', loading && 'opacity-50')"
      @keydown="onKeydown"
      @scroll="onScroll"
    >
      <template v-if="rows.length">
        <!-- Spacer sized to the full virtual height; windowed rows are absolutely positioned inside. -->
        <div :style="{ height: totalSize + 'px', width: '100%', position: 'relative' }">
          <div
            v-for="v in virtualRows"
            :key="v.row.id"
            :data-row-id="v.row.id"
            role="option"
            :aria-selected="v.row.id === selectedId"
            :class="cn(
              'absolute left-0 top-0 grid h-[31px] w-full items-center gap-x-3 border-b border-border/60 px-3 font-mono text-xs cursor-pointer',
              v.row.id === selectedId ? 'bg-muted' : 'hover:bg-muted/50',
            )"
            :style="{ gridTemplateColumns, transform: `translateY(${v.start}px)` }"
            @click="selectRow(v.row.id)"
          >
            <span
              v-if="severity(v.row.severity).tone !== 'neutral'"
              :class="[severityClasses(v.row.severity).solid, 'absolute inset-y-1 left-0 w-0.5 rounded-full']"
            />
            <span class="truncate text-muted-foreground">{{ formatClock(v.row.timestamp) }}</span>
            <span class="cursor-pointer" @click.stop="emit('filter-severity', v.row.severity)">
              <SeverityTag :level="v.row.severity" />
            </span>
            <span class="truncate text-foreground/80">{{ v.row.service }}</span>
            <span class="truncate text-foreground">{{ v.row.body }}</span>
            <span v-for="c in columns" :key="c" class="truncate text-muted-foreground">{{ v.row.attributes?.[c] ?? '' }}</span>
          </div>
        </div>
      </template>

      <EmptyState
        v-else-if="!loading"
        title="No logs match"
        description="Widen the time range, clear a filter, or change your search."
      />
    </div>
  </div>
</template>

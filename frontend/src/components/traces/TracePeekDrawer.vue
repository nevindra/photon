<script setup>
import { computed, ref, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { PeekDrawer } from '@/components/ui/peek-drawer'
import { SheetTitle } from '@/components/ui/sheet'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import { StatusDot } from '@/components/ui/status-dot'
import { Copy, ScrollText } from 'lucide-vue-next'
import { useTrace } from '@/lib/traces/tracesQueries'
import { getTraceTree, pct } from '@/lib/traces/traceTree'
import { useCopy } from '@/lib/core/useCopy'
import { formatDuration, formatFull, formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

// Compact, triage-first preview of a trace — lead with duration + error state and the ACTUAL
// error text, not a count. No deep inspection here (that's "Open full view", which hands off to
// the full TraceDetailView/waterfall). Rebuilt on the shared `ui/peek-drawer` primitive, which
// owns the Sheet shell + focus-trap-safe prev/next keyboard machinery; this component only
// supplies identity, the header, and the body.
const props = defineProps({
  traceId: { type: String, default: '' },
  spanId: { type: String, default: null },
  timeHintNs: { type: [String, Number], default: undefined },
  open: { type: Boolean, default: false },
  index: { type: Number, default: -1 },
  total: { type: Number, default: 0 },
})
const emit = defineEmits(['close', 'open-full', 'prev', 'next', 'view-logs'])

// Latch the drawer's identity instead of deriving the fetch id straight from `props.open`.
// `PeekDrawer`'s `SheetContent` stays mounted (`has-content="!!traceId"`, the latched id) through
// Reka's close animation, mirroring LogDetailDrawer — but LogDetailDrawer's `row` prop is
// naturally stable through close (its parent doesn't null the selection on close, only on next
// pick). This drawer's props are the *live* caller-controlled id/span, and the parent nulls them
// on close (`@close="drawer = null"`). Collapsing the fetch id to '' the instant `open` flips
// false would switch `useTrace` to its disabled/empty state mid-fade, flashing the empty "Trace
// not found" state over the still-visible drawer. So: latch `{ traceId, spanId, timeHintNs }` here
// whenever the drawer is genuinely open with a real id (covers first open, the id/span changing
// while already open, AND prev/next — the parent re-points `traceId`/`spanId` while `open` stays
// true, so the latch re-points and the header updates while the new trace fetches); leave it
// untouched while closed/closing so the last-shown trace rides out the animation. Nothing here
// needs to gate the fetch on open — `useTrace`'s own `enabled` guard (tracesQueries.js) already
// disables the query whenever the (latched) id is falsy, which is also what keeps this component
// from fetching anything before its first real open (identity starts at `null`).
const identity = ref(null) // { traceId, spanId, timeHintNs } | null
watch(
  () => [props.open, props.traceId, props.spanId, props.timeHintNs],
  ([open, traceId, spanId, timeHintNs]) => {
    if (open && traceId) identity.value = { traceId, spanId, timeHintNs }
  },
  { immediate: true },
)

// Shadow the same-named props in the template with the latched values, so everything the drawer
// renders/matches against (header id, span highlight) tracks `identity`, not the live props.
const traceId = computed(() => identity.value?.traceId ?? '')
const spanId = computed(() => identity.value?.spanId ?? null)

const traceQuery = useTrace(traceId, computed(() => identity.value?.timeHintNs))

const isOpen = computed(() => props.open && !!props.traceId)
const loading = computed(() => traceQuery.isFetching.value)
const spans = computed(() => traceQuery.data.value?.spans ?? [])
const hasSpans = computed(() => spans.value.length > 0)
// getTraceTree is buildTrace memoized off the spans ARRAY reference (see traceTree.js) — the peek
// drawer, the waterfall, and the detail view all build from the same TanStack Query cache entry,
// so this avoids re-walking the same trace when several of them are mounted at once.
const trace = computed(() => getTraceTree(spans.value))

// Read-only span list: errored spans first, then longest-running first.
const sortedRows = computed(() => {
  const nodes = [...trace.value.nodes.values()]
  return nodes.sort((a, b) => {
    if (a.isError !== b.isError) return a.isError ? -1 : 1
    if (a.durationNs !== b.durationNs) return a.durationNs > b.durationNs ? -1 : 1
    return 0
  })
})

// Primary error = the first node in the errors-first `sortedRows` sort that `.isError`; its
// actual failure text drives the header callout.
const primaryError = computed(() => sortedRows.value.find((n) => n.isError) ?? null)

// Virtualize the sorted span list — a trace can carry hundreds of spans, and the drawer
// previously `v-for`d over all of them into a 480px sheet. Same pattern as TraceTable.vue /
// TraceWaterfall.vue: fixed-estimate rows, a full-height spacer, only the on-screen slice
// mounted. The scroll element is measured via its offsetWidth/offsetHeight; a headless (jsdom)
// environment reports 0 unless the size is stubbed — see this component's tests.
const scrollEl = ref(null)
const ROW_HEIGHT = 40 // matches the row's padding/line-height below (single-line rows)
// Errored rows with a status_message render a second line (the `peek-span-error-msg` <p> below) —
// a flat ROW_HEIGHT estimate here undersizes those rows and lets the next row's absolutely-
// positioned slot overlap it. Index-aware: only the specific rows that actually render the extra
// line get the taller estimate.
const ROW_HEIGHT_WITH_MESSAGE = 56
function estimateRowSize(index) {
  const node = sortedRows.value[index]
  return node?.isError && node?.span?.status_message ? ROW_HEIGHT_WITH_MESSAGE : ROW_HEIGHT
}
const rowVirtualizer = useVirtualizer(
  computed(() => ({
    count: sortedRows.value.length,
    getScrollElement: () => scrollEl.value,
    estimateSize: estimateRowSize,
    overscan: 12,
  })),
)
// Pair each virtual item with its node so the template stays terse.
const visibleRows = computed(() =>
  rowVirtualizer.value
    .getVirtualItems()
    .map((vr) => ({ key: vr.key, start: vr.start, size: vr.size, node: sortedRows.value[vr.index] }))
    .filter((r) => r.node),
)
const totalSize = computed(() => rowVirtualizer.value.getTotalSize())

// Virtualization means a pre-focused span may not be in the initially-rendered window (unlike the
// old full `v-for`, which always mounted it) — nudge the virtualizer to include its index; the
// actual scrollIntoView still happens from `setRowRef` once Vue mounts that row.
function scrollVirtualToSpan(id) {
  const idx = sortedRows.value.findIndex((n) => n.id === id)
  if (idx !== -1) rowVirtualizer.value.scrollToIndex(idx, { align: 'center' })
}

function barStyle(node) {
  const total = trace.value.durationNs
  const left = pct(node.offsetNs, total)
  const width = Math.max(pct(node.durationNs, total), 1)
  return { left: `${left}%`, width: `${Math.min(width, 100 - left)}%` }
}

const { copy } = useCopy()
function copyTraceId() {
  copy(traceId.value, 'trace ID')
}

function openFull() {
  emit('open-full', { traceId: traceId.value, spanId: spanId.value })
}

function viewLogs() {
  emit('view-logs', { traceId: traceId.value, timeHintNs: identity.value?.timeHintNs })
}

// PeekDrawer forwards every non-nav keydown while open; this drawer's own one-key jumps live here.
function onShortcut(e) {
  if (e.key === 'o') {
    e.preventDefault()
    openFull()
  } else if (e.key === 'l') {
    e.preventDefault()
    viewLogs()
  }
}

function onUpdateOpen(val) {
  if (!val) emit('close')
}

// Pre-focus: scroll the row matching `spanId` into view. The Sheet's content is teleported and
// its enter transition can delay the actual DOM insertion past this component's own onMounted, so
// scrolling from a post-mount hook isn't reliable — instead, trigger it straight from the `:ref`
// callback the moment Vue actually mounts *that* row (covers first paint, and trace data arriving
// after an initial loading state). A `watch` on `[open, spanId, sortedRows]` (immediate, so it also
// covers first mount) handles the rest: the row already being mounted and the caller changing
// which span to focus without a new row mounting, PLUS — now that the list is virtualized — the
// target row not being in the initial/current window at all (`scrollVirtualToSpan` nudges it in;
// `setRowRef` then does the precise scroll once Vue actually mounts it).
const rowEls = new Map()
function setRowRef(id, el) {
  if (el) {
    rowEls.set(id, el)
    if (props.open && id === spanId.value) el.scrollIntoView?.({ block: 'center' })
  } else {
    rowEls.delete(id)
  }
}
watch(
  () => [props.open, spanId.value, sortedRows.value],
  ([open, id]) => {
    if (!open || !id) return
    scrollVirtualToSpan(id)
    rowEls.get(id)?.scrollIntoView?.({ block: 'center' })
  },
  { immediate: true },
)
// Clicking the error callout jumps the span row into view — same mechanism as the spanId
// pre-focus (bring it into the virtual window, then scrollIntoView once it's mounted).
function scrollToSpan(id) {
  scrollVirtualToSpan(id)
  rowEls.get(id)?.scrollIntoView?.({ block: 'center' })
}
</script>

<template>
  <PeekDrawer
    :open="isOpen"
    :has-content="!!traceId"
    :index="index"
    :total="total"
    :width="480"
    @update:open="onUpdateOpen"
    @prev="emit('prev')"
    @next="emit('next')"
    @shortcut="onShortcut"
  >
    <template #header>
      <SheetTitle class="font-mono text-xs">
        <span v-if="hasSpans" data-testid="peek-stat-root">
          <span class="text-foreground">{{ trace.rootService }}</span>
          <span class="text-muted-foreground"> · {{ trace.rootName }}</span>
        </span>
        <span v-else class="text-muted-foreground">Trace {{ traceId.slice(0, 8) }}…</span>
      </SheetTitle>

      <!-- Headline: lead with the two triage questions — how slow, and did it error. -->
      <div v-if="hasSpans" class="mt-1.5 flex items-baseline gap-2.5">
        <span
          data-testid="peek-stat-duration"
          class="font-mono text-2xl font-semibold tabular-nums text-foreground"
        >
          {{ formatDuration(trace.durationNs) }}
        </span>
        <span
          v-if="trace.errorCount > 0"
          data-testid="peek-stat-errors"
          class="inline-flex items-center rounded-full bg-sev-error-soft px-2 py-0.5 font-mono text-[11px] font-medium text-sev-error"
        >
          {{ trace.errorCount }} errors
        </span>
      </div>

      <!-- Primary error callout: the ACTUAL failure text, mirroring SpanDetailPanel's Alert. -->
      <Alert
        v-if="primaryError"
        variant="error"
        data-testid="peek-error-callout"
        role="button"
        tabindex="0"
        class="mt-3 cursor-pointer text-left"
        @click="scrollToSpan(primaryError.id)"
        @keydown.enter.prevent="scrollToSpan(primaryError.id)"
      >
        <AlertDescription class="space-y-0.5 font-mono text-xs">
          <span class="block text-foreground">
            {{ primaryError.span.service }} · {{ primaryError.span.name }}
          </span>
          <span class="block">
            {{ primaryError.span.status_text ?? primaryError.span.status_code
            }}<template v-if="primaryError.span.status_message"> — {{ primaryError.span.status_message }}</template>
          </span>
          <span v-if="trace.errorCount > 1" class="block text-[11px] text-sev-error/70">
            +{{ trace.errorCount - 1 }} more
          </span>
        </AlertDescription>
      </Alert>

      <!-- Actions: copy id + one-key jumps to the waterfall (o) and to logs (l). -->
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <button
          type="button"
          data-testid="copy-trace-id"
          class="flex items-center gap-1.5 rounded-md bg-muted px-2 py-1 font-mono text-[11px] text-muted-foreground transition-colors hover:text-foreground"
          title="Copy trace ID"
          @click="copyTraceId"
        >
          {{ traceId.slice(0, 12) }}…
          <Copy class="size-3" />
        </button>
        <Button data-testid="open-full" variant="outline" size="sm" @click="openFull">
          Open full view →
        </Button>
        <Button data-testid="view-logs" variant="outline" size="sm" @click="viewLogs">
          <ScrollText class="mr-1 size-3.5" />
          View logs
        </Button>
      </div>
    </template>

    <div class="px-6 py-4">
      <div v-if="loading" class="space-y-3" data-testid="peek-skeleton">
        <Skeleton class="h-6 w-40" />
        <Skeleton class="h-3.5 w-full" />
        <Skeleton class="h-3.5 w-full" />
        <Skeleton class="h-3.5 w-full" />
      </div>

      <EmptyState
        v-else-if="!hasSpans"
        title="Trace not found"
        description="This trace has no spans, or it could not be loaded."
      />

      <template v-else>
        <!-- Compact stat strip — duration + errors moved up into the headline. -->
        <div
          class="flex flex-wrap gap-x-8 gap-y-3 border-b border-border pb-4 text-xs"
          data-testid="peek-summary"
        >
          <div data-testid="peek-stat-spans">
            <span class="block text-[10px] uppercase tracking-wider text-muted-foreground">Spans</span>
            <span class="font-mono text-foreground">{{ formatNumber(trace.spanCount) }}</span>
          </div>
          <div data-testid="peek-stat-services">
            <span class="block text-[10px] uppercase tracking-wider text-muted-foreground">Services</span>
            <span class="font-mono text-foreground">{{ formatNumber(trace.serviceCount) }}</span>
          </div>
          <div data-testid="peek-stat-started">
            <span class="block text-[10px] uppercase tracking-wider text-muted-foreground">Started</span>
            <span class="font-mono text-foreground">{{ formatFull(trace.startNs) }}</span>
          </div>
        </div>

        <!-- Virtualized span list: a full-height spacer with only the on-screen slice absolutely
             positioned inside it (same structure as TraceTable.vue's virtual body). The container
             gets its own bounded scroll region (rather than relying on the outer PeekDrawer
             ScrollArea, whose Radix-managed viewport isn't reachable from here) so the virtualizer
             has a real, measurable scroll element. -->
        <div ref="scrollEl" class="mt-4 max-h-[50vh] overflow-y-auto" data-testid="peek-span-list">
          <div :style="{ height: totalSize + 'px', width: '100%', position: 'relative' }">
            <div
              v-for="row in visibleRows"
              :key="row.key"
              :ref="(el) => setRowRef(row.node.id, el)"
              data-testid="peek-span-row"
              :data-span-id="row.node.id"
              :data-selected="row.node.id === spanId ? 'true' : 'false'"
              :class="
                cn(
                  'rounded-md px-2 py-1.5',
                  row.node.id === spanId
                    ? 'bg-accent'
                    : row.node.isError
                      ? 'bg-sev-error/10'
                      : 'hover:bg-muted/60',
                )
              "
              :style="{
                position: 'absolute',
                top: 0,
                left: 0,
                width: '100%',
                height: row.size + 'px',
                transform: `translateY(${row.start}px)`,
              }"
            >
              <div class="flex items-center gap-2">
                <StatusDot :tone="row.node.isError ? 'error' : 'neutral'" size="sm" />
                <span class="w-36 shrink-0 truncate font-mono text-xs text-foreground">
                  {{ row.node.span.service }} · {{ row.node.span.name }}
                </span>
                <div class="relative h-2 flex-1 rounded-sm bg-muted">
                  <span
                    class="absolute inset-y-0 rounded-sm"
                    :class="row.node.isError ? 'bg-sev-error' : 'bg-foreground/70'"
                    :style="barStyle(row.node)"
                  />
                </div>
                <span class="w-14 shrink-0 text-right font-mono text-[11px] text-muted-foreground">
                  {{ formatDuration(row.node.durationNs) }}
                </span>
              </div>
              <p
                v-if="row.node.isError && row.node.span.status_message"
                data-testid="peek-span-error-msg"
                class="mt-1 truncate pl-4 font-mono text-[11px] text-sev-error"
                :title="row.node.span.status_message"
              >
                {{ row.node.span.status_message }}
              </p>
            </div>
          </div>
        </div>
      </template>
    </div>
  </PeekDrawer>
</template>

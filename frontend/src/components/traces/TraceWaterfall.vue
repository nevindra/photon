<script setup>
import { computed, nextTick, ref, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { ChevronRight, ChevronDown, ChevronUp } from 'lucide-vue-next'
import { useElementSize } from '@vueuse/core'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Label } from '@/components/ui/label'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { StatusDot } from '@/components/ui/status-dot'
import TraceMinimap from '@/components/traces/TraceMinimap.vue'
import { getTraceTree, pct } from '@/lib/traces/traceTree'
import { serviceColorClass } from '@/lib/services/serviceColor'
import { formatDuration } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  spans: { type: Array, default: () => [] },
  selectedSpanId: { type: [String, null], default: null },
  collapseHealthy: { type: Boolean, default: false },
  // Optional span id to select + reveal + centre on mount (and whenever it changes to a
  // non-empty value) — powers deep-linking from `/traces/:id?span=<id>` (see TraceDetailView)
  // and T10's "Open full view" jump. Unknown/absent ids no-op cleanly.
  initialSpanId: { type: [String, null], default: null },
  // Optional prebuilt trace tree (from getTraceTree). When the parent already built the tree
  // (e.g. TraceDetailView), pass it here so the waterfall reuses it instead of rebuilding.
  // Absent → build (memoised) from `spans`.
  tree: { type: Object, default: null },
})
const emit = defineEmits(['select-span', 'update:collapseHealthy'])

const trace = computed(() => props.tree ?? getTraceTree(props.spans))

const ROW_HEIGHT = 28 // matches the h-7 rows below

// Shared column split (label column | bar-track column) so the axis ticks, the gridlines, and
// every row all measure their 0-100% span identically — the "bar-track origin" the ticks and
// gridlines are aligned to.
const GRID_TEMPLATE_COLUMNS = 'minmax(200px, 320px) 1fr'

// Filter: case-insensitive substring match on service OR operation name. Non-matches are DIMMED,
// not removed, so the tree shape stays legible — but matches are also navigable (see below).
const filterText = ref('')
// Pure: does this node's service or operation name contain the (already-lowercased) query `q`?
function nodeMatches(node, q) {
  const svc = (node.span.service ?? '').toLowerCase()
  const name = (node.span.name ?? '').toLowerCase()
  return svc.includes(q) || name.includes(q)
}
function matchesFilter(node) {
  const q = filterText.value.trim().toLowerCase()
  if (!q) return true
  return nodeMatches(node, q)
}

// Match set = every span matching the filter across the WHOLE tree (`trace.flat`), in pre-order,
// so matches inside collapsed / healthy-hidden subtrees are still reachable via n/N.
const matches = computed(() => {
  if (!filterText.value.trim()) return []
  return trace.value.flat.filter(matchesFilter).map((n) => n.id)
})
const matchTotal = computed(() => matches.value.length)
// Which match n/N is parked on. Reset (and re-revealed) whenever the filter text changes.
const currentMatchIndex = ref(0)

// Set form of `matches`, for O(1) per-row lookups. This is intentionally its OWN computed — kept
// separate from `openRows`' geometry pass below — so typing in the filter box only re-derives this
// small id Set (cheap: a substring scan + Set build) instead of recomputing bar/self-time/event-
// marker geometry for every open row. `nodeMatches` is pure, so a node's match result is the same
// whether it's evaluated here (over the whole tree) or against just the open rows — this Set is a
// safe lookup source for both the per-row dim flag and the minimap's match markers.
const matchIds = computed(() => new Set(matches.value))
// A row "matches" when there's no active filter (nothing is dimmed) or its id is in `matchIds`.
function isRowMatch(id) {
  return !filterText.value.trim() || matchIds.value.has(id)
}

// Ancestor ids of a node (excluding itself), walking parentId up via the node map. Cycle-safe.
function ancestorIdsOf(id) {
  const nodes = trace.value.nodes
  const result = []
  const seen = new Set([id])
  let parentId = nodes.get(id)?.parentId ?? null
  while (parentId != null && !seen.has(parentId)) {
    seen.add(parentId)
    const parent = nodes.get(parentId)
    if (!parent) break
    result.push(parent.id)
    parentId = parent.parentId ?? null
  }
  return result
}

// When collapsing healthy branches, keep roots, error subtrees, and the critical path — PLUS,
// when a filter is active, every matched node and its full ancestor chain, so a healthy match
// isn't hidden. (Non-matches still merely dim; this only affects which rows survive collapse.)
const rows = computed(() => {
  const t = trace.value
  if (!props.collapseHealthy) return t.flat
  const keepMatched = new Set()
  if (filterText.value.trim()) {
    for (const id of matches.value) {
      keepMatched.add(id)
      for (const a of ancestorIdsOf(id)) keepMatched.add(a)
    }
  }
  return t.flat.filter(
    (n) => n.depth === 0 || n.subtreeHasError || n.onCriticalPath || keepMatched.has(n.id),
  )
})

// Index of every node's position in `rows`, used to find each node's descendants (a contiguous
// run of deeper entries immediately following it — valid because `rows` preserves the tree's
// pre-order, and collapse-healthy never removes an ancestor of a node it keeps).
const rowsIndex = computed(() => {
  const m = new Map()
  rows.value.forEach((n, i) => m.set(n.id, i))
  return m
})

function descendantCount(node) {
  const list = rows.value
  const idx = rowsIndex.value.get(node.id)
  if (idx == null) return 0
  let count = 0
  for (let i = idx + 1; i < list.length; i++) {
    if (list[i].depth <= node.depth) break
    count++
  }
  return count
}

// Per-node collapse: ids of spans whose subtree is hidden. Reactive Set, replaced (not mutated
// in place) on every change so the computed below re-derives.
const collapsed = ref(new Set())

function setCollapsed(id, value) {
  const next = new Set(collapsed.value)
  if (value) next.add(id)
  else next.delete(id)
  collapsed.value = next
}
function toggleCollapse(id) {
  setCollapsed(id, !collapsed.value.has(id))
}
function onChevronClick(event, id) {
  event.stopPropagation() // don't let the click bubble up to the row's select-span handler
  toggleCollapse(id)
}
function expandAll() {
  collapsed.value = new Set()
}
function collapseAll() {
  const next = new Set()
  for (const n of trace.value.flat) if (n.children.length) next.add(n.id)
  collapsed.value = next
}

// `rows` (collapse-healthy applied) with any node that has a collapsed ancestor dropped. Safe to
// walk with a simple depth threshold for the same pre-order reason `descendantCount` relies on.
//
// Each surviving node is materialised into a plain row object carrying its precomputed per-row
// geometry (bar left/width %, self-time insets, event markers) and collapsed descendant count.
// Computing these ONCE here (keyed on spans/trace/collapse — deliberately NOT on filterText, see
// `matchIds`/`isRowMatch` above) — instead of calling barStyle/selfTimeInsets/eventMarkers/
// descendantCount inline per visible row per scroll frame — is the waterfall's dominant-jank fix;
// the template just reads the fields. `descendantCount` is only walked for rows that actually show
// the "+N" badge (collapsed parents) — it's an O(subtree) scan, so doing it unconditionally for
// every row would make this pass up to O(n²).
const openRows = computed(() => {
  const t = trace.value
  const list = rows.value
  const result = []
  let hideDepth = null
  for (const n of list) {
    if (hideDepth !== null) {
      if (n.depth > hideDepth) continue
      hideDepth = null
    }
    const { leftPct, widthPct } = computeBar(n, t)
    const isCollapsedParent = n.children.length && collapsed.value.has(n.id)
    result.push({
      ...n,
      barLeftPct: leftPct,
      barWidthPct: widthPct,
      selfInsets: computeSelfInsets(n),
      eventMarkers: computeEventMarkers(n),
      descendantCount: isCollapsedParent ? descendantCount(n) : 0,
    })
    if (isCollapsedParent) hideDepth = n.depth
  }
  return result
})

// Count of matching rows among the currently-visible ones (for the "N of M spans" label). Reads
// `isRowMatch` (filterText-dependent) against the already-computed `openRows` list, so a filter
// keystroke recomputes this cheap reduce, never the geometry pass above.
const matchCount = computed(() => openRows.value.reduce((n, r) => n + (isRowMatch(r.id) ? 1 : 0), 0))

// Virtualize the (possibly deep) span-rows list — only the on-screen slice mounts.
const scrollEl = ref(null)
const rowVirtualizer = useVirtualizer(
  computed(() => ({
    count: openRows.value.length,
    getScrollElement: () => scrollEl.value,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  })),
)
const visibleRows = computed(() =>
  rowVirtualizer.value
    .getVirtualItems()
    .map((vr) => ({ key: vr.key, start: vr.start, size: vr.size, node: openRows.value[vr.index] }))
    .filter((r) => r.node),
)
const totalSize = computed(() => rowVirtualizer.value.getTotalSize())

// --- match-jump navigation ---
// Reveal a matched span: clear per-node collapse on ALL its ancestors (so it can't be hidden),
// select it, and centre it in the viewport once the row list has re-rendered.
function revealMatch(id) {
  const ancestors = ancestorIdsOf(id)
  if (ancestors.length) {
    const next = new Set(collapsed.value)
    let changed = false
    for (const a of ancestors) if (next.delete(a)) changed = true
    if (changed) collapsed.value = next
  }
  emit('select-span', id)
  nextTick(() => {
    const idx = openRows.value.findIndex((n) => n.id === id)
    if (idx !== -1) rowVirtualizer.value.scrollToIndex(idx, { align: 'center' })
  })
}
// Step to the next (+1) / previous (-1) match, wrapping, and reveal it.
function goToMatch(direction) {
  const list = matches.value
  if (!list.length) return
  const len = list.length
  const idx = (((currentMatchIndex.value + direction) % len) + len) % len
  currentMatchIndex.value = idx
  revealMatch(list[idx])
}
// Typing in the filter auto-selects the first match so `n` starts somewhere sensible.
watch(filterText, () => {
  currentMatchIndex.value = 0
  const list = matches.value
  if (list.length) revealMatch(list[0])
})

// --- initial-selection deep link ---
// Reveal + select + centre `initialSpanId` on mount and on every subsequent change to a
// non-empty value (e.g. navigating `/traces/:id?span=<id>` while this instance stays mounted).
// Reuses `revealMatch`'s ancestor-clearing + scroll-to-centre logic. No-ops cleanly when the id
// is absent or doesn't exist in this trace.
watch(
  () => props.initialSpanId,
  (id) => {
    if (!id) return
    if (!trace.value.nodes.has(id)) return
    revealMatch(id)
  },
  { immediate: true },
)

// --- minimap scroll tracking ---
// Only rendered past MINIMAP_THRESHOLD rows (small traces stay uncluttered).
const MINIMAP_THRESHOLD = 50
// True when the right-edge minimap is shown. The minimap narrows the rows' bar-track by its own
// width, so the axis above reserves a matching-width spacer (see template) to keep ticks aligned
// with the gridlines/bars below.
const showMinimap = computed(() => openRows.value.length > MINIMAP_THRESHOLD)
const scrollTop = ref(0)
const { height: viewportHeight } = useElementSize(scrollEl)
function onRowsScroll(event) {
  scrollTop.value = event.target.scrollTop
}
function onMinimapScrollTo(px) {
  if (scrollEl.value) scrollEl.value.scrollTop = px
  scrollTop.value = px
}

// Axis ticks (5 segments) with duration labels.
const ticks = computed(() => {
  const total = trace.value.durationNs
  return Array.from({ length: 6 }, (_, i) => ({
    leftPct: (i / 5) * 100,
    label: formatDuration((total * BigInt(i)) / 5n),
  }))
})

function barClass(node) {
  return node.isError ? 'bg-sev-error' : serviceColorClass(node.span.service)
}

// Bar left/width as % of the full trace width. A floor keeps sub-pixel spans visible.
// Pure — takes the node + trace tree, returns numeric percentages; folded into `openRows` once
// (per recompute) so the template reads static fields instead of recomputing this per frame.
function computeBar(node, t) {
  const left = pct(node.offsetNs, t.durationNs)
  const width = Math.max(pct(node.durationNs, t.durationNs), 0.5)
  return { leftPct: left, widthPct: Math.min(width, 100 - left) }
}

// Self-time insets: the union of child-covered intervals (already clamped + merged by
// buildTrace), positioned within the BAR's own local space so the remaining solid slivers read
// as the span's own self-time.
function computeSelfInsets(node) {
  if (node.durationNs === 0n) return []
  return (node.childCovered ?? []).map(([s, e]) => ({
    leftPct: pct(s - node.startNs, node.durationNs),
    widthPct: pct(e - s, node.durationNs),
  }))
}

// Event markers positioned within the span bar (relative to the span start).
function computeEventMarkers(node) {
  const events = Array.isArray(node.span.events) ? node.span.events : []
  const span = node.span
  return events
    .map((e) => {
      const t = e.time_unix_nano != null ? BigInt(e.time_unix_nano) : null
      if (t == null || node.durationNs === 0n) return null
      const within = t - span.start_time_nanos
      if (within < 0n || within > node.durationNs) return null
      return { leftPct: pct(within, node.durationNs), name: e.name ?? 'event' }
    })
    .filter(Boolean)
}

// Keyboard nav (rows container has focus): j/k or arrow up/down move selection through the
// currently-visible rows; left/right collapse/expand the selected subtree.
function moveSelection(delta) {
  const list = openRows.value
  if (!list.length) return
  const curIdx = list.findIndex((n) => n.id === props.selectedSpanId)
  const nextIdx =
    curIdx === -1 ? (delta > 0 ? 0 : list.length - 1) : Math.min(list.length - 1, Math.max(0, curIdx + delta))
  const next = list[nextIdx]
  if (!next) return
  emit('select-span', next.id)
  rowVirtualizer.value.scrollToIndex(nextIdx, { align: 'auto' })
}
function collapseSelected() {
  const node = trace.value.nodes.get(props.selectedSpanId)
  if (node?.children.length) setCollapsed(node.id, true)
}
function expandSelected() {
  const node = trace.value.nodes.get(props.selectedSpanId)
  if (node?.children.length) setCollapsed(node.id, false)
}
function onRowsKeydown(event) {
  switch (event.key) {
    case 'j':
    case 'ArrowDown':
      event.preventDefault()
      moveSelection(1)
      break
    case 'k':
    case 'ArrowUp':
      event.preventDefault()
      moveSelection(-1)
      break
    case 'ArrowLeft':
      event.preventDefault()
      collapseSelected()
      break
    case 'ArrowRight':
      event.preventDefault()
      expandSelected()
      break
    case 'n':
      event.preventDefault()
      goToMatch(1)
      break
    case 'N':
      event.preventDefault()
      goToMatch(-1)
      break
  }
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <template v-if="trace.spanCount">
      <!-- Waterfall toolbar: filter, collapse-healthy switch, expand/collapse-all, N of M count. -->
      <div class="mx-3 mt-3 flex flex-wrap items-center gap-x-4 gap-y-1.5">
        <div class="flex items-center gap-1">
          <Input
            v-model="filterText"
            type="text"
            placeholder="Filter spans…"
            class="h-7 w-44 font-mono text-xs"
          />
          <!-- Match navigation: current/total + prev/next chevrons (n / N). -->
          <div v-if="filterText.trim()" class="flex items-center gap-0.5">
            <span
              data-testid="match-nav-count"
              class="w-12 text-center font-mono text-[11px] tabular-nums text-muted-foreground"
            >
              {{ matchTotal ? currentMatchIndex + 1 : 0 }} / {{ matchTotal }}
            </span>
            <Button
              variant="ghost"
              size="icon"
              class="size-7"
              :disabled="matchTotal === 0"
              aria-label="Previous match"
              @click="goToMatch(-1)"
            >
              <ChevronUp class="size-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="size-7"
              :disabled="matchTotal === 0"
              aria-label="Next match"
              @click="goToMatch(1)"
            >
              <ChevronDown class="size-3.5" />
            </Button>
          </div>
        </div>

        <div class="flex items-center gap-1.5">
          <Label for="waterfall-collapse-healthy" class="cursor-pointer text-xs text-muted-foreground"
            >Collapse healthy</Label
          >
          <Switch
            id="waterfall-collapse-healthy"
            :model-value="collapseHealthy"
            aria-label="Collapse healthy spans"
            @update:model-value="emit('update:collapseHealthy', $event)"
          />
        </div>

        <div class="flex items-center gap-1">
          <Button variant="ghost" size="sm" class="h-7 px-2 font-mono text-[11px]" @click="expandAll"
            >Expand all</Button
          >
          <Button variant="ghost" size="sm" class="h-7 px-2 font-mono text-[11px]" @click="collapseAll"
            >Collapse all</Button
          >
        </div>

        <span class="font-mono text-xs text-muted-foreground">{{ matchCount }} of {{ openRows.length }} spans</span>

        <span class="ml-auto font-mono text-[10px] text-muted-foreground/70"
          >j/k move · ←/→ collapse · n/N match · esc close</span
        >
      </div>

      <!-- Time axis + ticks, split on the same grid as the rows so ticks sit above the bar track.
           When the minimap is shown it narrows the rows' bar-track, so the axis carries a matching
           spacer on its right to keep the tick track the same width as the gridlines/bars below. -->
      <div class="mx-3 mt-2 flex h-4 shrink-0 border-b border-border">
        <div class="grid min-w-0 flex-1" :style="{ gridTemplateColumns: GRID_TEMPLATE_COLUMNS }">
          <div></div>
          <div class="relative h-full">
            <span
              v-for="(t, i) in ticks"
              :key="'tick-' + i"
              class="absolute -translate-x-1/2 font-mono text-[10px] text-muted-foreground"
              :style="{ left: t.leftPct + '%' }"
              >{{ t.label }}</span
            >
          </div>
        </div>
        <div v-if="showMinimap" class="w-[72px] shrink-0" aria-hidden="true" />
      </div>

      <!-- Span rows (virtualized) + right-edge minimap for deep traces. -->
      <div class="flex min-h-0 flex-1">
      <div
        ref="scrollEl"
        data-testid="waterfall-rows"
        tabindex="0"
        class="min-h-0 flex-1 overflow-y-auto outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
        @keydown="onRowsKeydown"
        @scroll="onRowsScroll"
      >
        <div :style="{ height: totalSize + 'px', width: '100%', position: 'relative' }">
          <!-- Gridlines: faint vertical lines at each axis tick, behind the rows. -->
          <div
            class="pointer-events-none absolute inset-0 grid"
            :style="{ gridTemplateColumns: GRID_TEMPLATE_COLUMNS }"
          >
            <div></div>
            <div class="relative h-full">
              <span
                v-for="(t, i) in ticks"
                :key="'grid-' + i"
                class="absolute inset-y-0 w-px bg-border/40"
                :style="{ left: t.leftPct + '%' }"
              />
            </div>
          </div>

          <div
            v-for="row in visibleRows"
            :key="row.key"
            data-span-row
            :data-span-id="row.node.id"
            :class="
              cn(
                'grid cursor-pointer items-center gap-x-2 border-b border-border/50 pr-3 font-mono text-xs transition-opacity duration-150 motion-reduce:transition-none',
                row.node.id === selectedSpanId ? 'bg-muted' : 'hover:bg-muted/40',
                !isRowMatch(row.node.id) && 'opacity-40',
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
            @click="emit('select-span', row.node.id)"
          >
            <!-- Label column: indent by depth, collapse chevron, service + operation. -->
            <div
              class="flex items-center gap-1.5 overflow-hidden"
              :style="{ paddingLeft: row.node.depth * 14 + 4 + 'px' }"
            >
              <button
                v-if="row.node.children.length"
                type="button"
                data-testid="span-collapse-toggle"
                class="flex size-3.5 shrink-0 items-center justify-center text-muted-foreground hover:text-foreground"
                :aria-label="collapsed.has(row.node.id) ? 'Expand subtree' : 'Collapse subtree'"
                @click="onChevronClick($event, row.node.id)"
              >
                <ChevronRight v-if="collapsed.has(row.node.id)" class="size-3" />
                <ChevronDown v-else class="size-3" />
              </button>
              <span v-else class="size-3.5 shrink-0" />

              <StatusDot v-if="row.node.isError" tone="error" class="size-2" />
              <span
                v-else
                :class="cn('size-2 shrink-0 rounded-full', serviceColorClass(row.node.span.service))"
              />
              <span class="truncate text-foreground/80">{{ row.node.span.service }}</span>
              <span class="truncate text-foreground">{{ row.node.span.name }}</span>
              <span
                v-if="collapsed.has(row.node.id)"
                class="shrink-0 rounded bg-muted px-1 text-[9px] text-muted-foreground"
                >+{{ row.node.descendantCount }}</span
              >
              <span
                v-if="row.node.hasClockSkew"
                class="shrink-0 rounded bg-amber-500/20 px-1 text-[9px] text-amber-600 dark:text-amber-400"
                title="clock skew: this span starts before its parent"
                >skew</span
              >
            </div>

            <!-- Bar track. -->
            <div class="relative h-full">
              <div
                data-span-bar
                :class="
                  cn(
                    'absolute top-1/2 flex h-3 -translate-y-1/2 items-center rounded-sm',
                    barClass(row.node),
                    row.node.onCriticalPath && 'ring-1 ring-foreground/60',
                  )
                "
                :style="{ left: row.node.barLeftPct + '%', width: row.node.barWidthPct + '%' }"
                :title="`${row.node.span.name} · ${formatDuration(row.node.durationNs)}`"
              >
                <!-- Self-time insets: regions covered by children render lighter, leaving the
                     remaining solid slivers as the span's own self-time. -->
                <span
                  v-for="(inset, i) in row.node.selfInsets"
                  :key="'self-' + i"
                  class="absolute inset-y-0 bg-background/50"
                  :style="{ left: inset.leftPct + '%', width: inset.widthPct + '%' }"
                />
                <span
                  v-for="(ev, i) in row.node.eventMarkers"
                  :key="'ev-' + i"
                  class="absolute -top-0.5 -translate-x-1/2 text-[8px] leading-none text-foreground"
                  :style="{ left: ev.leftPct + '%' }"
                  :title="ev.name"
                  >◆</span
                >
              </div>
              <span
                class="absolute top-1/2 -translate-y-1/2 pl-1 font-mono text-[10px] text-muted-foreground"
                :style="{ left: row.node.barLeftPct + row.node.barWidthPct + '%' }"
                >{{ formatDuration(row.node.durationNs) }}</span
              >
            </div>
          </div>
        </div>
      </div>

        <TraceMinimap
          v-if="showMinimap"
          :rows="openRows"
          :trace-duration-ns="trace.durationNs"
          :scroll-top="scrollTop"
          :viewport-height="viewportHeight"
          :total-height="totalSize"
          :matches="matchIds"
          @scroll-to="onMinimapScrollTo"
        />
      </div>
    </template>

    <EmptyState
      v-else
      title="No spans"
      description="This trace has no spans to display."
    />
  </div>
</template>

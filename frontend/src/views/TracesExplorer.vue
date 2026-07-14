<script setup>
import { ref, computed, watch, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { useQuery, keepPreviousData } from '@tanstack/vue-query'
import { refDebounced, useIntersectionObserver } from '@vueuse/core'
import AppShell from '@/components/common/AppShell.vue'
import SearchBar from '@/components/common/SearchBar.vue'
import LiveControl from '@/components/common/LiveControl.vue'
import TracesFilters from '@/components/traces/TracesFilters.vue'
import SpanVolumeHistogram from '@/components/traces/SpanVolumeHistogram.vue'
import LatencyHistogram from '@/components/traces/LatencyHistogram.vue'
import TraceTable from '@/components/traces/TraceTable.vue'
import SpanTable from '@/components/traces/SpanTable.vue'
import TracePeekDrawer from '@/components/traces/TracePeekDrawer.vue'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { SelectMenu } from '@/components/ui/select-menu'
import { Switch } from '@/components/ui/switch'
import { Label } from '@/components/ui/label'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { formatNumber, relative } from '@/lib/core/format'
import { useUrlState } from '@/lib/core/useUrlState'
import { useSearchTraces, useSearchSpans, useTracesFields } from '@/lib/traces/tracesQueries'
import { useLiveTail, mergeLiveRows } from '@/lib/core/useLiveTail'
import { nextIndex } from '@/lib/core/listNav'
import {
  toggleFieldValue,
  toggleFacetValue,
  removeFieldAll,
  setDurationRange,
  fieldValues,
  onlyFieldValue,
} from '@/lib/core/queryLang'
import { SPAN_FIELDS, SPAN_EXAMPLE_QUERIES } from '@/lib/traces/spanFields'
import {
  timeRange,
  customRange,
  nowTick,
  startMs,
  endMs,
  startNs,
  endNs,
  setCustomRange,
} from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'

const router = useRouter()

// --- state ---
// timeRange/customRange/nowTick/startMs/endMs/startNs/endNs are now app-wide (lib/context.js),
// surfaced by the ContextBar mounted in AppShell — same window presets as before ("last 30m" etc.
// still mean the same thing across every explorer), just no longer duplicated per view.
const text = ref('')
const sort = ref('recent') // 'recent' | 'slowest' | 'errors'
const resultMode = ref('traces') // 'traces' | 'spans' — which result grain the list shows
const chartMode = ref('volume') // 'volume' | 'latency'

const SORTS = [
  { key: 'recent', label: 'Recent' },
  { key: 'slowest', label: 'Slowest' },
  { key: 'errors', label: 'Errors' },
]
const CHART_MODES = [
  { key: 'volume', label: 'Volume' },
  { key: 'latency', label: 'Latency' },
]
const RESULT_MODES = [
  { key: 'traces', label: 'Traces' },
  { key: 'spans', label: 'Spans' },
]

// Rounded aggregate window for the facet rails + histograms. They key their queries off the
// window, so feeding them the raw now-anchored `startMs`/`endMs` would churn those cache keys on
// every idle 12s `nowTick` tick (a re-fetch for a sub-second window shift nobody can see). Snapping
// to a 60s bucket (floor the start, ceil the end) holds their keys steady between real minute
// boundaries. The LIST search keeps the PRECISE window via `buildRequest` — this only feeds aggregates.
const AGG_BUCKET_MS = 60_000
const aggStartMs = computed(() => Math.floor(startMs.value / AGG_BUCKET_MS) * AGG_BUCKET_MS)
const aggEndMs = computed(() => Math.ceil(endMs.value / AGG_BUCKET_MS) * AGG_BUCKET_MS)

// --- URL persistence (seed BEFORE the debounced mirror so the first search carries the URL query) ---
// `text` rides the shared logs/traces URL scheme (`q`); time now lives entirely in context.js's own
// `range`/`from`/`to` URL sync (seeded + started once in main.js).
useUrlState({ text })

// `sort` and `resultMode` aren't among useUrlState's fixed keys, so they're layered on top with
// their own tiny sync: seed each once from the raw query string (before anything rewrites it)...
if (typeof window !== 'undefined') {
  const params = new URLSearchParams(window.location.search)
  const initialSort = params.get('sort')
  if (initialSort) sort.value = initialSort
  const initialMode = params.get('mode')
  if (initialMode) resultMode.value = initialMode
}

// --- services (for the search-bar autocomplete) ---
const servicesQuery = useQuery({
  queryKey: ['services'],
  queryFn: ({ signal }) => api.services({ signal }),
  staleTime: 5 * 60 * 1000,
})
const servicesList = computed(() => servicesQuery.data.value ?? [])

// --- field catalog (fetched ONCE here, shared by both tables' column pickers as the raw
// [{ name, kind }] shape; each table filters/maps it internally). ---
const fieldsQuery = useTracesFields(startNs, endNs)
const fieldsCatalog = computed(() => fieldsQuery.data.value ?? [])

// Attribute columns the trace search should project (learned from TraceTable's `columns-changed`)
// so rows carry `root_attributes` for them. Spans never request columns (their attributes come
// inline), so this only feeds the trace request.
const traceAttrCols = ref([])

// --- search (TanStack infinite query) ---
// The search bar debounces at 180ms — only the settled text keys the query (and thus refetches).
const debouncedText = refDebounced(text, 180)

// Cache key = the RELATIVE search descriptor. It deliberately excludes the now-anchored absolute
// window (that would churn the key every millisecond); the absolute start/end are resolved at
// FETCH time in `buildRequest`. A pinned custom range IS part of the key (it's fixed, not "now").
// `mode` re-keys when flipping Traces↔Spans; the sorted attribute-column list re-keys so adding a
// column refetches with the extra projection.
const searchDescriptor = computed(() => ({
  range: timeRange.value,
  custom: customRange.value ? `${customRange.value.startMs}-${customRange.value.endMs}` : null,
  query: debouncedText.value.trim(),
  sort: sort.value,
  mode: resultMode.value,
  columns: [...traceAttrCols.value].sort().join(','),
  limit: 100,
}))

// Runs inside the queryFn — for every page and every refetch (incl. each live-tail poll). The
// now-anchored window resolves here against the wall clock at the moment of the fetch. `nowTick`
// is refreshed only on the first page (cursor == null) so all pages of one cycle share a window.
function buildRequest(cursor) {
  if (cursor == null && !customRange.value) nowTick.value = Date.now()
  const ns = (ms) => (BigInt(Math.round(ms)) * 1_000_000n).toString()
  return {
    start: ns(startMs.value),
    end: ns(endMs.value),
    query: debouncedText.value.trim(),
    sort: sort.value,
    limit: 100,
    cursor,
    // Only traces carry projectable attribute columns; spans decode their attributes inline.
    columns: resultMode.value === 'traces' ? traceAttrCols.value : [],
  }
}

// Peek drawer: a row click opens an in-list preview (NOT a route navigation); the full
// `/traces/:id` waterfall opens only from the drawer's "Open full view". Declared here (ahead of
// the query options) because `refetchInterval` below reads it to pause live tail while it's open.
const drawer = ref(null) // { traceId, spanId, timeHintNs } | null

// Live tail: streams flat SPANS (grain: 'spans') regardless of which result mode is showing —
// selecting Live auto-switches `resultMode` to 'spans' (see `onLiveMode` below), since traces
// (aggregated multi-span groups) have no meaningful streamed row. `pollMs` mirrors the old `live`
// boolean's job of driving `refetchInterval`, but as a mode-aware value: a number while polling
// (5s/30s), `false` while manual OR while 'live' hands rows to the SSE stream instead.
// `search` (below) is referenced inside `onPoll` before its own declaration runs — safe because
// `onPoll` only fires from a later user interaction, by which time `search` is assigned.
const pollMs = ref(false)
const liveTail = useLiveTail({
  grain: 'spans',
  query: computed(() => debouncedText.value.trim()),
  onPoll: (v) => {
    if (v === 'once') {
      search.value.refetch()
      return
    }
    pollMs.value = v
  },
})
// Opening the drawer pauses the stream's prepend (rows queue into `newCount` instead) so the list
// underneath a peek doesn't shift; polling is paused the same way it always was, via
// `refetchInterval` below.
watch(drawer, (d) => liveTail.setPaused(!!d))

// Shared query options for both grains: live-tail poll (paused for a custom range OR an open
// drawer) + keep the last good page on screen while a new/failed search resolves — so a bad query
// leaves the previous rows visible under the search-bar error underline.
const refetchInterval = computed(() =>
  typeof pollMs.value === 'number' && !customRange.value && !drawer.value ? pollMs.value : false,
)
const searchOpts = { refetchInterval, placeholderData: keepPreviousData }

// Both grains are instantiated; only the active one is `enabled`, so the other never fetches.
const traceSearch = useSearchTraces(searchDescriptor, buildRequest, {
  enabled: computed(() => resultMode.value === 'traces'),
  ...searchOpts,
})
const spanSearch = useSearchSpans(searchDescriptor, buildRequest, {
  enabled: computed(() => resultMode.value === 'spans'),
  ...searchOpts,
})
const search = computed(() => (resultMode.value === 'spans' ? spanSearch : traceSearch))

// Normalize the two page shapes (traces expose `traces`, spans expose `rows`) to one list.
const results = computed(() => {
  const pages = search.value.data.value?.pages ?? []
  return resultMode.value === 'spans'
    ? pages.flatMap((p) => p.rows ?? [])
    : pages.flatMap((p) => p.traces ?? [])
})
const matchedCount = computed(() => search.value.data.value?.pages?.[0]?.matched_count ?? 0)
const elapsedMs = computed(() => search.value.data.value?.pages?.[0]?.elapsed_ms ?? 0)
const loading = computed(() => search.value.isFetching.value)
const resultNoun = computed(() => (resultMode.value === 'spans' ? 'spans' : 'traces'))

// The table shows the streamed spans while Live is active AND the Spans grain is showing — merged on
// top of the current span search page as a frozen baseline, so entering Live keeps the already-
// loaded spans visible instead of blanking to the empty stream buffer. Live only ever streams spans,
// so if the user manually flips back to Traces while still live (the LiveControl itself can't reach
// that combination, but the Traces/Spans segmented toggle can), `results` (the real traces page)
// must win rather than feeding span rows into <TraceTable>.
const displayResults = computed(() =>
  liveTail.mode.value === 'live' && resultMode.value === 'spans'
    ? mergeLiveRows(liveTail.rows.value, results.value)
    : results.value,
)

// Aggregates (span-volume/latency histogram + facet rails) key off `startMs`/`endMs`, which derive
// from `nowTick` unless a custom range is pinned. `nowTick` normally advances via the list query's
// own fetch cycle (`buildRequest`) — but 'live' hands the list off to the SSE stream, which never
// calls `buildRequest`, so the charts/facets would otherwise freeze at the moment Live was
// selected. A slow 12s tick keeps them moving (and therefore refetching, since their query keys
// include the window) without a second `refetchInterval` plumbed through every aggregate
// component. 5s/30s polling already advances `nowTick` on its own (faster than 12s), so this only
// needs to run for 'live'.
let aggregatesTimer = null
watch(
  () => liveTail.mode.value,
  (mode) => {
    if (aggregatesTimer) {
      clearInterval(aggregatesTimer)
      aggregatesTimer = null
    }
    if (mode === 'live') {
      aggregatesTimer = setInterval(() => {
        if (!customRange.value) nowTick.value = Date.now()
      }, 12000)
    }
  },
  { immediate: true },
)
onUnmounted(() => {
  if (aggregatesTimer) clearInterval(aggregatesTimer)
})
// De-emphasized staleness caption next to the chart header while live/polling is active — reuses
// `nowTick` (the window anchor) rather than a dedicated per-second clock.
const asOfLabel = computed(() => relative(BigInt(nowTick.value) * 1_000_000n))

// 400 surfacing: map the active query's error to a FRESH `{ message, offset }` object on every
// error/success resolve (SearchBar's error-suppression watch keys off the prop REFERENCE changing,
// not a deep-equal). Non-400 errors never reach here — api.searchTraces/searchSpans fall back to
// the mock for those.
const queryError = ref(null)
watch(
  () => [search.value.error.value, search.value.errorUpdatedAt.value, search.value.dataUpdatedAt.value],
  () => {
    const e = search.value.error.value
    queryError.value =
      e && e.status === 400
        ? { message: e.body?.error ?? 'invalid query', offset: e.body?.offset ?? null }
        : null
  },
)

// --- infinite scroll: a sentinel below the list (inside the active table's scroll area) pulls the
// next page as it scrolls into view. Only one table (hence one #footer sentinel) is mounted at a
// time; the ref re-binds to whichever is active. ---
const sentinel = ref(null)
useIntersectionObserver(sentinel, ([entry]) => {
  const s = search.value
  if (entry?.isIntersecting && s.hasNextPage.value && !s.isFetchingNextPage.value) {
    s.fetchNextPage()
  }
})

// --- handlers ---
function onSort(value) {
  if (!value || value === sort.value) return
  sort.value = value
}
function onResultMode(value) {
  if (!value || value === resultMode.value) return
  resultMode.value = value
}
// The LiveControl's `@update:mode` handler — routes every mode change through `liveTail.setMode`,
// but selecting 'live' while looking at Traces auto-switches to Spans FIRST (traces have no
// meaningful streamed row; only flat spans stream). Must never fire from a watcher — re-entering
// 'live' via a watch would re-clear the stream buffer on every unrelated reactive update.
function onLiveMode(next) {
  if (next === 'live' && resultMode.value === 'traces') resultMode.value = 'spans'
  liveTail.setMode(next)
}
function onChartMode(value) {
  if (!value || value === chartMode.value) return
  chartMode.value = value
}
// Still wired to SpanVolumeHistogram's drag-zoom (`@zoom`) — the global ContextBar owns time now,
// but the chart's zoom gesture still pins a custom window.
function onCustomRange(r) {
  setCustomRange(r)
  // Pinning a fixed window makes no sense alongside a live/polling tail — drop back to manual.
  if (liveTail.mode.value !== 'manual') liveTail.setMode('manual')
}
// Dragging a duration band on the latency chart rewrites the search text to a removable
// `duration>=A duration<=B` pill — the existing text→debouncedText→search pipeline refetches
// the list, facets, and the chart itself against the new range (same as typing it by hand).
function onLatencyBrush({ minNs, maxNs }) {
  text.value = setDurationRange(text.value, minNs, maxNs)
}
// A facet checkbox toggles a value's single-state membership (SigNoz-style). `toggleFacetValue`
// picks the mode: from the default all-checked state, unchecking a value EXCLUDES it (`-field:v`);
// inside an active include-set it adds/removes an include. The query text feeds the search key, so
// the search refetches automatically — same generic mechanism across both explorers.
function onToggleValue({ field, value }) {
  text.value = toggleFacetValue(text.value, field, value)
}
// "Clear All" resets a field to its default (all-checked) state — drops BOTH its includes and its
// exclusions via `removeFieldAll` (the include-only `removeField` would leave stray `-field:v`).
function onClearField(field) {
  text.value = removeFieldAll(text.value, field)
}
// SigNoz-style hover "Only" action: collapse the field to exactly this value (clearing its other
// includes AND exclusions). Rewrites the query text (the single source of truth), so the search +
// facets refetch automatically — same mechanism as `onToggleValue`. There is no separate "Exclude"
// action in the single-state model: unchecking a value IS the exclusion (see `onToggleValue`).
function onOnlyValue({ field, value }) {
  text.value = onlyFieldValue(text.value, field, value)
}
// Errors-only shortcut: checked ⇔ the query already selects `status:error`; toggling adds/removes
// exactly that term via the same grammar helper the facet rail uses.
const errorsOnly = computed(() => fieldValues(text.value, 'status').includes('error'))
function onErrorsOnly() {
  text.value = toggleFieldValue(text.value, 'status', 'error')
}
// Row clicks open the drawer (trace start_ts / span start time become the manifest time hint).
function onOpenTrace({ traceId, timeHintNs }) {
  drawer.value = { traceId, spanId: null, timeHintNs }
}
function onOpenSpan({ traceId, spanId, timeHintNs }) {
  drawer.value = { traceId, spanId, timeHintNs }
}
// "Open full view →" hands off to the waterfall route, carrying the time hint (+ span to
// pre-select) so the detail view lands narrowed and, for a span, pre-selected.
function onOpenFull({ traceId, spanId }) {
  router.push(
    '/traces/' + traceId + '?t=' + drawer.value.timeHintNs + (spanId ? '&span=' + spanId : ''),
  )
}

// --- prev/next-between-rows while the peek drawer is open ---
// The drawer emits intent (`prev`/`next`); this view owns the move over `displayResults`. Opening
// the drawer already pauses live prepend (`liveTail.setPaused(true)`), so the list under the peek
// is stable and index stepping is safe. Identity matches by `trace_id` in traces mode, `span_id`
// in spans mode — the same fields the row-open handlers key off.
const drawerIndex = computed(() => {
  if (!drawer.value) return -1
  const list = displayResults.value
  return resultMode.value === 'spans'
    ? list.findIndex((r) => r.span_id === drawer.value.spanId)
    : list.findIndex((r) => r.trace_id === drawer.value.traceId)
})
const drawerTotal = computed(() => displayResults.value.length)

// The drawer's current row identity, handed to the active table so the list highlights the peeked
// row and follows the drawer's prev/next stepping (trace grain matches `trace_id`, span grain
// `span_id`). Null when the drawer is closed.
const selectedId = computed(() =>
  !drawer.value ? null : resultMode.value === 'spans' ? drawer.value.spanId : drawer.value.traceId,
)

// Step the drawer to an adjacent row (clamped to the loaded list via `nextIndex` — moving past an
// end is a no-op, matching the primitive's disabled ‹ › buttons; no page fetch, per non-goal).
function stepDrawer(delta) {
  if (!drawer.value) return
  const list = displayResults.value
  const i = nextIndex(list.length, drawerIndex.value, delta)
  if (i < 0 || i === drawerIndex.value) return
  const row = list[i]
  if (!row) return
  drawer.value =
    resultMode.value === 'spans'
      ? { traceId: row.trace_id, spanId: row.span_id, timeHintNs: row.start_time_nanos }
      : { traceId: row.trace_id, spanId: null, timeHintNs: String(row.start_ts) }
}
function onDrawerPrev() {
  stepDrawer(-1)
}
function onDrawerNext() {
  stepDrawer(1)
}

// trace → logs pivot: filter the logs explorer to this trace via a `trace_id:<id>` grammar term
// (LogsView seeds its search `text` from `route.query.q`). Time hint is optional for logs; the
// `q` term is the required part. Mirrors the existing span/trace → logs convention.
function onViewLogs({ traceId }) {
  router.push(correlate({ path: '/logs', query: { q: 'trace_id:' + traceId } }))
}

// ...then re-stamp `sort` + `mode` onto the URL after every relevant change. useUrlState's own
// watcher (the default 'pre' flush timing) rewrites the WHOLE query string via buildQuery whenever
// timeRange/text change, and buildQuery has no notion of `sort`/`mode` — a `flush: 'post'` watcher
// here always runs after that rewrite, so neither gets silently dropped from the URL.
if (typeof window !== 'undefined') {
  watch(
    [timeRange, text, sort, resultMode],
    () => {
      const params = new URLSearchParams(window.location.search)
      params.set('sort', sort.value)
      params.set('mode', resultMode.value)
      window.history.replaceState(null, '', '?' + params.toString())
    },
    { flush: 'post' },
  )
}
</script>

<template>
  <AppShell active="traces" :mock="api.mock" crumb="Traces">
    <template #toolbar>
      <SearchBar
        :model-value="text"
        :services="servicesList"
        :error="queryError"
        :catalog="SPAN_FIELDS"
        :example-queries="SPAN_EXAMPLE_QUERIES"
        @update:model-value="text = $event"
      />
    </template>

    <div class="flex flex-1 min-h-0">
      <aside class="flex w-[210px] flex-none flex-col overflow-y-auto border-r border-border">
        <TracesFilters
          :query="text"
          :start-ms="aggStartMs"
          :end-ms="aggEndMs"
          @toggle-value="onToggleValue"
          @clear-field="onClearField"
          @only-value="onOnlyValue"
        />
      </aside>

      <main class="flex flex-col flex-1 min-w-0 min-h-0">
        <div class="px-5 pb-3 pt-5">
          <div class="mb-3 flex items-center gap-2.5">
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              :model-value="chartMode"
              @update:model-value="onChartMode"
            >
              <ToggleGroupItem
                v-for="m in CHART_MODES"
                :key="m.key"
                :value="m.key"
                :data-testid="'chart-' + m.key"
                class="px-3 font-mono text-xs"
              >
                {{ m.label }}
              </ToggleGroupItem>
            </ToggleGroup>
            <span class="ml-auto font-mono text-[11px] text-muted-foreground">
              {{ customRange ? 'custom range' : `last ${timeRange}` }}
              <span v-if="liveTail.mode.value !== 'manual'" class="text-muted-foreground/60">
                · as of {{ asOfLabel }}
              </span>
            </span>
          </div>
          <SpanVolumeHistogram
            v-if="chartMode === 'volume'"
            :query="text"
            :start-ms="aggStartMs"
            :end-ms="aggEndMs"
            @zoom="onCustomRange"
          />
          <LatencyHistogram
            v-else
            :query="text"
            :start-ms="aggStartMs"
            :end-ms="aggEndMs"
            @brush="onLatencyBrush"
          />
        </div>

        <div class="flex items-center gap-2.5 px-5 pb-2 text-xs text-muted-foreground">
          <span class="font-mono tabular-nums text-foreground/80">
            {{ formatNumber(matchedCount) }} {{ resultNoun }}
          </span>
          <span class="text-border">·</span>
          <span class="font-mono tabular-nums">{{ elapsedMs }} ms</span>
          <span class="text-border">·</span>
          <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
          <Spinner v-if="loading" size="sm">Searching…</Spinner>

          <div class="ml-auto flex items-center gap-3">
            <Segmented
              type="single"
              :model-value="resultMode"
              @update:model-value="onResultMode"
            >
              <SegmentedItem
                v-for="m in RESULT_MODES"
                :key="m.key"
                :value="m.key"
                :data-testid="'mode-' + m.key"
                class="font-mono"
              >
                {{ m.label }}
              </SegmentedItem>
            </Segmented>
            <div class="flex items-center gap-1.5">
              <Label for="errors-only-toggle" class="cursor-pointer text-xs text-muted-foreground">
                Errors only
              </Label>
              <Switch
                id="errors-only-toggle"
                data-testid="errors-only-toggle"
                :model-value="errorsOnly"
                aria-label="Errors only"
                @update:model-value="onErrorsOnly"
              />
            </div>
            <SelectMenu
              :model-value="sort"
              :options="SORTS.map((s) => ({ value: s.key, label: s.label }))"
              prefix="Sort:"
              aria-label="Sort results"
              @update:model-value="onSort"
            />
            <LiveControl
              :mode="liveTail.mode.value"
              :status="liveTail.status.value"
              :rate="liveTail.rate.value"
              @update:mode="onLiveMode"
              @refresh="liveTail.refresh"
            />
          </div>
        </div>

        <TraceTable
          v-if="resultMode === 'traces'"
          :traces="displayResults"
          :loading="loading"
          :attr-catalog="fieldsCatalog"
          :selected-id="selectedId"
          @open-trace="onOpenTrace"
          @toggle-value="onToggleValue"
          @columns-changed="traceAttrCols = $event"
        >
          <template #footer>
            <div ref="sentinel" class="h-px w-full" aria-hidden="true" />
          </template>
        </TraceTable>
        <SpanTable
          v-else
          :spans="displayResults"
          :loading="loading"
          :attr-catalog="fieldsCatalog"
          :selected-id="selectedId"
          @open-span="onOpenSpan"
          @toggle-value="onToggleValue"
        >
          <template #footer>
            <div ref="sentinel" class="h-px w-full" aria-hidden="true" />
          </template>
        </SpanTable>
      </main>
    </div>

    <TracePeekDrawer
      :trace-id="drawer?.traceId"
      :span-id="drawer?.spanId"
      :time-hint-ns="drawer?.timeHintNs"
      :open="!!drawer"
      :index="drawerIndex"
      :total="drawerTotal"
      @close="drawer = null"
      @open-full="onOpenFull"
      @prev="onDrawerPrev"
      @next="onDrawerNext"
      @view-logs="onViewLogs"
    />
  </AppShell>
</template>

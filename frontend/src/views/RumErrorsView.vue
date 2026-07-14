<script setup>
// RUM Errors (`/rum/:appId/errors`): the app-wide JS error issue list (grouped by fingerprint,
// ordered by count desc). Same shell + sub-nav as the vitals hero (Errors active). Task 15 adds
// a SearchBar + fixed facet panel + URL-persisted `q`, mirroring LogsView's `text`/`useUrlState`/
// `refDebounced` wiring and the single-state facet model (RumErrorFilters derives checked state
// from `text`, this view only translates facet clicks into query edits via `queryLang`).
import { computed, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { refDebounced } from '@vueuse/core'
import AppShell from '@/components/common/AppShell.vue'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import SearchBar from '@/components/common/SearchBar.vue'
import RumErrorFilters from '@/components/rum/RumErrorFilters.vue'
import ErrorIssueList from '@/components/rum/ErrorIssueList.vue'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { formatNumber } from '@/lib/core/format'
import { timeRange, customRange, startNs, endNs } from '@/lib/core/context'
import { useRumErrors } from '@/lib/rum/rumQueries'
import { useUrlState } from '@/lib/core/useUrlState'
import { toggleFacetValue, onlyFieldValue, removeFieldAll } from '@/lib/core/queryLang'

const route = useRoute()

const app = computed(() => {
  const a = route.params.appId
  return ((Array.isArray(a) ? a[0] : a) ?? '').trim()
})
const appBase = computed(() => '/rum/' + encodeURIComponent(app.value))
// Keep the app-level crumb (sub-page of the same RUM app; scope set by the vitals view).
const crumb = computed(() => 'Frontend › ' + app.value)

// The search-bar text is the single source of truth for the fixed facet panel's checked state
// (RumErrorFilters derives it via `facetChecked`) — same single-state model as LogsView/
// LogsFilters. Persisted to the URL's `q` param via `useUrlState`, seeded BEFORE the debounced
// query is built so the first fetch already carries a deep-linked/seeded query.
const text = ref('')
useUrlState({ text })
// Correlation entry: a pivot into this view (e.g. `/rum/:app/errors?q=...`) lands with `q`
// already in the route — seed `text` from it synchronously, mirroring LogsView.
if (typeof route.query.q === 'string' && route.query.q) text.value = route.query.q
const debouncedText = refDebounced(text, 180)

// Fixed catalog for the SearchBar's field-name autocomplete — the same six dimensions
// RumErrorFilters facets on (`ERROR_FACET_FIELDS` in `crates/photon-api/src/rum.rs`). No
// `values` lists (unlike LogsView's services list) — free-typed values only.
const RUM_ERROR_FACET_CATALOG = [
  { name: 'exception.type', kind: 'attribute' },
  { name: 'error.kind', kind: 'attribute' },
  { name: 'browser.route', kind: 'attribute' },
  { name: 'browser.name', kind: 'attribute' },
  { name: 'device.type', kind: 'attribute' },
  { name: 'network.connection', kind: 'attribute' },
]

const errorsQuery = useRumErrors(app, startNs, endNs, debouncedText)
const errors = computed(() => errorsQuery.data.value?.errors ?? [])
const loading = computed(() => errorsQuery.isFetching.value)

// 400 contract (mirrors LogsView): publish a fresh { message, offset } object whenever a
// malformed query 400s, and clear it on each successful fetch. SearchBar's error-suppression
// watch keys off the prop REFERENCE changing, so a fresh object per resolve is required even for
// a repeated identical error.
const queryError = ref(null)
watch(
  [() => errorsQuery.errorUpdatedAt.value, () => errorsQuery.dataUpdatedAt.value],
  () => {
    const e = errorsQuery.error.value
    if (errorsQuery.isError.value && e?.status === 400) {
      queryError.value = { message: e.body?.error ?? 'invalid query', offset: e.body?.offset ?? null }
    } else if (errorsQuery.isSuccess.value) {
      queryError.value = null
    }
  },
)

// The three unified single-state facet emits (see queryLang.ts / LogsView's identical trio):
// row click toggles one value's checked state, "Only" narrows the field to exactly one value,
// Clear resets the field to its default all-checked state. `text`'s own watch (inside
// useUrlState/useRumErrors' reactive query keys) re-runs the search, so the panel, the search
// bar's pills, and the results can never desync.
function onToggleValue({ field, value }) {
  text.value = toggleFacetValue(text.value, field, value)
}
function onOnlyValue({ field, value }) {
  text.value = onlyFieldValue(text.value, field, value)
}
function onClearField(field) {
  text.value = removeFieldAll(text.value, field)
}
</script>

<template>
  <AppShell :mock="api.mock" :crumb="crumb">
    <!-- Web Vitals · Pages · Errors sub-nav + the search bar share the ContextBar's ONE fixed-
         height (h-12) search region — unlike LogsView (SearchBar alone), so the two are laid out
         side-by-side in a single row (NavTabs shrink-to-content, SearchBar filling the rest)
         rather than stacked, which would overflow the bar's fixed height. Deviation from the task
         brief, which showed them simply consecutive (implying a vertical stack). -->
    <template #toolbar>
      <div class="flex w-full items-center gap-3">
        <NavTabs class="shrink-0 text-xs">
          <NavTabItem :to="{ path: appBase, query: route.query }" :active="false">Web Vitals</NavTabItem>
          <NavTabItem :to="{ path: `${appBase}/pages`, query: route.query }" :active="false">Pages</NavTabItem>
          <NavTabItem :to="{ path: `${appBase}/errors`, query: route.query }" :active="true">Errors</NavTabItem>
        </NavTabs>
        <div class="min-w-0 flex-1">
          <SearchBar
            :model-value="text"
            @update:model-value="text = $event"
            :catalog="RUM_ERROR_FACET_CATALOG"
            :error="queryError"
            placeholder="Search errors…"
          />
        </div>
      </div>
    </template>

    <div class="flex flex-1 min-h-0 gap-4 px-5 pb-5">
      <RumErrorFilters
        :app="app"
        :query="text"
        :start-ns="startNs"
        :end-ns="endNs"
        @toggle="onToggleValue"
        @only="onOnlyValue"
        @clear="onClearField"
      />

      <main class="flex min-h-0 flex-1 flex-col overflow-y-auto">
        <div class="flex items-center gap-2.5 pb-2 pt-5 text-xs text-muted-foreground">
          <span class="font-mono tabular-nums text-foreground/80">{{ formatNumber(errors.length) }} issues</span>
          <span class="text-border">·</span>
          <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
          <Spinner v-if="loading" size="sm">loading…</Spinner>
        </div>

        <ErrorIssueList :issues="errors" :service="app" />
      </main>
    </div>
  </AppShell>
</template>

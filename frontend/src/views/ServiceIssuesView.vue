<script setup lang="ts">
// `/services/:service/errors` — this service's Backend Issues.
//
// An Issue is every errored span sharing one `error.fingerprint`: one bug, however many times it
// fired. The list answers "what is broken here", which the Operations tab cannot — an operation
// with a 4% error rate tells you where to look, not what you are looking at.
//
// Deliberately the same page as `/rum/:app/errors`, one runtime over: search bar, fixed six-field
// facet panel, list ordered by occurrences. The two worlds should read alike.
import { computed, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { refDebounced } from '@vueuse/core'
import ServiceScaffold from '@/components/services/ServiceScaffold.vue'
import IssueFilters from '@/components/services/IssueFilters.vue'
import IssueList from '@/components/services/IssueList.vue'
import IssueBackfillNote from '@/components/services/IssueBackfillNote.vue'
import SearchBar from '@/components/common/SearchBar.vue'
import { Spinner } from '@/components/ui/spinner'
import { formatNumber } from '@/lib/core/format'
import { startNs, endNs, timeRange, customRange } from '@/lib/core/context'
import { useBackendIssues } from '@/lib/services/servicesQueries'
import { useServiceParam } from '@/lib/services/useServiceParam'
import { useUrlState } from '@/lib/core/useUrlState'
import { toggleFacetValue, onlyFieldValue, removeFieldAll } from '@/lib/core/queryLang'

const route = useRoute()
const service = useServiceParam()

// The search-bar text is the single source of truth for the facet panel's checked state, same
// model as LogsView/RumErrorsView. Persisted to `?q=` and seeded from it, so a correlated pivot
// into this view lands already filtered.
const text = ref('')
useUrlState({ text })
if (typeof route.query.q === 'string' && route.query.q) text.value = route.query.q
const debouncedText = refDebounced(text, 180)

// Field-name autocomplete for the search bar — the six facet fields, mirroring
// `ISSUE_FACET_FIELDS`. No value lists: free-typed values only.
const ISSUE_FACET_CATALOG = [
  { name: 'error.type', kind: 'attribute' },
  { name: 'name', kind: 'attribute' },
  { name: 'kind_text', kind: 'attribute' },
  { name: 'deployment.environment', kind: 'attribute' },
  { name: 'host.name', kind: 'attribute' },
  { name: 'http.status_code', kind: 'attribute' },
]

const issuesQuery = useBackendIssues(service, debouncedText, startNs, endNs)
const issues = computed(() => issuesQuery.data.value?.errors ?? [])
const loading = computed(() => issuesQuery.isFetching.value)
const events = computed(() => issues.value.reduce((sum, i) => sum + i.count, 0))

// 400 contract (mirrors LogsView/RumErrorsView): publish a FRESH { message, offset } object per
// malformed query, clear it on each success. SearchBar's suppression watch keys off the prop
// reference changing, so a repeated identical error still needs a new object.
const queryError = ref<{ message: string; offset: number | null } | undefined>(undefined)
watch([() => issuesQuery.errorUpdatedAt.value, () => issuesQuery.dataUpdatedAt.value], () => {
  const e = issuesQuery.error.value as { status?: number; body?: { error?: string; offset?: number } } | null
  if (issuesQuery.isError.value && e?.status === 400) {
    queryError.value = { message: e.body?.error ?? 'invalid query', offset: e.body?.offset ?? null }
  } else if (issuesQuery.isSuccess.value) {
    queryError.value = undefined
  }
})

function onToggleValue({ field, value }: { field: string; value: string }): void {
  text.value = toggleFacetValue(text.value, field, value)
}
function onOnlyValue({ field, value }: { field: string; value: string }): void {
  text.value = onlyFieldValue(text.value, field, value)
}
function onClearField(field: string): void {
  text.value = removeFieldAll(text.value, field)
}
</script>

<template>
  <ServiceScaffold :service="service" crumb-suffix="Issues">
    <template #search>
      <SearchBar
        :model-value="text"
        :catalog="ISSUE_FACET_CATALOG"
        :error="queryError"
        placeholder="Search issues…"
        @update:model-value="text = $event"
      />
    </template>

    <div class="flex min-h-0 flex-1 gap-4 px-5 pb-5">
      <IssueFilters
        :service="service"
        :query="text"
        :start-ns="startNs"
        :end-ns="endNs"
        @toggle="onToggleValue"
        @only="onOnlyValue"
        @clear="onClearField"
      />

      <main class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto pt-5">
        <div class="flex flex-wrap items-center gap-2.5 text-xs text-muted-foreground">
          <span class="font-mono tabular-nums text-foreground/80">{{ formatNumber(issues.length) }} issues</span>
          <span class="text-border">·</span>
          <span class="font-mono tabular-nums">{{ formatNumber(events) }} events</span>
          <span class="text-border">·</span>
          <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
          <Spinner v-if="loading" size="sm">loading…</Spinner>
        </div>

        <IssueList :issues="issues" :loading="loading" />

        <IssueBackfillNote />
      </main>
    </div>
  </ServiceScaffold>
</template>

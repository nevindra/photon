<script setup lang="ts">
// `/services/errors` — Backend Issues across the whole fleet.
//
// The per-service list answers "what is broken in this service". This one answers the question
// you actually start the morning with: "what is broken anywhere". Same list, same ordering, one
// term removed — so the ranking here and inside a service can never disagree.
//
// There is no facet panel. Facets are computed per service by the API (they are scoped to one
// service's errored spans), and a fleet-wide facet would need a different endpoint to be honest
// about it. The search bar still works: every field a facet would offer is typable.
import { computed, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { refDebounced } from '@vueuse/core'
import AppShell from '@/components/common/AppShell.vue'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import SearchBar from '@/components/common/SearchBar.vue'
import IssueList from '@/components/services/IssueList.vue'
import IssueBackfillNote from '@/components/services/IssueBackfillNote.vue'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { formatNumber } from '@/lib/core/format'
import { startNs, endNs, timeRange, customRange } from '@/lib/core/context'
import { useBackendIssues } from '@/lib/services/servicesQueries'
import { useUrlState } from '@/lib/core/useUrlState'

const route = useRoute()

const text = ref('')
useUrlState({ text })
if (typeof route.query.q === 'string' && route.query.q) text.value = route.query.q
const debouncedText = refDebounced(text, 180)

const ISSUE_FACET_CATALOG = [
  { name: 'service.name', kind: 'attribute' },
  { name: 'error.type', kind: 'attribute' },
  { name: 'name', kind: 'attribute' },
  { name: 'kind_text', kind: 'attribute' },
  { name: 'deployment.environment', kind: 'attribute' },
  { name: 'host.name', kind: 'attribute' },
  { name: 'http.status_code', kind: 'attribute' },
]

// `null` service = the fleet-wide route.
const issuesQuery = useBackendIssues(null, debouncedText, startNs, endNs)
const issues = computed(() => issuesQuery.data.value?.errors ?? [])
const loading = computed(() => issuesQuery.isFetching.value)
const events = computed(() => issues.value.reduce((sum, i) => sum + i.count, 0))
const services = computed(() => new Set(issues.value.map((i) => i.service)).size)

const queryError = ref<{ message: string; offset: number | null } | undefined>(undefined)
watch([() => issuesQuery.errorUpdatedAt.value, () => issuesQuery.dataUpdatedAt.value], () => {
  const e = issuesQuery.error.value as { status?: number; body?: { error?: string; offset?: number } } | null
  if (issuesQuery.isError.value && e?.status === 400) {
    queryError.value = { message: e.body?.error ?? 'invalid query', offset: e.body?.offset ?? null }
  } else if (issuesQuery.isSuccess.value) {
    queryError.value = undefined
  }
})
</script>

<template>
  <AppShell :mock="api.mock" crumb="Backend › Issues">
    <template #toolbar>
      <div class="flex w-full items-center gap-3">
        <NavTabs class="shrink-0 text-xs" aria-label="Backend sections">
          <NavTabItem :to="{ path: '/services', query: route.query }" :active="false">Services</NavTabItem>
          <NavTabItem :to="{ path: '/services/errors', query: route.query }" :active="true">Issues</NavTabItem>
          <NavTabItem :to="{ path: '/services/map', query: route.query }" :active="false">Map</NavTabItem>
        </NavTabs>
        <div class="min-w-0 flex-1">
          <SearchBar
            :model-value="text"
            :catalog="ISSUE_FACET_CATALOG"
            :error="queryError"
            placeholder="Search issues across every service…"
            @update:model-value="text = $event"
          />
        </div>
      </div>
    </template>

    <main class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-5">
      <div class="flex flex-wrap items-center gap-2.5 text-xs text-muted-foreground">
        <span class="font-mono tabular-nums text-foreground/80">{{ formatNumber(issues.length) }} issues</span>
        <span class="text-border">·</span>
        <span class="font-mono tabular-nums">{{ formatNumber(events) }} events</span>
        <span class="text-border">·</span>
        <span class="font-mono tabular-nums">{{ formatNumber(services) }} services</span>
        <span class="text-border">·</span>
        <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
        <Spinner v-if="loading" size="sm">loading…</Spinner>
      </div>

      <IssueList :issues="issues" :loading="loading" show-service />

      <IssueBackfillNote />
    </main>
  </AppShell>
</template>

<script setup lang="ts">
// The Services index: a FLEET BOARD, not a table with a Site column
// (docs/adr/0004-the-services-index-is-a-fleet-board.md).
//
// Three reading layers, in the order a person actually needs them:
//
//   1. The Site band — which Sites are even reporting. A stale Site is tinted and badged, because
//      its rows below are Unknown, not OK. It is also the page's Site filter; there is no separate
//      picker, since a picker that showed no health would make you choose blind.
//   2. KPIs and the attention strip — what is wrong right now, worst first, capped and honest
//      about what the cap hides.
//   3. The table — everything, in one dense row per `(service, environment)`.
//
// One request feeds all three (`GET /api/services/overview`). The previous window is a second copy
// of the same request: without it, a service that was serving 5,000/s and stopped renders as a calm
// grey zero instead of Down.
import { ref, computed, watch, useTemplateRef } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useQuery } from '@tanstack/vue-query'
import AppShell from '@/components/common/AppShell.vue'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import SearchBar from '@/components/common/SearchBar.vue'
import MetricTiles from '@/components/metrics/MetricTiles.vue'
import ServicesTable from '@/components/services/ServicesTable.vue'
import SiteBand from '@/components/services/SiteBand.vue'
import AttentionStrip from '@/components/services/AttentionStrip.vue'
import { Spinner } from '@/components/ui/spinner'
import { Badge } from '@/components/ui/badge'
import { api, type OverviewRow } from '@/lib/core/api'
import { formatCompact, formatDuration, formatNumber } from '@/lib/core/format'
import { useUrlState } from '@/lib/core/useUrlState'
import {
  useServicesOverview,
  useIncidentsByService,
  mergeFleet,
  siteStats,
  levelCounts,
  fleetTotals,
  type FleetRow,
  type SiteSlice,
} from '@/lib/services/servicesOverview'
import { correlate } from '@/lib/core/useCorrelate'
import { grammarTerm } from '@/lib/core/queryLang'
import { SPAN_FIELDS, SPAN_EXAMPLE_QUERIES } from '@/lib/traces/spanFields'
import {
  timeRange,
  customRange,
  startNs,
  endNs,
  prevStartNs,
  prevEndNs,
  windowMs,
  tenant,
  setTenant,
  clearTenant,
} from '@/lib/core/context'

const router = useRouter()
const route = useRoute()

// --- state ---
// Time, Site and Environment are global (ContextBar in AppShell); only the search text is local.
const text = ref('')
useUrlState({ text })

const servicesQuery = useQuery({
  queryKey: ['services'],
  queryFn: ({ signal }) => api.services({ signal }),
  staleTime: 5 * 60 * 1000,
})
const servicesList = computed(() => servicesQuery.data.value ?? [])

// --- data ---
// 30 buckets is the sparkline resolution; the table renders one per row.
const now = useServicesOverview(() => text.value.trim(), startNs, endNs, 30)
// The previous window is fetched WITHOUT sparklines (1 bucket): it exists only so `serviceLevel`
// can tell Silence ("it reported, then stopped") from never-reported, and 30 unused series per
// service is a real cost on a fleet.
const prev = useServicesOverview(() => text.value.trim(), prevStartNs, prevEndNs, 1)
const incidentsByService = useIncidentsByService()

const loading = computed(() => now.isFetching.value)
const prevRows = computed<OverviewRow[]>(() => prev.data.value?.rows ?? [])
const rows = computed<FleetRow[]>(() =>
  mergeFleet(now.data.value, { prev: prevRows.value, incidentsByService: incidentsByService.value }),
)
const sites = computed(() => siteStats(now.data.value, rows.value))
// `rollup` means the numbers came from pre-aggregated `photon.service.*` metrics rather than a raw
// span scan. Surfaced because a rollup bug should show up as an attributable number.
const source = computed(() => now.data.value?.source ?? 'raw')

// 400 surfacing: map the overview query error to a fresh { message, offset } for the SearchBar
// underline (same pattern as the other explorer views).
const queryError = ref<{ message: string; offset: number | null } | null>(null)
watch(
  () => [now.error.value, now.errorUpdatedAt.value, now.dataUpdatedAt.value],
  () => {
    const e = now.error.value as { status?: number; body?: { error?: string; offset?: number } } | null
    queryError.value =
      e && e.status === 400
        ? { message: e.body?.error ?? 'invalid query', offset: e.body?.offset ?? null }
        : null
  },
)

// --- KPIs ---
const windowSecs = computed(() => windowMs.value / 1000)
const comparisonLabel = computed(() => `vs prev ${customRange.value ? 'window' : timeRange.value}`)

/** Signed fraction (cur - prev)/prev, or null when there is nothing comparable to divide by. */
function delta(cur: number, before: number): number | null {
  if (!before || !Number.isFinite(before)) return null
  return (cur - before) / before
}

const counts = computed(() => levelCounts(rows.value))
const totals = computed(() => fleetTotals(rows.value))
// The previous window is not merged (no Spread, no levels needed) — only its raw sums are.
const prevTotals = computed(() => {
  const requests = prevRows.value.reduce((a, r) => a + (r.count ?? 0), 0)
  const errors = prevRows.value.reduce((a, r) => a + (r.error_count ?? 0), 0)
  return {
    requests,
    errorRate: requests > 0 ? errors / requests : 0,
    p99: prevRows.value.reduce((a, r) => Math.max(a, Number(r.p99 ?? 0)), 0),
  }
})

const openIncidents = computed(() =>
  Object.values(incidentsByService.value).reduce((a, list) => a + list.length, 0),
)
const spans = computed(() => rows.value.reduce((a, r) => a + r.span_count, 0))
// Fleet Apdex is recomputed from the bands, never averaged: the mean of per-service scores weights
// a service serving 4 requests the same as one serving 4 million.
const fleetApdex = computed(() => {
  const s = rows.value.reduce((a, r) => a + r.satisfied, 0)
  const t = rows.value.reduce((a, r) => a + r.tolerating, 0)
  const f = rows.value.reduce((a, r) => a + r.frustrated, 0)
  const banded = s + t + f
  return banded > 0 ? (s + t / 2) / banded : null
})

const tiles = computed(() => {
  const rate = totals.value.requests / windowSecs.value
  return [
    {
      label: 'Requests',
      value: formatCompact(totals.value.requests),
      sub: `${formatCompact(rate)}/s · ${formatCompact(spans.value)} spans`,
      delta: delta(totals.value.requests, prevTotals.value.requests),
      tone: 'neutral',
    },
    {
      label: 'Error rate',
      value: (totals.value.errorRate * 100).toFixed(2) + '%',
      sub: `${formatCompact(rows.value.reduce((a, r) => a + r.error_count, 0))} errors`,
      delta: delta(totals.value.errorRate, prevTotals.value.errorRate),
      tone: 'up-bad',
    },
    {
      label: 'p99 latency',
      value: formatDuration(totals.value.p99),
      sub: 'worst service',
      delta: delta(totals.value.p99, prevTotals.value.p99),
      tone: 'up-bad',
    },
    {
      label: 'Apdex',
      value: fleetApdex.value == null ? '—' : fleetApdex.value.toFixed(2),
      sub: 'weighted by requests',
      delta: null,
      tone: 'up-good',
    },
    {
      label: 'Open incidents',
      value: formatNumber(openIncidents.value),
      sub: openIncidents.value ? 'firing now' : 'none firing',
      delta: null,
      tone: 'up-bad',
    },
  ]
})

// --- handlers ---
const tableEl = useTemplateRef<HTMLElement>('tableEl')

function onSelectSite(key: string | null): void {
  if (key) setTenant(key)
  else clearTenant()
}
function openService(service: string, environment?: string | null): void {
  const query = environment ? { env: environment } : undefined
  router.push({ path: '/services/' + encodeURIComponent(service), query })
}
function onOverflow(): void {
  tableEl.value?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}
// Opening one Site's slice narrows the whole board to that Site first, so the detail page you land
// on describes the same thing the child row did.
function onOpenSite(slice: SiteSlice): void {
  if (slice.tenant) setTenant(slice.tenant)
  openService(slice.service, slice.environment)
}
// Operations have no route of their own until the detail sub-routes land; until then the honest
// destination is the traces explorer, filtered to that operation and sorted slowest-first.
function onOpenOperation({ service, operation }: { service: string; operation: string }): void {
  router.push(
    correlate({
      path: '/traces',
      query: {
        q: [grammarTerm('service.name', service), grammarTerm('name', operation)].filter(Boolean).join(' '),
        sort: 'slowest',
      },
    }),
  )
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Backend › Services">
    <!-- Services · Issues: two readings of the same fleet, so they share one bar. The Issues
         list is not a filter of this table (it groups errored spans, not services), which is why
         it is a sibling tab rather than a mode toggle. -->
    <template #toolbar>
      <div class="flex w-full items-center gap-3">
        <NavTabs class="shrink-0 text-xs" aria-label="Backend sections">
          <NavTabItem :to="{ path: '/services', query: route.query }" :active="true">Services</NavTabItem>
          <NavTabItem :to="{ path: '/services/errors', query: route.query }" :active="false">Issues</NavTabItem>
          <NavTabItem :to="{ path: '/services/map', query: route.query }" :active="false">Map</NavTabItem>
        </NavTabs>
        <div class="min-w-0 flex-1">
          <SearchBar
            :model-value="text"
            :services="servicesList"
            :error="queryError ?? undefined"
            :catalog="SPAN_FIELDS"
            :example-queries="SPAN_EXAMPLE_QUERIES"
            @update:model-value="text = $event"
          />
        </div>
      </div>
    </template>

    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
      <!-- 1. Which Sites are reporting (central only — a Site install has no registry). -->
      <SiteBand :sites="sites" :active="tenant" @select="onSelectSite" />

      <!-- 2. What is wrong. -->
      <MetricTiles :tiles="tiles" :comparison-label="comparisonLabel" :cols="5" />

      <AttentionStrip :rows="rows" :max="8" @open="openService($event.service, $event.environment)" @overflow="onOverflow" />

      <!-- 3. Everything. -->
      <div ref="tableEl" class="flex flex-col gap-2">
        <div class="flex flex-wrap items-center gap-2.5 text-xs text-muted-foreground">
          <span class="font-mono tabular-nums text-foreground/80">{{ formatNumber(rows.length) }} services</span>
          <span class="text-border">·</span>
          <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
          <template v-for="k in (['down', 'critical', 'warning', 'unknown'] as const)" :key="k">
            <Badge v-if="counts[k]" :data-level="k" variant="outline" class="px-1.5 py-0 text-[10px] capitalize">
              {{ counts[k] }} {{ k }}
            </Badge>
          </template>
          <Badge
            v-if="source === 'rollup'"
            variant="secondary"
            data-testid="source-badge"
            class="px-1.5 py-0 text-[10px]"
            title="Answered from pre-aggregated photon.service.* metrics rather than a raw span scan"
          >
            rollup
          </Badge>
          <Spinner v-if="loading" size="sm">loading…</Spinner>
        </div>

        <ServicesTable
          :rows="rows"
          :loading="loading"
          :window-secs="windowSecs"
          :start-ns="startNs"
          :end-ns="endNs"
          @open-service="openService($event.service, $event.environment)"
          @open-site="onOpenSite"
          @open-operation="onOpenOperation"
        />
      </div>
    </main>
  </AppShell>
</template>

<script setup>
import { ref, computed, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useQuery } from '@tanstack/vue-query'
import AppShell from '@/components/common/AppShell.vue'
import SearchBar from '@/components/common/SearchBar.vue'
import MetricTiles from '@/components/metrics/MetricTiles.vue'
import ServicesTable from '@/components/services/ServicesTable.vue'
import ServiceHealthCounts from '@/components/services/ServiceHealthCounts.vue'
import AttentionStrip from '@/components/services/AttentionStrip.vue'
import ChartPanel from '@/components/charts/ChartPanel.vue'
import SpanVolumeHistogram from '@/components/traces/SpanVolumeHistogram.vue'
import LatencyHistogram from '@/components/traces/LatencyHistogram.vue'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { formatDuration, formatNumber } from '@/lib/core/format'
import { useUrlState } from '@/lib/core/useUrlState'
import { useServicesList } from '@/lib/services/servicesQueries'
import { useTracesLatency } from '@/lib/traces/tracesQueries'
import { setDurationRange } from '@/lib/core/queryLang'
import { SPAN_FIELDS, SPAN_EXAMPLE_QUERIES } from '@/lib/traces/spanFields'
import {
  timeRange,
  customRange,
  startNs,
  endNs,
  startMs,
  endMs,
  windowMs,
  prevStartNs,
  prevEndNs,
  setCustomRange,
} from '@/lib/core/context'

// Services (APM) list — Task 11. Adapted from `REDMetricsView.vue`'s "by service" RED table, but
// as its own top-level page: no operation/service toggle (this table is always service-level) and
// no sub-view tab bar (Services lives outside the Traces section). `useServicesList` returns the
// same RED-shaped rows as `useRed(..., 'service')` plus a per-row `apdex` score consumed by
// `ServicesTable`'s Apdex column.
const router = useRouter()

// --- state ---
// Time is now global (lib/context.js, driven by the ContextBar in AppShell — Task 8) — only the
// search text is owned locally.
const text = ref('')

// --- URL persistence: text only now (timeRange/customRange are synced by context.js itself). ---
useUrlState({ text })

// --- services (search-bar autocomplete) ---
const servicesQuery = useQuery({
  queryKey: ['services'],
  queryFn: ({ signal }) => api.services({ signal }),
  staleTime: 5 * 60 * 1000,
})
const servicesList = computed(() => servicesQuery.data.value ?? [])

// --- data: services RED current/previous + latency current/previous (p99 KPI reuses /api/traces/latency) ---
const svcNow = useServicesList(() => text.value.trim(), startNs, endNs)
const svcPrev = useServicesList(() => text.value.trim(), prevStartNs, prevEndNs)
const latencyNow = useTracesLatency(() => text.value.trim(), startNs, endNs, 1)
const latencyPrev = useTracesLatency(() => text.value.trim(), prevStartNs, prevEndNs, 1)

const rows = computed(() => svcNow.data.value ?? [])
const prevRows = computed(() => svcPrev.data.value ?? [])
const loading = computed(() => svcNow.isFetching.value)

// 400 surfacing: map the services-list query error to a fresh { message, offset } for the
// SearchBar underline (same pattern as REDMetricsView).
const queryError = ref(null)
watch(
  () => [svcNow.error.value, svcNow.errorUpdatedAt.value, svcNow.dataUpdatedAt.value],
  () => {
    const e = svcNow.error.value
    queryError.value =
      e && e.status === 400
        ? { message: e.body?.error ?? 'invalid query', offset: e.body?.offset ?? null }
        : null
  },
)

// --- KPI derivation ---
function totals(list) {
  const arr = list ?? []
  const count = arr.reduce((s, r) => s + (r.count ?? 0), 0)
  const errors = arr.reduce((s, r) => s + (r.error_count ?? 0), 0)
  return { count, errors }
}
// Signed fraction (cur - prev)/prev, or null when there is no comparable previous value.
function delta(cur, prev) {
  if (prev == null || prev === 0 || !Number.isFinite(prev)) return null
  return (cur - prev) / prev
}

const windowSecs = computed(() => windowMs.value / 1000)
// Label under each KPI value naming the comparison window ("vs prev 30m" / "vs prev window").
const comparisonLabel = computed(() => `vs prev ${customRange.value ? 'window' : timeRange.value}`)
// The previous window is, by construction (context.js), the same length as the current one — so
// the previous-window rate denominator reuses `windowSecs` (no separate prevWindowSecs needed).

const tiles = computed(() => {
  const now = totals(svcNow.data.value)
  const prev = totals(svcPrev.data.value)
  const rateNow = now.count / windowSecs.value
  const ratePrev = prev.count / windowSecs.value
  const errNow = now.count ? now.errors / now.count : 0
  const errPrev = prev.count ? prev.errors / prev.count : 0
  const p99Now = Number(latencyNow.data.value?.p99 ?? 0)
  const p99Prev = Number(latencyPrev.data.value?.p99 ?? 0)
  return [
    {
      label: 'Request rate',
      value: (rateNow >= 100 ? formatNumber(Math.round(rateNow)) : rateNow.toFixed(1)) + '/s',
      delta: delta(rateNow, ratePrev),
      tone: 'neutral',
    },
    {
      label: 'Error rate',
      value: (errNow * 100).toFixed(2) + '%',
      delta: delta(errNow, errPrev),
      tone: 'up-bad',
    },
    {
      label: 'p99 latency',
      value: formatDuration(p99Now),
      delta: delta(p99Now, p99Prev),
      tone: 'up-bad',
    },
    {
      label: 'Requests',
      value: formatNumber(now.count),
      delta: delta(now.count, prev.count),
      tone: 'neutral',
    },
  ]
})

// --- handlers ---
// Dragging a duration band on the latency chart rewrites the search text to a removable
// `duration>=A duration<=B` pill — the text feeds every query, so the services table, the charts,
// and the KPIs all refetch against the new filter (same as typing it by hand).
function onLatencyBrush({ minNs, maxNs }) {
  text.value = setDurationRange(text.value, minNs, maxNs)
}
// Row click → service detail page.
function onOpenService(service) {
  router.push('/services/' + encodeURIComponent(service))
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Backend › Services">
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

    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <!-- fleet health counts -->
      <div class="px-5 pt-5">
        <ServiceHealthCounts :rows="rows" />
      </div>

      <!-- KPI row -->
      <div class="px-5 pt-3">
        <MetricTiles :tiles="tiles" :comparison-label="comparisonLabel" />
      </div>

      <!-- needs attention (auto-hides when the fleet is all-healthy) -->
      <div class="px-5 pt-4 empty:hidden">
        <AttentionStrip
          :rows="rows"
          :prev-rows="prevRows"
          :start-ns="startNs"
          :end-ns="endNs"
          @open-service="onOpenService"
        />
      </div>

      <!-- meta row: service count + range -->
      <div class="flex items-center gap-2.5 px-5 pb-2 pt-5 text-xs text-muted-foreground">
        <span class="font-mono tabular-nums text-foreground/80">{{ formatNumber(rows.length) }} services</span>
        <span class="text-border">·</span>
        <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
        <Spinner v-if="loading" size="sm">loading…</Spinner>
      </div>

      <div class="px-5">
        <ServicesTable :rows="rows" :prev-rows="prevRows" :loading="loading" @open-service="onOpenService" />
      </div>

      <!-- exploratory charts, now secondary (below the table) -->
      <div class="grid grid-cols-1 gap-4 px-5 pb-5 pt-6 lg:grid-cols-2">
        <ChartPanel title="Request volume &amp; errors" subtitle="Stacked by status · drag to zoom the time range">
          <SpanVolumeHistogram :query="text.trim()" :start-ms="startMs" :end-ms="endMs" @zoom="setCustomRange" />
        </ChartPanel>
        <ChartPanel title="Latency distribution" subtitle="Span durations · drag to filter by duration">
          <LatencyHistogram :query="text.trim()" :start-ms="startMs" :end-ms="endMs" @brush="onLatencyBrush" />
        </ChartPanel>
      </div>
    </main>
  </AppShell>
</template>

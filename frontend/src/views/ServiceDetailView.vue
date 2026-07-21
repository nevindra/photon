<!-- frontend/src/views/ServiceDetailView.vue -->
<script setup>
// Per-service APM dashboard (`/services/:service`): KPI row, four time-series charts, a Key
// operations table (RED grouped by operation, scoped to this service), and Database/External
// dependency tables. Time is global (lib/context.js, driven by the ContextBar in AppShell — Task
// 8) — this view derives its current/previous windows from there, so "last 30m" and the KPI trend
// chips mean the same thing everywhere. Mounting here also sets the app-wide entity SCOPE to this
// service (surfaced as a chip in the ContextBar, cleared by the user via its ✕).
import { computed, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import ChartPanel from '@/components/charts/ChartPanel.vue'
import MetricChart from '@/components/metrics/MetricChart.vue'
import HealthBanner from '@/components/services/HealthBanner.vue'
import ServiceVolumeChart from '@/components/services/ServiceVolumeChart.vue'
import ApdexBandChart from '@/components/services/ApdexBandChart.vue'
import { serviceStatus } from '@/lib/services/serviceHealth'
import MetricTiles from '@/components/metrics/MetricTiles.vue'
import RedTable from '@/components/metrics/RedTable.vue'
import ApdexThresholdControl from '@/components/services/ApdexThresholdControl.vue'
import DependencyTable from '@/components/services/DependencyTable.vue'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { ArrowLeft } from 'lucide-vue-next'
import { api } from '@/lib/core/api'
import { formatDuration, formatNumber } from '@/lib/core/format'
import { useRed } from '@/lib/traces/tracesQueries'
import { useServiceTimeseries, useServiceDependencies } from '@/lib/services/servicesQueries'
import {
  timeRange,
  customRange,
  startNs,
  endNs,
  startMs,
  endMs,
  prevStartNs,
  prevEndNs,
  setScope,
} from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'

const route = useRoute()
const router = useRouter()

const service = computed(() => {
  const s = route.params.service
  return ((Array.isArray(s) ? s[0] : s) ?? '').trim()
})

// Scope the app-wide context to this service. Keyed off `service` (not a one-shot onMounted) so it
// stays in sync if the `:service` route param ever changes under an already-mounted instance (Vue
// Router reuses the component for same-route param navigations).
watch(
  service,
  (s) => {
    if (s) setScope({ type: 'service', id: s, label: s })
  },
  { immediate: true },
)

function backToServices() {
  router.push('/services')
}

// --- data ---
// ONE `useServiceTimeseries` call (48 buckets) feeds all four charts. `buckets=1` on the
// current/previous windows collapses to a single whole-window aggregate for the KPI tiles — the
// same trick REDMetricsView plays via `useTracesLatency(..., 1)` for its p99 tile.
const timeseries = useServiceTimeseries(service, startNs, endNs, 48)
const kpiNow = useServiceTimeseries(service, startNs, endNs, 1)
const kpiPrev = useServiceTimeseries(service, prevStartNs, prevEndNs, 1)
const dependencies = useServiceDependencies(service, startNs, endNs)

// Key operations: RED grouped by operation, scoped to this service via the grammar's raw
// `service.name:<svc>` attribute term (`service.name`/`name` and the friendlier `service`/
// `operation` aliases resolve identically — see photon-core's SpanFieldResolver::resolve_field).
const keyOpsQuery = useRed(() => `service.name:${service.value}`, startNs, endNs, 'operation')
const keyOps = computed(() => keyOpsQuery.data.value ?? [])
const keyOpsLoading = computed(() => keyOpsQuery.isFetching.value)

const databaseRows = computed(() => dependencies.data.value?.database ?? [])
const externalRows = computed(() => dependencies.data.value?.external ?? [])

// Signed fraction (cur - prev)/prev, or null when there is no comparable previous value.
function delta(cur, prev) {
  if (prev == null || prev === 0 || !Number.isFinite(prev)) return null
  return (cur - prev) / prev
}

const comparisonLabel = computed(() => `vs prev ${customRange.value ? 'window' : timeRange.value}`)

const tiles = computed(() => {
  const now = kpiNow.data.value?.[0]
  const prev = kpiPrev.data.value?.[0]
  const rateNow = now?.rate ?? 0
  const ratePrev = prev?.rate ?? 0
  const errNow = now?.error_rate ?? 0
  const errPrev = prev?.error_rate ?? 0
  const p99Now = Number(now?.p99 ?? 0)
  const p99Prev = Number(prev?.p99 ?? 0)
  const apdexNow = now?.apdex ?? null
  const apdexPrev = prev?.apdex ?? null
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
      label: 'Apdex',
      value: apdexNow != null ? apdexNow.toFixed(2) : '—',
      // Higher Apdex is better → up-good (up=green / down=red), enabled in MetricTiles.
      delta: apdexNow != null && apdexPrev != null ? delta(apdexNow, apdexPrev) : null,
      tone: 'up-good',
    },
  ]
})

// Banner: whole-window status + a plain-language "why" line (reasons + trend hints + frustrated %).
const bannerStatus = computed(() => serviceStatus(kpiNow.data.value?.[0] ?? {}))

function trendHint(cur, prev) {
  const d = delta(cur, prev)
  if (d == null) return ''
  return ` ${d > 0 ? '▲' : '▼'}${Math.abs(d * 100).toFixed(0)}%`
}
const bannerReasons = computed(() => {
  const now = kpiNow.data.value?.[0]
  const prev = kpiPrev.data.value?.[0]
  // No traffic in the window → no "why" line; the banner falls back to its calm state (rather than
  // emitting a noisy "p99 0ms" for an idle service).
  if (!now || !(now.count > 0)) return []
  const out = []
  const er = now.error_rate ?? 0
  if (er > 0) out.push(`Error rate ${(er * 100).toFixed(1)}%${trendHint(er, prev?.error_rate)}`)
  const p99 = Number(now.p99 ?? 0)
  out.push(`p99 ${formatDuration(p99)}${trendHint(p99, Number(prev?.p99 ?? 0))}`)
  if (now.apdex != null) {
    const banded = (now.satisfied ?? 0) + (now.tolerating ?? 0) + (now.frustrated ?? 0)
    const frustPct = banded > 0 ? Math.round(((now.frustrated ?? 0) / banded) * 100) : null
    out.push(`Apdex ${now.apdex.toFixed(2)}${frustPct != null ? ` (${frustPct}% frustrated)` : ''}`)
  }
  return out
})

// --- chart series, all derived from the one `timeseries` fetch ---
const buckets = computed(() => timeseries.data.value ?? [])
const chartsLoading = computed(() => timeseries.isFetching.value)

const errorRateSeries = computed(() => {
  if (!buckets.value.length) return []
  // 0-1 fraction, NOT pre-scaled — MetricChart's `percent` prop multiplies by 100 itself;
  // pre-scaling here would double-scale the chart. No yRange here → axis auto-ranges so a
  // low error line stays readable (unlike the utilization panels, which pin [0,100]).
  return [{ labels: {}, points: buckets.value.map((b) => ({ t: b.ts, v: b.error_rate ?? 0 })), exemplars: [] }]
})

const latencySeries = computed(() => {
  if (!buckets.value.length) return []
  return ['p50', 'p90', 'p99'].map((k) => ({
    labels: { percentile: k },
    points: buckets.value.map((b) => ({ t: b.ts, v: b[k] != null ? Number(b[k]) / 1e6 : null })),
    exemplars: [],
  }))
})

// Exemplar pivot from a Key-operations row into the Traces explorer, slowest first. Operation
// names containing whitespace/comma can't be expressed as a `name:` term (the grammar splits OR
// lists on `,` and terms on whitespace) — REDMetricsView's onOpenExemplars uses the same guard.
function onOpenExemplars({ operation }) {
  let q = `service.name:${service.value}`
  if (operation && !/[\s,]/.test(operation)) q += ` name:${operation}`
  router.push(correlate({ path: '/traces', query: { q, sort: 'slowest' } }))
}

// Dependency row → the service's slowest traces in the Traces explorer. (Service-scoped; a
// dependency-scoped filter can follow once a peer/db-system grammar field is confirmed.)
function onOpenDependencyTraces() {
  router.push(correlate({ path: '/traces', query: { q: `service.name:${service.value}`, sort: 'slowest' } }))
}
</script>

<template>
  <AppShell :mock="api.mock" :crumb="`Backend › ${service}`">
    <!-- Back button folds into the ContextBar's lead slot; the crumb already reads "Backend › {service}". -->
    <template #lead>
      <Button
        data-testid="back-to-services"
        size="icon"
        variant="ghost"
        class="size-7 shrink-0"
        aria-label="Back to services"
        @click="backToServices"
      >
        <ArrowLeft class="size-4" />
      </Button>
    </template>
    <!-- View-specific actions fold into the ContextBar's actions slot (before the time picker). -->
    <template #actions>
      <div v-if="service" class="flex items-center gap-2">
        <RelatedMenu :entity="{ kind: 'service', fields: { service } }" />
        <ApdexThresholdControl :service="service" />
      </div>
    </template>

    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <!-- health banner -->
      <div class="px-5 pt-5">
        <HealthBanner :status="bannerStatus" :reasons="bannerReasons" />
      </div>

      <!-- KPI row -->
      <div class="px-5 pt-4">
        <MetricTiles :tiles="tiles" :comparison-label="comparisonLabel" />
      </div>

      <!-- What changed: volume (absolute) · error % · latency · apdex bands -->
      <div class="grid grid-cols-1 gap-4 px-5 pt-4 lg:grid-cols-2">
        <ChartPanel title="Request volume" subtitle="Requests + errors · absolute count">
          <ServiceVolumeChart :buckets="buckets" :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" />
        </ChartPanel>
        <ChartPanel title="Error %" subtitle="Share of requests that errored">
          <MetricChart :series="errorRateSeries" percent :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" />
        </ChartPanel>
        <ChartPanel title="Latency" subtitle="p50 / p90 / p99 · ms">
          <MetricChart :series="latencySeries" unit="ms" :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" />
        </ChartPanel>
        <ChartPanel title="Apdex bands" subtitle="Satisfied / tolerating / frustrated">
          <ApdexBandChart :buckets="buckets" :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" />
        </ChartPanel>
      </div>

      <!-- Where is the problem? -->
      <div class="flex items-center gap-2.5 px-5 pb-2 pt-6 text-xs font-medium uppercase tracking-wider text-muted-foreground">
        Where is the problem?
      </div>
      <div class="flex items-center gap-2.5 px-5 pb-2 text-xs text-muted-foreground">
        <span class="font-mono tabular-nums text-foreground/80">{{ formatNumber(keyOps.length) }} operations</span>
        <span class="text-border">·</span>
        <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
        <Spinner v-if="keyOpsLoading" size="sm">loading…</Spinner>
      </div>
      <RedTable :rows="keyOps" group="operation" :loading="keyOpsLoading" @open-exemplars="onOpenExemplars" />

      <!-- Dependencies (clickable → traces) -->
      <div class="grid grid-cols-1 gap-4 px-5 py-5 lg:grid-cols-2">
        <DependencyTable title="Database calls" :rows="databaseRows" @open-traces="onOpenDependencyTraces" />
        <DependencyTable title="External calls" :rows="externalRows" @open-traces="onOpenDependencyTraces" />
      </div>
    </main>
  </AppShell>
</template>

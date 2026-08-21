<script setup lang="ts">
// `/services/:service` — the service Overview tab.
//
// It answers one question, in this order: is this service healthy, what changed, and where is the
// problem. Everything that answers a DIFFERENT question moved to its own sub-route (Operations,
// Dependencies), so this page paints without waiting on a dependency scan.
//
// The **Logs panel** is the piece that was missing. A service can be on fire in its logs while its
// spans look normal — a failing background job, a config parse error, a connection-pool warning
// every 200ms produce no errored SERVER span and move no number on the RED charts. Four latency
// charts and no log volume is a health page that can be green during an outage.
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import ServiceScaffold from '@/components/services/ServiceScaffold.vue'
import ChartPanel from '@/components/charts/ChartPanel.vue'
import MetricChart from '@/components/metrics/MetricChart.vue'
import HealthBanner from '@/components/services/HealthBanner.vue'
import ServiceVolumeChart from '@/components/services/ServiceVolumeChart.vue'
import ApdexBandChart from '@/components/services/ApdexBandChart.vue'
import ServiceLogPanel from '@/components/services/ServiceLogPanel.vue'
import SloPanel from '@/components/services/SloPanel.vue'
import OperationsTable from '@/components/services/OperationsTable.vue'
import MetricTiles from '@/components/metrics/MetricTiles.vue'
import { Badge } from '@/components/ui/badge'
import { Spinner } from '@/components/ui/spinner'
import { serviceHealth } from '@/lib/services/serviceHealth'
import { useDeployments, useServiceTimeseries, useServiceOperations } from '@/lib/services/servicesQueries'
import { useServiceParam } from '@/lib/services/useServiceParam'
import { deployMarkers } from '@/lib/services/deployMarkers'
import { grammarTerm } from '@/lib/core/queryLang'
import { formatCompact, formatDuration } from '@/lib/core/format'
import {
  timeRange, customRange, startNs, endNs, startMs, endMs, prevStartNs, prevEndNs, windowMs,
} from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'
import type { RedRow } from '@/lib/core/api'

const router = useRouter()
const service = useServiceParam()

// ONE 48-bucket read feeds all four charts; `buckets=1` collapses to whole-window KPIs.
const timeseries = useServiceTimeseries(service, startNs, endNs, 48)
const kpiNow = useServiceTimeseries(service, startNs, endNs, 1)
const kpiPrev = useServiceTimeseries(service, prevStartNs, prevEndNs, 1)
const opsQuery = useServiceOperations(service, startNs, endNs)
// Releases, drawn on every chart below. A step in a line and a release at the same x is the
// cheapest cause-and-effect this page can offer, and it costs one small control-plane read.
const deploysQuery = useDeployments(service, startNs, endNs)
const markers = computed(() => deployMarkers(deploysQuery.data.value?.deployments ?? []))

const now = computed(() => kpiNow.data.value?.[0])
const prev = computed(() => kpiPrev.data.value?.[0])
const buckets = computed(() => timeseries.data.value ?? [])
const chartsLoading = computed(() => timeseries.isFetching.value)
const windowSecs = computed(() => windowMs.value / 1000)

// Key operations: the busiest ENTRY points. The full list — including the work this service does
// rather than serves — is one click away on the Operations tab, which is where that distinction
// gets the explanation it needs.
const keyOps = computed<RedRow[]>(() =>
  [...(opsQuery.data.value ?? [])]
    .filter((r) => (r.basis ?? 'entry') === 'entry')
    .sort((a, b) => (b.count ?? 0) - (a.count ?? 0))
    .slice(0, 8),
)
const opsLoading = computed(() => opsQuery.isFetching.value)

function delta(cur: number, before: number): number | null {
  if (!before || !Number.isFinite(before)) return null
  return (cur - before) / before
}
const comparisonLabel = computed(() => `vs prev ${customRange.value ? 'window' : timeRange.value}`)

const tiles = computed(() => {
  const n = now.value
  const p = prev.value
  const rate = (n?.count ?? 0) / windowSecs.value
  const prevRate = (p?.count ?? 0) / windowSecs.value
  return [
    {
      label: 'Requests',
      value: formatCompact(n?.count ?? 0),
      sub: `${formatCompact(rate)}/s · ${formatCompact(n?.span_count ?? 0)} spans`,
      delta: delta(rate, prevRate),
      tone: 'neutral',
    },
    {
      label: 'Error rate',
      value: ((n?.error_rate ?? 0) * 100).toFixed(2) + '%',
      sub: n?.http_5xx || n?.http_4xx ? `${formatCompact(n?.http_5xx ?? 0)} 5xx · ${formatCompact(n?.http_4xx ?? 0)} 4xx` : undefined,
      delta: delta(n?.error_rate ?? 0, p?.error_rate ?? 0),
      tone: 'up-bad',
    },
    {
      label: 'p99 latency',
      value: formatDuration(Number(n?.p99 ?? 0)),
      sub: `p50 ${formatDuration(Number(n?.p50 ?? 0))} · p90 ${formatDuration(Number(n?.p90 ?? 0))}`,
      delta: delta(Number(n?.p99 ?? 0), Number(p?.p99 ?? 0)),
      tone: 'up-bad',
    },
    {
      label: 'Apdex',
      value: n?.apdex == null ? '—' : n.apdex.toFixed(2),
      sub: bandSub.value,
      delta: n?.apdex != null && p?.apdex != null ? delta(n.apdex, p.apdex) : null,
      tone: 'up-good',
    },
  ]
})

const bandSub = computed(() => {
  const n = now.value
  if (!n) return undefined
  const banded = (n.satisfied ?? 0) + (n.tolerating ?? 0) + (n.frustrated ?? 0)
  if (!banded) return undefined
  return `${Math.round(((n.frustrated ?? 0) / banded) * 100)}% frustrated`
})

// Banner: whole-window Attention Level plus a plain-language "why". The previous window rides
// along so a service that reported and then stopped reads Down here too, rather than a calm banner
// over an empty chart.
const banner = computed(() =>
  serviceHealth(
    { count: now.value?.count, rate: now.value?.rate, error_rate: now.value?.error_rate, apdex: now.value?.apdex ?? null },
    { prev: prev.value ? { count: prev.value.count, rate: prev.value.rate } : undefined },
  ),
)

function trendHint(cur: number, before: number): string {
  const d = delta(cur, before)
  if (d == null) return ''
  return ` ${d > 0 ? '▲' : '▼'}${Math.abs(d * 100).toFixed(0)}%`
}
const bannerReasons = computed<string[]>(() => {
  const n = now.value
  const p = prev.value
  // No traffic → the model's own line ("No spans 340/s" or "No traffic"), not a noisy "p99 0ms".
  if (!n || !(n.count > 0)) return banner.value.text
  const out: string[] = []
  const er = n.error_rate ?? 0
  if (er > 0) out.push(`Error rate ${(er * 100).toFixed(1)}%${trendHint(er, p?.error_rate ?? 0)}`)
  const p99 = Number(n.p99 ?? 0)
  out.push(`p99 ${formatDuration(p99)}${trendHint(p99, Number(p?.p99 ?? 0))}`)
  if (n.apdex != null) out.push(`Apdex ${n.apdex.toFixed(2)}${bandSub.value ? ` (${bandSub.value})` : ''}`)
  return out
})

// --- chart series, all off the one `timeseries` fetch ---
// Errors BY CLASS, not one error line: a 5xx is this service's own fault and a 4xx is the caller's
// bad request, and one line for both is why "our error rate doubled" turns into an hour of work to
// discover somebody else shipped a broken client.
// `MetricChart` speaks NANOSECONDS on `t` (it is shared with the metrics explorer, whose
// points are ns); these buckets are epoch ms. Without the conversion every point lands in 1970
// and the chart draws an empty window.
const MS_TO_NS = 1e6

const errorSeries = computed(() =>
  !buckets.value.length
    ? []
    : [
        { labels: { class: '5xx' }, points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: b.http_5xx ?? 0 })), exemplars: [] },
        { labels: { class: '4xx' }, points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: b.http_4xx ?? 0 })), exemplars: [] },
        { labels: { class: 'span status' }, points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: b.error_count ?? 0 })), exemplars: [] },
      ],
)

const latencySeries = computed(() =>
  !buckets.value.length
    ? []
    : (['p50', 'p90', 'p99'] as const).map((k) => ({
        labels: { percentile: k },
        points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: b[k] != null ? Number(b[k]) / 1e6 : null })),
        exemplars: [],
      })),
)

// The entry-set basis is the same on every bucket of one series, so the first bucket speaks for
// all of them.
const basis = computed(() => buckets.value[0]?.basis ?? now.value?.basis ?? 'entry')

function openOperation(row: RedRow): void {
  if (!row.operation) return
  router.push({
    path: `/services/${encodeURIComponent(service.value)}/operations/${encodeURIComponent(row.operation)}`,
    query: router.currentRoute.value.query,
  })
}
function openAllOperations(): void {
  router.push({
    path: `/services/${encodeURIComponent(service.value)}/operations`,
    query: router.currentRoute.value.query,
  })
}
function openExemplars(row: RedRow): void {
  const q = [grammarTerm('service.name', service.value), grammarTerm('name', row.operation)]
    .filter(Boolean)
    .join(' ')
  router.push(correlate({ path: '/traces', query: { q, sort: 'slowest' } }))
}
</script>

<template>
  <ServiceScaffold :service="service">
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
      <HealthBanner :level="banner.level" :reasons="bannerReasons" />

      <MetricTiles :tiles="tiles" :comparison-label="comparisonLabel" />

      <!-- Objectives sit directly under the KPIs and above the charts: a budget is the only thing
           on this page that is already a decision, and everything below it is the evidence. -->
      <SloPanel :service="service" />

      <!-- What changed. -->
      <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <ChartPanel title="Request volume" subtitle="Requests + errors · absolute count">
          <ServiceVolumeChart :markers="markers" :buckets="buckets" :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" />
        </ChartPanel>
        <ChartPanel
          title="Errors by class"
          subtitle="5xx is ours · 4xx is the caller's · span status is neither"
        >
          <MetricChart :markers="markers" :series="errorSeries" :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" />
        </ChartPanel>
        <ChartPanel title="Latency" subtitle="p50 / p90 / p99 · ms">
          <MetricChart :markers="markers" :series="latencySeries" unit="ms" :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" />
        </ChartPanel>
        <ChartPanel title="Apdex bands" subtitle="Satisfied / tolerating / frustrated">
          <ApdexBandChart :markers="markers" :buckets="buckets" :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" />
        </ChartPanel>
      </div>

      <!-- Where is the problem: the busiest entry points, and what the logs say. -->
      <div class="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <div class="flex flex-col gap-2">
          <div class="flex flex-wrap items-center gap-2.5 text-xs text-muted-foreground">
            <span class="font-medium uppercase tracking-wider">Key operations</span>
            <Badge
              v-if="basis !== 'entry'"
              variant="secondary"
              data-testid="basis-badge"
              class="px-1.5 py-0 text-[10px]"
              title="This service emits no server or consumer spans, so its request count is every span it recorded"
            >
              span-based
            </Badge>
            <Spinner v-if="opsLoading" size="sm">loading…</Spinner>
            <button type="button" class="ml-auto text-[11px] text-primary hover:underline" @click="openAllOperations">
              All operations ↗
            </button>
          </div>
          <OperationsTable
            :rows="keyOps"
            :loading="opsLoading"
            :window-secs="windowSecs"
            @open="openOperation"
            @open-exemplars="openExemplars"
          />
        </div>

        <ServiceLogPanel :service="service" />
      </div>
    </main>
  </ServiceScaffold>
</template>

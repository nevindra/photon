<script setup lang="ts">
// `/services/:service/operations/:operation` — one operation, in enough detail to act on.
//
// Three things live here that nowhere else in the app can show:
//
//   1. The **latency heatmap**, with the Apdex T and 4T lines drawn on its own axes. Percentile
//      lines cannot show a distribution splitting in two, and the Apdex score cannot show where
//      the split sits. Putting both on one pair of axes is what lets you check one against the
//      other.
//   2. A **response-code table**. `error_rate` collapses 502, 504, 409 and 422 into one number,
//      and those are four different problems with four different owners.
//   3. **Slowest traces**, so the next click is a real waterfall rather than another aggregate.
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import ServiceScaffold from '@/components/services/ServiceScaffold.vue'
import ChartPanel from '@/components/charts/ChartPanel.vue'
import LatencyHeatmap from '@/components/charts/LatencyHeatmap.vue'
import MetricChart from '@/components/metrics/MetricChart.vue'
import MetricTiles from '@/components/metrics/MetricTiles.vue'
import HealthPill from '@/components/services/HealthPill.vue'
import ApdexBandBar from '@/components/services/ApdexBandBar.vue'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { Badge } from '@/components/ui/badge'
import { useQuery } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import { useServiceParam, useRouteParam } from '@/lib/services/useServiceParam'
import {
  useDeployments, useServiceTimeseries, useServiceSettings, useOperationHeatmap,
} from '@/lib/services/servicesQueries'
import { deployMarkers } from '@/lib/services/deployMarkers'
import { serviceLevel } from '@/lib/services/serviceHealth'
import { grammarTerm } from '@/lib/core/queryLang'
import { contextQueryTerm, startNs, endNs, startMs, endMs, prevStartNs, prevEndNs, timeRange, customRange, windowMs } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'
import { formatCompact, formatDuration } from '@/lib/core/format'

const router = useRouter()
const service = useServiceParam()
const operation = useRouteParam('operation')

// The grammar scope this whole page reads through. `grammarTerm` quotes a name containing a space
// (`GET /v1/charges` always does) — unquoted it would parse as a name plus a stray free-text word.
const opTerm = computed(() => grammarTerm('name', operation.value) ?? '')
const scoped = computed(() => [opTerm.value, contextQueryTerm()].filter(Boolean).join(' '))

const settings = useServiceSettings(service)
const thresholdMs = computed(() => settings.data.value?.apdex_threshold_ms ?? null)

// One 48-bucket read drives the two line charts; `buckets=1` collapses to whole-window KPIs.
const series = useServiceTimeseries(service, startNs, endNs, 48)
const kpiNow = useServiceTimeseries(service, startNs, endNs, 1)
const kpiPrev = useServiceTimeseries(service, prevStartNs, prevEndNs, 1)
const heat = useOperationHeatmap(service, operation, startNs, endNs, 48)

// The service timeseries endpoint takes a `q`, so scoping it to one operation needs no new route.
// It is passed through the same `scoped` term every panel here uses.
const opSeries = useQuery({
  queryKey: computed(() => ['service', 'operation-series', service.value, operation.value, startNs.value, endNs.value]),
  queryFn: ({ signal }) =>
    api.serviceTimeseries(service.value, { start: startNs.value, end: endNs.value, buckets: 48, q: scoped.value }, { signal }),
  enabled: computed(() => !!service.value && !!operation.value),
})
const opKpi = useQuery({
  queryKey: computed(() => ['service', 'operation-kpi', service.value, operation.value, startNs.value, endNs.value]),
  queryFn: ({ signal }) =>
    api.serviceTimeseries(service.value, { start: startNs.value, end: endNs.value, buckets: 1, q: scoped.value }, { signal }),
  enabled: computed(() => !!service.value && !!operation.value),
})
const opKpiPrev = useQuery({
  queryKey: computed(() => ['service', 'operation-kpi-prev', service.value, operation.value, prevStartNs.value, prevEndNs.value]),
  queryFn: ({ signal }) =>
    api.serviceTimeseries(service.value, { start: prevStartNs.value, end: prevEndNs.value, buckets: 1, q: scoped.value }, { signal }),
  enabled: computed(() => !!service.value && !!operation.value),
})

// Response codes: a facet over the spans this operation produced. `http.status_code` is the OTel
// semantic-convention key; the facet route resolves the legacy spelling too.
const codeQuery = computed(() =>
  [grammarTerm('service.name', service.value), opTerm.value, contextQueryTerm()].filter(Boolean).join(' '),
)
const codes = useQuery({
  queryKey: computed(() => ['service', 'operation-codes', codeQuery.value, startNs.value, endNs.value]),
  queryFn: ({ signal }) => api.tracesFacet('http.status_code', codeQuery.value, startNs.value, endNs.value, 20, { signal }),
  enabled: computed(() => !!service.value && !!operation.value),
})

const slowest = useQuery({
  queryKey: computed(() => ['service', 'operation-exemplars', codeQuery.value, startNs.value, endNs.value]),
  queryFn: ({ signal }) =>
    api.searchTraces(
      { query: codeQuery.value, start: startNs.value, end: endNs.value, limit: 10, sort: 'slowest' },
      { signal },
    ),
  enabled: computed(() => !!service.value && !!operation.value),
})

const now = computed(() => opKpi.data.value?.[0])
const prev = computed(() => opKpiPrev.data.value?.[0])
const level = computed(
  () =>
    serviceLevel(
      { count: now.value?.count, rate: now.value?.rate, error_rate: now.value?.error_rate, apdex: now.value?.apdex ?? null },
      { prev: prev.value ? { count: prev.value.count, rate: prev.value.rate } : undefined },
    ).level,
)

function delta(cur: number, before: number): number | null {
  if (!before || !Number.isFinite(before)) return null
  return (cur - before) / before
}

const comparisonLabel = computed(() => `vs prev ${customRange.value ? 'window' : timeRange.value}`)
const windowSecs = computed(() => windowMs.value / 1000)

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
      sub: `${formatCompact(n?.error_count ?? 0)} errors`,
      delta: delta(n?.error_rate ?? 0, p?.error_rate ?? 0),
      tone: 'up-bad',
    },
    {
      label: 'p99 latency',
      value: formatDuration(Number(n?.p99 ?? 0)),
      sub: `p50 ${formatDuration(Number(n?.p50 ?? 0))}`,
      delta: delta(Number(n?.p99 ?? 0), Number(p?.p99 ?? 0)),
      tone: 'up-bad',
    },
    {
      label: 'Apdex',
      value: n?.apdex == null ? '—' : n.apdex.toFixed(2),
      sub: thresholdMs ? `T = ${thresholdMs}ms` : undefined,
      delta: n?.apdex != null && p?.apdex != null ? delta(n.apdex, p.apdex) : null,
      tone: 'up-good',
    },
  ]
})

const buckets = computed(() => opSeries.data.value ?? [])
// `MetricChart` speaks NANOSECONDS on `t` (it is shared with the metrics explorer, whose
// points are ns); these buckets are epoch ms. Without the conversion every point lands in 1970
// and the chart draws an empty window.
const MS_TO_NS = 1e6

// Deploys are a SERVICE fact, not an operation one — a release ships every operation at once — so
// the same markers ride on this page's charts too.
const deploysQuery = useDeployments(service, startNs, endNs)
const markers = computed(() => deployMarkers(deploysQuery.data.value?.deployments ?? []))

const latencySeries = computed(() =>
  !buckets.value.length
    ? []
    : (['p50', 'p90', 'p99'] as const).map((k) => ({
        labels: { percentile: k },
        points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: b[k] != null ? Number(b[k]) / 1e6 : null })),
        exemplars: [],
      })),
)
const volumeSeries = computed(() =>
  !buckets.value.length
    ? []
    : [
        { labels: { series: 'requests' }, points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: b.count ?? 0 })), exemplars: [] },
        { labels: { series: 'errors' }, points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: b.error_count ?? 0 })), exemplars: [] },
      ],
)

// Response codes, worst class first: a 5xx is this service's own fault, a 4xx is the caller's.
const CLASS_ORDER: Record<string, number> = { '5': 0, '4': 1, '3': 2, '2': 3 }
const codeRows = computed(() => {
  const values = codes.data.value?.values ?? []
  const total = values.reduce((a, v) => a + (v.count ?? 0), 0)
  return values
    .map((v) => ({ code: String(v.value), count: v.count ?? 0, pct: total ? (v.count ?? 0) / total : 0 }))
    .sort((a, b) => (CLASS_ORDER[a.code[0]] ?? 9) - (CLASS_ORDER[b.code[0]] ?? 9) || b.count - a.count)
})
function codeTone(code: string): string {
  if (code.startsWith('5')) return 'text-sev-error'
  if (code.startsWith('4')) return 'text-sev-warn'
  return 'text-muted-foreground'
}

const traces = computed(() => slowest.data.value?.traces ?? [])
function openTrace(traceId: string): void {
  router.push(correlate({ path: `/traces/${encodeURIComponent(traceId)}` }))
}
function openAllTraces(): void {
  router.push(correlate({ path: '/traces', query: { q: codeQuery.value, sort: 'slowest' } }))
}
</script>

<template>
  <ServiceScaffold :service="service" :crumb-suffix="operation">
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
      <div class="flex flex-wrap items-center gap-2">
        <HealthPill :level="level" />
        <span class="truncate font-mono text-sm text-foreground">{{ operation }}</span>
        <Badge v-if="now && now.basis !== 'entry'" variant="secondary" class="px-1.5 py-0 text-[10px]">
          not an entry point
        </Badge>
        <Spinner v-if="opKpi.isFetching.value" size="sm">loading…</Spinner>
      </div>

      <MetricTiles :tiles="tiles" :comparison-label="comparisonLabel" />

      <ChartPanel
        title="Latency distribution over time"
        subtitle="Each cell is how many requests landed in that latency band. T and 4T are the Apdex thresholds."
      >
        <LatencyHeatmap
          :data="heat.data.value"
          :loading="heat.isPending.value"
          :threshold-ms="thresholdMs"
        />
      </ChartPanel>

      <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <ChartPanel title="Request volume" subtitle="Requests + errors · absolute count">
          <MetricChart :markers="markers" :series="volumeSeries" :start-ms="startMs" :end-ms="endMs" :loading="opSeries.isFetching.value" />
        </ChartPanel>
        <ChartPanel title="Latency" subtitle="p50 / p90 / p99 · ms">
          <MetricChart :markers="markers" :series="latencySeries" unit="ms" :start-ms="startMs" :end-ms="endMs" :loading="opSeries.isFetching.value" />
        </ChartPanel>
      </div>

      <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <!-- Response codes. `error_rate` collapses 502/504/409/422 into one number; those are four
             different problems with four different owners. -->
        <section class="flex flex-col overflow-hidden rounded-xl border border-border bg-card shadow-1">
          <header class="px-4 pb-2 pt-3.5">
            <h3 class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Response codes</h3>
          </header>
          <Table v-if="codeRows.length" container-class="overflow-visible" class="border-collapse font-mono text-xs">
            <TableHeader>
              <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
                <TableHead>Code</TableHead>
                <TableHead class="text-right">Count</TableHead>
                <TableHead class="text-right">Share</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="r in codeRows" :key="r.code" data-testid="code-row" :data-code="r.code" class="border-border/60">
                <TableCell class="py-1.5" :class="codeTone(r.code)">{{ r.code }}</TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatCompact(r.count) }}</TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ (r.pct * 100).toFixed(1) }}%</TableCell>
              </TableRow>
            </TableBody>
          </Table>
          <EmptyState
            v-else
            title="No HTTP status codes"
            description="This operation records no http.status_code attribute — it may not be an HTTP endpoint."
            class="h-auto flex-1"
          />
          <div class="border-t border-border px-4 py-2">
            <ApdexBandBar
              :satisfied="now?.satisfied"
              :tolerating="now?.tolerating"
              :frustrated="now?.frustrated"
              :threshold-ms="thresholdMs"
            />
          </div>
        </section>

        <!-- Slowest traces: the next click should be a real waterfall, not another aggregate. -->
        <section class="flex flex-col overflow-hidden rounded-xl border border-border bg-card shadow-1">
          <header class="flex items-center justify-between px-4 pb-2 pt-3.5">
            <h3 class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Slowest traces</h3>
            <button type="button" class="text-[11px] text-primary hover:underline" @click="openAllTraces">
              All traces ↗
            </button>
          </header>
          <Table v-if="traces.length" container-class="overflow-visible" class="border-collapse font-mono text-xs">
            <TableHeader>
              <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
                <TableHead>Trace</TableHead>
                <TableHead class="text-right">Spans</TableHead>
                <TableHead class="text-right">Duration</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow
                v-for="t in traces"
                :key="t.trace_id"
                data-testid="exemplar-row"
                role="button"
                tabindex="0"
                class="cursor-pointer border-border/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
                @click="openTrace(t.trace_id)"
                @keydown.enter.prevent="openTrace(t.trace_id)"
              >
                <TableCell class="py-1.5 text-foreground/80">{{ t.trace_id.slice(0, 16) }}…</TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ t.span_count }}</TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-foreground/80">{{ formatDuration(Number(t.duration_ns ?? 0)) }}</TableCell>
              </TableRow>
            </TableBody>
          </Table>
          <EmptyState v-else title="No traces in range" description="Widen the time range." class="h-auto flex-1" />
        </section>
      </div>
    </main>
  </ServiceScaffold>
</template>

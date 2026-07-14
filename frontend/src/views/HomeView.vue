<script setup lang="ts">
// Overview dashboard (`/home`) — the cross-signal landing page (Task 13). One glance across the
// three "worlds": a KPI strip fusing backend RED + frontend RUM vitals + infra uptime, two headline
// trend charts, and three per-world panels. Everything is bound to the app-wide time window
// (`lib/context`, driven by the ContextBar in AppShell) and every tile/row drills out via
// `correlate()`, so the selected range + entity scope ride along on every navigation hop.
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import ChartPanel from '@/components/charts/ChartPanel.vue'
import LineChart from '@/components/charts/LineChart.vue'
import RedTable from '@/components/metrics/RedTable.vue'
import { StatTile } from '@/components/ui/stat-tile'
import { Sparkline } from '@/components/ui/sparkline'
import { StatusDot } from '@/components/ui/status-dot'
import { EmptyState } from '@/components/ui/empty-state'
import { api } from '@/lib/core/api'
import { startNs, endNs, startMs, endMs } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'
import { useServicesList, useServiceTimeseries } from '@/lib/services/servicesQueries'
import { useRumApps, useRumVitals } from '@/lib/rum/rumQueries'
import { useMonitors } from '@/lib/uptime/uptimeQueries'
import { formatDuration, formatNumber } from '@/lib/core/format'

const router = useRouter()

// The `api.*` methods (untyped JS) resolve to `unknown`, so the query rows/vitals/monitors are
// treated as `any[]` here — the annotation makes every `.reduce/.map/.filter/.find` callback param
// explicitly `any` (satisfying `noImplicitAny`) without pretending we have real row types.
type Accent = 'success' | 'error' | 'warning' | 'info' | 'neutral'

// --- Backend world: RED grouped by service (whole fleet, no filter) ---
const servicesQuery = useServicesList(() => '', startNs, endNs)
const rows = computed<any[]>(() => servicesQuery.data.value ?? [])
const servicesLoading = computed(() => servicesQuery.isFetching.value)

// --- Frontend world: RUM Core Web Vitals for the first registered app ---
const rumAppsQuery = useRumApps()
const apps = computed<any[]>(() => rumAppsQuery.data.value?.apps ?? [])
const firstApp = computed<string>(() => apps.value[0]?.name ?? '')
const vitalsQuery = useRumVitals(firstApp, startNs, endNs)
const vitals = computed<any[]>(() => vitalsQuery.data.value?.vitals ?? [])

// --- Infra world: uptime monitors ---
const monitorsQuery = useMonitors()
const monitors = computed<any[]>(() => monitorsQuery.data.value ?? [])

// --- Headline-service timeseries feeds both trend charts (busiest service by request rate) ---
const topService = computed<string>(() => {
  const list = rows.value
  if (!list.length) return ''
  return [...list].sort((a, b) => (b.rate ?? 0) - (a.rate ?? 0))[0].service
})
const timeseries = useServiceTimeseries(topService, startNs, endNs, 48)
const buckets = computed<any[]>(() => timeseries.data.value ?? [])
const chartsLoading = computed(() => timeseries.isFetching.value)

// --- KPI derivation (cross-world) ---
// Fleet request rate = Σ per-service rate. Fleet error rate = rate-weighted mean of per-service
// error rates (no `count` needed — proxies volume by rate). Latency headline = worst service p99.
const requestRate = computed(() => rows.value.reduce((s, r) => s + (r.rate ?? 0), 0))
const errorRate = computed(() => {
  const total = requestRate.value
  if (!total) return 0
  return rows.value.reduce((s, r) => s + (r.rate ?? 0) * (r.error_rate ?? 0), 0) / total
})
const worstP99 = computed(() => rows.value.reduce((m, r) => Math.max(m, Number(r.p99 ?? 0)), 0))
const lcpVital = computed(() => vitals.value.find((v) => v.metric === 'web_vitals.lcp'))
const monitorsUp = computed(() => monitors.value.filter((m) => m.last_state === 'up').length)

// req/s: whole numbers at scale, one decimal when small (mirrors RedTable.fmtRate).
function fmtRate(r: number) {
  const n = r ?? 0
  return (n >= 100 ? formatNumber(Math.round(n)) : n.toFixed(1)) + '/s'
}
// A Web-Vital duration in MILLISECONDS → "2.8s" / "620ms".
function fmtMs(ms: number | null | undefined) {
  if (ms == null) return '—'
  return ms >= 1000 ? (ms / 1000).toFixed(2) + 's' : Math.round(ms) + 'ms'
}
// Chart value formatter for the latency series (already ms Numbers).
function fmtMsChart(v: number) {
  return Math.round(v) + 'ms'
}

// Web-Vital rating → StatTile/StatusDot tone. Ratings arrive as 'good' | 'needs'(-improvement) |
// 'poor' — the client never hardcodes cutoffs, it colours whatever rating the API returned.
function vitalTone(rating: string | undefined): Accent {
  if (!rating) return 'neutral'
  if (rating.startsWith('good')) return 'success'
  if (rating.startsWith('poor')) return 'error'
  return 'warning'
}
function monitorTone(m: any): Accent {
  if (m.last_state === 'up') return 'success'
  if (m.last_state === 'down') return 'error'
  return 'neutral'
}

const VITAL_LABELS: Record<string, string> = {
  'web_vitals.lcp': 'LCP',
  'web_vitals.inp': 'INP',
  'web_vitals.cls': 'CLS',
  'web_vitals.fcp': 'FCP',
  'web_vitals.ttfb': 'TTFB',
}
function vitalLabel(metric: string) {
  return VITAL_LABELS[metric] ?? metric
}
// CLS is unit-less; the rest are milliseconds.
function vitalValue(v: any) {
  return v.metric === 'web_vitals.cls' ? String(v.p75) : fmtMs(v.p75)
}

// Where each RUM/uptime tile drills. Backend tiles go to the services list.
const rumDest = computed<string>(() => (firstApp.value ? '/rum/' + encodeURIComponent(firstApp.value) : '/rum'))

interface Kpi {
  label: string
  value: string
  accent: Accent
  to: string
}
const kpis = computed<Kpi[]>(() => [
  { label: 'Request rate', value: fmtRate(requestRate.value), accent: 'info', to: '/services' },
  {
    label: 'Error rate',
    value: (errorRate.value * 100).toFixed(2) + '%',
    accent: errorRate.value >= 0.05 ? 'error' : errorRate.value > 0 ? 'warning' : 'success',
    to: '/services',
  },
  { label: 'p99 latency', value: formatDuration(worstP99.value), accent: 'neutral', to: '/services' },
  {
    label: 'LCP · p75',
    value: lcpVital.value ? fmtMs(lcpVital.value.p75) : '—',
    accent: vitalTone(lcpVital.value?.rating),
    to: rumDest.value,
  },
  {
    label: 'Monitors up',
    value: `${monitorsUp.value}/${monitors.value.length}`,
    accent: !monitors.value.length ? 'neutral' : monitorsUp.value === monitors.value.length ? 'success' : 'error',
    to: '/uptime',
  },
  { label: 'Services', value: formatNumber(rows.value.length), accent: 'neutral', to: '/services' },
])

// --- Chart series (all derived from the one headline-service timeseries) ---
// `ts` is an epoch-ms Number; percentiles are decimal-ns strings → ms via Number(p)/1e6.
const trafficSeries = computed(() => {
  if (!buckets.value.length) return []
  return [
    { key: 'rate', label: 'req/s', points: buckets.value.map((b) => ({ t: b.ts, v: b.rate ?? 0 })) },
    {
      key: 'errors',
      label: 'errors/s',
      points: buckets.value.map((b) => ({ t: b.ts, v: (b.rate ?? 0) * (b.error_rate ?? 0) })),
    },
  ]
})
const latencySeries = computed(() => {
  if (!buckets.value.length) return []
  return ['p50', 'p99'].map((k) => ({
    key: k,
    label: k,
    points: buckets.value.map((b) => ({ t: b.ts, v: b[k] != null ? Number(b[k]) / 1e6 : null })),
  }))
})
const rateSpark = computed(() => buckets.value.map((b) => b.rate ?? 0))
const p99Spark = computed(() => buckets.value.map((b) => Number(b.p99 ?? 0) / 1e6))

// --- drilldowns (every hop carries the time window + scope via correlate) ---
function go(path: string) {
  router.push(correlate({ path }))
}
function openService(service: string) {
  if (service) go('/services/' + encodeURIComponent(service))
}
function onOpenExemplars({ service }: { service: string }) {
  openService(service)
}
</script>

<template>
  <AppShell active="home" :mock="api.mock" crumb="Home · Overview">
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" data-testid="home">
      <!-- Cross-world KPI strip -->
      <section class="grid grid-cols-2 gap-3 md:grid-cols-6">
        <StatTile
          v-for="k in kpis"
          :key="k.label"
          :label="k.label"
          :value="k.value"
          :accent="k.accent"
          class="cursor-pointer"
          role="button"
          tabindex="0"
          @click="go(k.to)"
          @keydown.enter="go(k.to)"
        />
      </section>

      <!-- Headline trend charts -->
      <section class="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <ChartPanel title="Traffic &amp; errors" subtitle="requests/s · busiest service" class="lg:col-span-2">
          <template #summary>
            <div class="flex items-center gap-2">
              <span class="font-mono text-xs tabular-nums text-foreground">{{ fmtRate(requestRate) }}</span>
              <Sparkline :points="rateSpark" class="text-primary" />
            </div>
          </template>
          <LineChart :series="trafficSeries" :start-ms="startMs" :end-ms="endMs" :loading="chartsLoading" area />
        </ChartPanel>
        <ChartPanel title="Latency" subtitle="p50 · p99">
          <template #summary>
            <div class="flex items-center gap-2">
              <span class="font-mono text-xs tabular-nums text-foreground">{{ formatDuration(worstP99) }}</span>
              <Sparkline :points="p99Spark" class="text-primary" />
            </div>
          </template>
          <LineChart
            :series="latencySeries"
            :start-ms="startMs"
            :end-ms="endMs"
            :format-value="fmtMsChart"
            :loading="chartsLoading"
          />
        </ChartPanel>
      </section>

      <!-- Three worlds -->
      <section class="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <!-- Frontend · RUM Web Vitals -->
        <div class="flex flex-col rounded-lg border border-border bg-surface-1 p-3">
          <header class="mb-2 flex items-center justify-between gap-2">
            <h3 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">Frontend · Web Vitals</h3>
            <button
              type="button"
              class="text-[11px] text-primary hover:underline"
              @click="go(rumDest)"
            >
              {{ firstApp || 'RUM' }}
            </button>
          </header>
          <button
            v-for="v in vitals"
            :key="v.metric"
            type="button"
            class="flex items-center gap-2 rounded px-1 py-1 font-mono text-xs hover:bg-muted"
            @click="go(rumDest)"
          >
            <StatusDot :tone="vitalTone(v.rating)" />
            <span class="text-foreground">{{ vitalLabel(v.metric) }}</span>
            <span class="ml-auto tabular-nums text-muted-foreground">{{ vitalValue(v) }}</span>
          </button>
          <EmptyState
            v-if="!vitals.length"
            title="No RUM data"
            description="Instrument an app with the @photon/rum SDK."
            class="h-auto flex-1 py-6"
          />
        </div>

        <!-- Backend · Services (RED) -->
        <div class="flex flex-col rounded-lg border border-border bg-surface-1 p-3">
          <header class="mb-2 flex items-center justify-between gap-2">
            <h3 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">Backend · Services</h3>
            <button type="button" class="text-[11px] text-primary hover:underline" @click="go('/services')">
              {{ formatNumber(rows.length) }} services
            </button>
          </header>
          <RedTable :rows="rows" group="service" :loading="servicesLoading" @open-exemplars="onOpenExemplars" />
        </div>

        <!-- Ops · Uptime -->
        <div class="flex flex-col rounded-lg border border-border bg-surface-1 p-3">
          <header class="mb-2 flex items-center justify-between gap-2">
            <h3 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">Ops · Uptime</h3>
            <button type="button" class="text-[11px] text-primary hover:underline" @click="go('/uptime')">
              {{ monitorsUp }}/{{ monitors.length }} up
            </button>
          </header>
          <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
            <button
              v-for="m in monitors"
              :key="m.id"
              type="button"
              class="flex items-center gap-2 rounded border border-border/60 px-2 py-1.5 text-xs hover:bg-muted"
              @click="go('/uptime')"
            >
              <StatusDot :tone="monitorTone(m)" />
              <span class="truncate text-foreground">{{ m.name }}</span>
              <span v-if="m.last_latency_ms != null" class="ml-auto tabular-nums text-muted-foreground">
                {{ m.last_latency_ms }}ms
              </span>
            </button>
          </div>
          <EmptyState
            v-if="!monitors.length"
            title="No monitors"
            description="Add an uptime check in Ops."
            class="h-auto flex-1 py-6"
          />
        </div>
      </section>
    </main>
  </AppShell>
</template>

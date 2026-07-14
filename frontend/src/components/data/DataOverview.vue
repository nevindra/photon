<script setup>
// The Overview tab of the /data page: a KPI tile row (on-disk / durable / rows / current ingest
// rate) + a storage-composition bar + two usage-over-time charts (storage footprint by signal, and
// ingestion rate by signal). Storage totals come from `useStorage` (the reshaped `{ signals,
// durable }` payload); the time series comes from `useUsageSeries(usageWindow)`, whose window is
// derived from the ONE global time control (the ContextBar) — there is no page-local window
// selector. The durable tile is hidden entirely when no durable replica is configured. Chart series
// are built the same way ServiceDetailView builds its MetricChart series — `{ key, points:[{t,v}] }`.
import { computed } from 'vue'
import ChartPanel from '@/components/charts/ChartPanel.vue'
import { Spinner } from '@/components/ui/spinner'
import UsageChart from '@/components/data/UsageChart.vue'
import { StatTile } from '@/components/ui/stat-tile'
import { useStorage, useUsageSeries, usageWindow } from '@/lib/data/dataQueries'
import { formatBytes, formatNumber } from '@/lib/core/format'
import { signalColor } from '@/lib/core/signalMeta'

const { data: storage } = useStorage()
// Window follows the global ContextBar range (context.windowMs → usageWindow) — no local selector.
const { data: usage, isLoading } = useUsageSeries(usageWindow)

const SIGNALS = ['logs', 'traces', 'metrics']

const durableConfigured = computed(() => storage.value?.durable?.configured ?? false)
// Bucket width in seconds — used to convert the per-bucket ingest counts into a rows/sec rate.
const bucketSeconds = computed(() => (usage.value?.bucket_ms ?? 300_000) / 1000)

// The plotted window spans the first→last bucket of the (equally-sampled) logs series; all three
// signals share the same bucket grid, so any signal would do.
const startMs = computed(() => usage.value?.series?.logs?.[0]?.ts ?? 0)
const endMs = computed(() => {
  const a = usage.value?.series?.logs
  return a?.length ? a[a.length - 1].ts : 1
})

const sumSignals = (field) =>
  SIGNALS.reduce((acc, k) => acc + (storage.value?.signals?.[k]?.[field] ?? 0), 0)
const totalHot = computed(() => sumSignals('bytes'))
const totalDurable = computed(() => sumSignals('durable_bytes'))
const totalRows = computed(() => sumSignals('total_rows'))

// Storage composition: each signal's share of on-disk bytes, for the stacked bar + legend. Signals
// with no bytes yet are dropped so the bar/legend don't show a zero-width sliver.
const compositionSignals = computed(() => {
  const total = totalHot.value
  return SIGNALS.map((key) => {
    const bytes = storage.value?.signals?.[key]?.bytes ?? 0
    return { key, bytes, pct: total > 0 ? (bytes / total) * 100 : 0 }
  }).filter((s) => s.bytes > 0)
})

// Current ingest rate ≈ the most recent non-null per-bucket delta of each signal, divided by the
// bucket width, summed across signals. A null trailing bucket (counter reset / no previous) is
// skipped so the tile reflects the last real sample rather than reading 0.
const ingestRate = computed(() => {
  let sum = 0
  for (const k of SIGNALS) {
    const arr = usage.value?.series?.[k] ?? []
    for (let i = arr.length - 1; i >= 0; i--) {
      if (arr[i].ingest_rows != null) {
        sum += arr[i].ingest_rows / bucketSeconds.value
        break
      }
    }
  }
  return sum
})

const footprintSeries = computed(() =>
  SIGNALS.map((k) => ({
    key: k,
    points: (usage.value?.series?.[k] ?? []).map((p) => ({ t: p.ts, v: p.hot_bytes })),
  })),
)
const ingestionSeries = computed(() =>
  SIGNALS.map((k) => ({
    key: k,
    points: (usage.value?.series?.[k] ?? []).map((p) => ({
      t: p.ts,
      v: p.ingest_rows == null ? null : p.ingest_rows / bucketSeconds.value,
    })),
  })),
)
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
      <StatTile
        data-testid="tile-hot"
        label="On disk"
        :value="formatBytes(totalHot)"
        accent="info"
      />
      <StatTile
        v-if="durableConfigured"
        data-testid="tile-durable"
        label="Durable"
        :value="formatBytes(totalDurable)"
        accent="success"
      />
      <StatTile label="Rows" :value="formatNumber(totalRows)" accent="neutral" />
      <StatTile
        label="Ingest"
        :value="`${formatNumber(Math.round(ingestRate))}/s`"
        accent="neutral"
      />
    </div>

    <ChartPanel title="Storage composition" subtitle="On-disk share by signal">
      <template v-if="durableConfigured" #summary>
        <p class="text-[11px] text-muted-foreground">
          {{ formatNumber(storage?.durable?.pending ?? 0) }} pending replication
        </p>
      </template>

      <div data-testid="storage-composition">
        <div class="flex h-3 w-full overflow-hidden rounded-full bg-muted">
          <div
            v-for="s in compositionSignals"
            :key="s.key"
            class="h-full transition-[width] duration-300 ease-out"
            :style="{ background: signalColor(s.key), width: s.pct + '%' }"
          />
        </div>

        <ul v-if="compositionSignals.length" class="mt-3 flex flex-wrap gap-x-5 gap-y-1.5">
          <li v-for="s in compositionSignals" :key="s.key" class="flex items-center gap-1.5 text-xs">
            <span class="size-2 shrink-0 rounded-full" :style="{ background: signalColor(s.key) }" />
            <span class="font-medium capitalize text-card-foreground">{{ s.key }}</span>
            <span class="text-muted-foreground">{{ formatBytes(s.bytes) }} · {{ Math.round(s.pct) }}%</span>
          </li>
        </ul>
        <p v-else class="mt-3 text-xs text-muted-foreground">No data yet.</p>
      </div>
    </ChartPanel>

    <ChartPanel title="Storage footprint" subtitle="On-disk bytes by signal, over time">
      <Spinner v-if="isLoading" size="sm">Loading…</Spinner>
      <UsageChart
        v-else
        :series="footprintSeries"
        :start-ms="startMs"
        :end-ms="endMs"
        :format-value="formatBytes"
        area
        stacked
      />
    </ChartPanel>

    <ChartPanel title="Ingestion rate" subtitle="Rows per second by signal, over time">
      <Spinner v-if="isLoading" size="sm">Loading…</Spinner>
      <UsageChart
        v-else
        :series="ingestionSeries"
        :start-ms="startMs"
        :end-ms="endMs"
        :format-value="formatNumber"
      />
    </ChartPanel>
  </div>
</template>

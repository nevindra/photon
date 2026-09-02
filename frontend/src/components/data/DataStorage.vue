<script setup>
// The Storage tab of the /data page: a durable-replication status band above one rich card per
// signal present in storage. Reads the reshaped `{ signals, durable }` payload from `useStorage()`
// (cards iterate `storage.signals`), and pulls the shared usage series from `useUsageSeries(
// usageWindow)` — same key/window as the Overview tab, so it's cached/deduped, and it follows the
// ONE global time control (the ContextBar) — to draw each Parquet signal's on-disk footprint trend
// via the charts-kit `MiniAreaChart` (a real uPlot mini chart, not a hand-rolled SVG). Parquet
// signals (logs/traces/metrics) show size, rows/files, the footprint trend, a signal-hued "share of
// disk" bar and a "durable replication" meter. Uptime (only when the `[uptime]` store exists) keeps
// its distinct shape: heartbeat / monitor / incident counts, no bytes and no trend.
//
// It also carries the ONE place merge health is visible in the product. `merge_once` failing is
// silent by nature — ingest still acks and search still answers, the file count just climbs
// forever and every query gets slower. A stalled signal gets a banner at the top and a pill on its
// own card; the judgment itself lives in `lib/data/compaction.ts` so it is testable without a DOM.
import { computed } from 'vue'
import { Clock, AlertTriangle } from 'lucide-vue-next'
import { Alert, AlertTitle, AlertDescription } from '@/components/ui/alert'
import { Card } from '@/components/ui/card'
import { Meter } from '@/components/ui/meter'
import { StatusPill } from '@/components/ui/status-pill'
import MiniAreaChart from '@/components/charts/MiniAreaChart.vue'
import { Spinner } from '@/components/ui/spinner'
import { signalColor, signalIcon } from '@/lib/core/signalMeta'
import { formatBytes, formatNumber } from '@/lib/core/format'
import { judgeAll, isDegraded } from '@/lib/data/compaction'
import { useStorage, useUsageSeries, usageWindow } from '@/lib/data/dataQueries'

const { data: storage, isLoading: storageLoading } = useStorage()
// Same window as the Overview tab (both derive it from the global ContextBar), so the query key
// matches and TanStack Query dedupes/shares the fetch.
const { data: usage } = useUsageSeries(usageWindow)

// One card per signal. Uptime has a different shape than the Parquet signals; flag it so the
// template can branch.
const overviewCards = computed(() => {
  const s = storage.value?.signals
  if (!s) return []
  return Object.entries(s).map(([key, stat]) => ({ key, stat, uptime: 'monitor_count' in stat }))
})

// The on-disk footprint series for a Parquet signal, as { t, v } points for MiniAreaChart — real
// timestamps so the hover tooltip shows the bucket time. Only logs/traces/metrics have a usage
// series; uptime has none (returns [] → the chart's empty state).
function footprintPoints(key) {
  return (usage.value?.series?.[key] ?? []).map((p) => ({ t: p.ts, v: p.hot_bytes }))
}

// Total on-disk bytes across the Parquet signals only (uptime lives in SQLite, no `bytes`) — the
// denominator for each card's "share of disk".
const totalHot = computed(() => {
  const s = storage.value?.signals
  if (!s) return 0
  return Object.values(s).reduce(
    (acc, stat) => acc + ('monitor_count' in stat ? 0 : (stat?.bytes ?? 0)),
    0,
  )
})

// This signal's percentage of total on-disk bytes (0 when nothing on disk yet).
function sharePct(stat) {
  const total = totalHot.value
  if (!total || !stat?.bytes) return 0
  return Math.round((stat.bytes / total) * 100)
}

// Fraction of this signal's bytes that have been replicated to the durable store (0-1, for Meter).
function durableRatio(stat) {
  if (!stat?.bytes) return 0
  return (stat.durable_bytes ?? 0) / stat.bytes
}
function durablePct(stat) {
  return Math.round(durableRatio(stat) * 100)
}

// ns → date (Parquet min/max_ts_nanos); ms → date (uptime oldest/newest_heartbeat_ts). Number()
// loses sub-millisecond precision on 19-digit ns values but day-level rendering is unaffected.
function dateFromNanos(nanos) {
  return fmtDate(Number(nanos) / 1e6)
}
function fmtDate(ms) {
  if (ms == null || Number.isNaN(ms)) return '—'
  return new Date(ms).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })
}

// The oldest→newest span for a card, or "No data" when the signal is empty (so we never render the
// epoch "Jan 1, 1970" for a purged/fresh store).
function timeSpan(card) {
  const stat = card.stat ?? {}
  if (card.uptime) {
    if (stat.oldest_heartbeat_ts == null || stat.newest_heartbeat_ts == null) return 'No data'
    return `${fmtDate(Number(stat.oldest_heartbeat_ts))} → ${fmtDate(Number(stat.newest_heartbeat_ts))}`
  }
  if (!stat.file_count) return 'No data'
  return `${dateFromNanos(stat.min_ts_nanos)} → ${dateFromNanos(stat.max_ts_nanos)}`
}

const durable = computed(() => storage.value?.durable)

// Merge health, keyed by signal so a card can look up its own verdict. Empty on a node with no
// compactor attached — the cards then simply show no pill.
const compaction = computed(() => judgeAll(storage.value?.compaction))
const compactionBySignal = computed(() => Object.fromEntries(compaction.value.map((v) => [v.signal, v])))
// Only the signals worth interrupting the page for. The banner leads because a stall is invisible
// otherwise: everything else on this page keeps looking normal while it happens.
const stalled = computed(() => compaction.value.filter(isDegraded))
</script>

<template>
  <Spinner v-if="storageLoading" size="sm">Loading…</Spinner>
  <div v-else class="flex flex-col gap-3">
    <!-- Merge stall banner. Above the durable band on purpose: replication lagging is a copy that
         has not landed yet, a stalled merge is a tier quietly getting slower every day. -->
    <Alert v-if="stalled.length" :variant="stalled.some((v) => v.tone === 'error') ? 'error' : 'warning'">
      <AlertTriangle />
      <AlertTitle>
        Compaction {{ stalled.length === 1 ? 'is' : 'are' }} not merging
        ({{ stalled.map((v) => v.signal).join(', ') }})
      </AlertTitle>
      <AlertDescription class="flex flex-col gap-0.5">
        <span v-for="v in stalled" :key="v.signal" class="tabular-nums">
          <span class="capitalize">{{ v.signal }}</span>: {{ v.detail
          }}<template v-if="v.lastError"> — <span class="font-mono text-xs">{{ v.lastError }}</span></template>
        </span>
        <span class="opacity-80">
          Small files keep accumulating until this clears, and every query over them gets slower.
        </span>
      </AlertDescription>
    </Alert>

    <!-- Durable replication band -->
    <div
      v-if="durable?.configured"
      class="flex flex-wrap items-center gap-x-4 gap-y-1.5 rounded-xl border border-border bg-card px-4 py-3 shadow-1"
    >
      <span class="relative flex size-2.5 shrink-0 items-center justify-center">
        <span class="absolute inline-flex size-full animate-ping rounded-full bg-primary/60" />
        <span class="relative inline-flex size-1.5 rounded-full bg-primary" />
      </span>
      <span class="text-sm font-medium text-card-foreground">Durable replication</span>
      <span class="text-xs tabular-nums text-muted-foreground">
        {{ formatNumber(durable.pending ?? 0) }} pending
      </span>
      <span v-if="durable.last_replicated_ms" class="text-xs text-muted-foreground">
        last {{ new Date(durable.last_replicated_ms).toLocaleString() }}
      </span>
    </div>
    <div
      v-else
      class="flex items-center gap-2 rounded-xl border border-border bg-card px-4 py-3 text-xs text-muted-foreground shadow-1"
    >
      <span class="size-1.5 shrink-0 rounded-full bg-muted-foreground/50" />
      Durable storage not configured — hot tier only.
    </div>

    <!-- Per-signal storage cards -->
    <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
      <Card v-for="card in overviewCards" :key="card.key" class="flex flex-col gap-3 rounded-xl p-4">
        <!-- Shared header: tinted signal chip + name + a right-aligned share/store pill -->
        <div class="flex items-center gap-2.5">
          <span
            class="flex size-8 shrink-0 items-center justify-center rounded-lg"
            :style="{ background: signalColor(card.key) + '22' }"
          >
            <component :is="signalIcon(card.key)" class="size-4" :style="{ color: signalColor(card.key) }" />
          </span>
          <span class="text-sm font-medium capitalize text-card-foreground">{{ card.key }}</span>
          <StatusPill
            v-if="!card.uptime && compactionBySignal[card.key]"
            :tone="compactionBySignal[card.key].tone"
            :title="compactionBySignal[card.key].detail"
          >
            {{ compactionBySignal[card.key].label }}
          </StatusPill>
          <span
            v-if="card.uptime"
            class="ml-auto rounded-full border border-border bg-muted/60 px-2 py-0.5 text-[11px] font-medium text-muted-foreground"
          >
            SQLite
          </span>
          <span
            v-else
            class="ml-auto rounded-full border border-border bg-muted/60 px-2 py-0.5 text-[11px] font-medium tabular-nums text-muted-foreground"
          >
            {{ sharePct(card.stat) }}% of disk
          </span>
        </div>

        <!-- Uptime body: SQLite counts, no bytes / no sparkline -->
        <template v-if="card.uptime">
          <div>
            <p class="text-2xl font-semibold tabular-nums text-card-foreground">
              {{ formatNumber(card.stat?.heartbeat_count ?? 0) }}
            </p>
            <p class="text-xs text-muted-foreground">heartbeats</p>
          </div>
          <dl class="flex flex-col gap-1.5 text-xs">
            <div class="flex items-center justify-between">
              <dt class="text-muted-foreground">Monitors</dt>
              <dd class="tabular-nums text-card-foreground">{{ formatNumber(card.stat?.monitor_count ?? 0) }}</dd>
            </div>
            <div class="flex items-center justify-between">
              <dt class="text-muted-foreground">Incidents</dt>
              <dd class="tabular-nums text-card-foreground">{{ formatNumber(card.stat?.incident_count ?? 0) }}</dd>
            </div>
          </dl>
        </template>

        <!-- Parquet body: size figure, footprint sparkline, share + durable bars -->
        <template v-else>
          <div>
            <p class="text-2xl font-semibold tabular-nums text-card-foreground">
              {{ formatBytes(card.stat?.bytes) }}
            </p>
            <p class="text-xs tabular-nums text-muted-foreground">
              {{ formatNumber(card.stat?.total_rows ?? 0) }} rows ·
              {{ formatNumber(card.stat?.file_count ?? 0) }} files
            </p>
          </div>

          <MiniAreaChart
            :points="footprintPoints(card.key)"
            :color="signalColor(card.key)"
            :label="card.key"
            :format-value="formatBytes"
            :height="44"
          />

          <!-- Share of disk: signal-hued inline bar (Meter can't carry an arbitrary hue) -->
          <div class="flex flex-col gap-1">
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">Share of disk</span>
              <span class="tabular-nums text-muted-foreground">{{ sharePct(card.stat) }}%</span>
            </div>
            <div class="h-1.5 overflow-hidden rounded-full bg-muted">
              <div
                class="h-full rounded-full"
                :style="{ width: sharePct(card.stat) + '%', background: signalColor(card.key) }"
              />
            </div>
          </div>

          <!-- Durable replication: success-toned Meter -->
          <div class="flex flex-col gap-1">
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">Durable replication</span>
              <span v-if="card.stat?.durable_bytes" class="tabular-nums text-muted-foreground">
                {{ formatBytes(card.stat.durable_bytes) }} · {{ durablePct(card.stat) }}%
              </span>
              <span v-else class="text-muted-foreground/70">not replicated</span>
            </div>
            <Meter :value="durableRatio(card.stat)" :tone="card.stat?.durable_bytes ? 'success' : 'neutral'" />
          </div>
        </template>

        <!-- Shared footer: oldest→newest span, plus this signal's merge line when there is one -->
        <div
          class="mt-auto flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border pt-2.5 text-[11px] text-muted-foreground"
        >
          <span class="flex items-center gap-1.5">
            <Clock class="size-3 shrink-0" />
            <span class="tabular-nums">{{ timeSpan(card) }}</span>
          </span>
          <span
            v-if="!card.uptime && compactionBySignal[card.key]"
            class="tabular-nums"
            :class="isDegraded(compactionBySignal[card.key]) ? 'text-sev-warn' : ''"
          >
            {{ compactionBySignal[card.key].detail }}
          </span>
        </div>
      </Card>
    </div>
  </div>
</template>

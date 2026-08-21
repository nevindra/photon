<script setup lang="ts">
// The Services tab of `/data`: who is filling the disk.
//
// Sorted by estimated footprint, never by row count. Those give different answers and the footprint
// one is the answer to the question this page asks — a chatty service writing 40,000 tiny log lines
// can cost less disk than a quieter one writing 9,000 spans, and ordering by rows would put the
// cheap service at the top of a cost page.
//
// Two things this component must never soften:
//
//   1. **The bytes are divided, not measured.** Parquet is columnar and sorted by
//      `(service.name, timestamp)`, so a service's rows compress together and there is no
//      per-service byte figure to read. What the API does — and what this says out loud — is
//      `service rows × the signal's mean bytes/row`.
//   2. **Metrics are missing on purpose, and their bytes belong to somebody.** Host metrics belong
//      to a host, RUM vitals to a browser app, the rollup's output to the rollup. Leaving that
//      unsaid invites the reader to conclude the difference is unaccounted-for waste.
import { computed } from 'vue'
import { Info } from 'lucide-vue-next'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { Meter } from '@/components/ui/meter'
import { formatBytes, formatCompact } from '@/lib/core/format'
import { startNs, endNs } from '@/lib/core/context'
import { useServiceUsage } from '@/lib/data/dataQueries'

const usageQuery = useServiceUsage(startNs, endNs)
const rows = computed(() => usageQuery.data.value?.rows ?? [])
const signals = computed(() => usageQuery.data.value?.signals)
const notes = computed(() => usageQuery.data.value?.notes)
const loading = computed(() => usageQuery.isFetching.value)

/** The largest row's footprint, so each row's share bar is relative to the top talker. */
const peak = computed(() => rows.value.reduce((m, r) => Math.max(m, r.bytes_total), 0))

const attributedBytes = computed(() => rows.value.reduce((n, r) => n + r.bytes_total, 0))

/**
 * The store-wide bytes this page does NOT attribute — metrics, plus whatever fell outside the
 * window or below the row cap. Shown as its own line rather than folded into a service, because
 * every byte of it belongs to something real.
 */
const unattributed = computed(() => {
  const s = signals.value
  if (!s) return 0
  const store = s.logs.store_bytes + s.traces.store_bytes + s.metrics.store_bytes
  return Math.max(0, store - attributedBytes.value)
})

const share = (bytes: number) => (peak.value > 0 ? (bytes / peak.value) * 100 : 0)
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex flex-wrap items-center gap-2.5 text-xs text-muted-foreground">
      <span class="font-mono tabular-nums text-foreground/80">
        {{ formatBytes(attributedBytes) }} attributed
      </span>
      <span class="text-border">·</span>
      <span class="font-mono tabular-nums">{{ rows.length }} services</span>
      <span v-if="unattributed > 0" class="text-border">·</span>
      <span v-if="unattributed > 0" data-testid="usage-unattributed" class="font-mono tabular-nums">
        {{ formatBytes(unattributed) }} not attributed to a service
      </span>
      <Spinner v-if="loading" size="sm">loading…</Spinner>
    </div>

    <div class="overflow-hidden rounded-xl border border-border bg-card">
      <Table v-if="rows.length" container-class="overflow-visible" class="border-collapse font-mono text-xs">
        <TableHeader>
          <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
            <TableHead>Service</TableHead>
            <TableHead class="text-right">Log rows</TableHead>
            <TableHead class="text-right">Span rows</TableHead>
            <TableHead class="text-right">Est. footprint</TableHead>
            <TableHead class="w-[22%]">Share</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="r in rows" :key="r.service" data-testid="usage-service-row" :data-service="r.service" class="border-border/60">
            <TableCell class="py-1.5 text-foreground">{{ r.service }}</TableCell>
            <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">
              {{ formatCompact(r.signals.logs.rows) }}
            </TableCell>
            <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">
              {{ formatCompact(r.signals.traces.rows) }}
            </TableCell>
            <TableCell class="py-1.5 text-right tabular-nums text-foreground/80">
              {{ formatBytes(r.bytes_total) }}
            </TableCell>
            <TableCell class="py-1.5">
              <Meter :value="share(r.bytes_total)" :max="100" class="h-1.5" />
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
      <EmptyState
        v-else
        title="Nothing written by a named service in this window"
        description="Widen the time range, or check that your producers set service.name."
        class="h-auto py-10"
      />
    </div>

    <!-- Both caveats, stated once, where the numbers are. -->
    <div class="flex flex-col gap-1.5 text-[11px] text-muted-foreground">
      <p data-testid="usage-estimate-note" class="flex items-start gap-1.5">
        <Info class="mt-px size-3 shrink-0" />
        <span>
          Footprint is an <strong class="font-medium text-foreground/80">estimate</strong>: a
          service's rows in this window times its signal's mean bytes per row
          <template v-if="signals">
            ({{ signals.logs.bytes_per_row.toFixed(1) }} B/log row,
            {{ signals.traces.bytes_per_row.toFixed(1) }} B/span row)</template
          >. Parquet sorts by <code class="font-mono">service.name</code> and compresses a service's
          rows together, so there is no measured per-service byte figure to read.
        </span>
      </p>
      <p v-if="notes" data-testid="usage-metrics-note" class="flex items-start gap-1.5">
        <Info class="mt-px size-3 shrink-0" />
        <span>{{ notes.metrics }}</span>
      </p>
    </div>
  </div>
</template>

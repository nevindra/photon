<script setup lang="ts">
// `/services/:service/operations` — where inside this service the traffic, errors and time go.
//
// Two modes, and the distinction is not cosmetic. **Entry points** are the operations this service
// SERVED (its server/consumer spans); **All operations** additionally lists the work it DID — the
// outbound database call, the internal stage. Listing them together without saying so is how
// `SELECT users` ends up looking like an endpoint serving 4,000 requests (ADR-0003).
//
// The footer states what Entry points is hiding, and why. A filter that silently drops rows is a
// filter you cannot trust.
import { computed, ref, type Ref } from 'vue'
import { useRouter } from 'vue-router'
import ServiceScaffold from '@/components/services/ServiceScaffold.vue'
import OperationsTable from '@/components/services/OperationsTable.vue'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Spinner } from '@/components/ui/spinner'
import { useServiceOperations } from '@/lib/services/servicesQueries'
import { useServiceParam } from '@/lib/services/useServiceParam'
import { OTHER_OPERATION } from '@/lib/services/servicesOverview'
import { startNs, endNs, windowMs, timeRange, customRange } from '@/lib/core/context'
import { useUrlParam } from '@/lib/core/useUrlState'
import { formatNumber } from '@/lib/core/format'
import { correlate } from '@/lib/core/useCorrelate'
import { grammarTerm } from '@/lib/core/queryLang'
import type { RedRow } from '@/lib/core/api'

const router = useRouter()
const service = useServiceParam()

// `?mode=` so a shared link keeps the reader on the same footing. Entry points is the default and
// stays out of the URL.
const mode = ref<'entry' | 'all'>('entry')
useUrlParam('mode', mode as Ref<string | null>, { valid: (v) => v === 'entry' || v === 'all', fallback: 'entry' })

const ops = useServiceOperations(service, startNs, endNs)
const all = computed<RedRow[]>(() => ops.data.value ?? [])
// An operation with no server/consumer spans of its own is not something the service served. The
// rollup's `__other__` remainder always stays: it is a sum over both kinds and dropping it would
// make the visible rows stop adding up to the service.
const entryPoints = computed(() => all.value.filter((r) => (r.basis ?? 'entry') === 'entry' || r.operation === OTHER_OPERATION))
const rows = computed(() => (mode.value === 'entry' ? entryPoints.value : all.value))
const hidden = computed(() => all.value.length - entryPoints.value.length)

const loading = computed(() => ops.isFetching.value)
const windowSecs = computed(() => windowMs.value / 1000)

function openOperation(row: RedRow): void {
  const op = row.operation
  if (!op) return
  router.push({
    path: `/services/${encodeURIComponent(service.value)}/operations/${encodeURIComponent(op)}`,
    query: router.currentRoute.value.query,
  })
}
// Operation names carrying whitespace or a comma need quoting (the grammar splits terms on
// whitespace and OR-lists on commas); `grammarTerm` does that, and drops a term the grammar cannot
// express at all rather than emitting one that matches nothing.
function openExemplars(row: RedRow): void {
  const q = [grammarTerm('service.name', service.value), grammarTerm('name', row.operation)]
    .filter(Boolean)
    .join(' ')
  router.push(correlate({ path: '/traces', query: { q, sort: 'slowest' } }))
}
</script>

<template>
  <ServiceScaffold :service="service" crumb-suffix="Operations">
    <main class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-5">
      <div class="flex flex-wrap items-center gap-3">
        <Segmented :model-value="mode" class="text-xs" @update:model-value="mode = ($event as 'entry' | 'all') || 'entry'">
          <SegmentedItem value="entry">Entry points</SegmentedItem>
          <SegmentedItem value="all">All operations</SegmentedItem>
        </Segmented>
        <span class="font-mono text-xs tabular-nums text-muted-foreground">
          {{ formatNumber(rows.length) }} operations
        </span>
        <span class="font-mono text-xs text-muted-foreground">
          {{ customRange ? 'custom range' : `last ${timeRange}` }}
        </span>
        <Spinner v-if="loading" size="sm">loading…</Spinner>
      </div>

      <OperationsTable
        :rows="rows"
        :loading="loading"
        :window-secs="windowSecs"
        @open="openOperation"
        @open-exemplars="openExemplars"
      />

      <p v-if="mode === 'entry' && hidden > 0" data-testid="operations-footer" class="text-[11px] text-muted-foreground">
        {{ hidden }} more {{ hidden === 1 ? 'operation is' : 'operations are' }} hidden. They have no server or
        consumer spans, so they are work this service did — outbound calls and internal stages — not
        requests it served. Switch to <strong class="font-medium text-foreground">All operations</strong> to see them.
      </p>
    </main>
  </ServiceScaffold>
</template>

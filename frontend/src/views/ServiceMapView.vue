<script setup lang="ts">
// `/services/map` — the shape of the fleet.
//
// The index answers "what is wrong"; this answers "what talks to what", which is the question you
// ask once you know something is wrong and need to know how far it reaches. Every node's dot is
// the same Attention Level the index ranks on, so a red node here and a red row there are the
// same judgment, not two.
//
// Below the graph sits the edge table, because a picture is for finding the edge and a table is
// for reading its numbers. Trying to put p99 on a curve is how you get a graph nobody can read.
import { computed } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import FleetGraph from '@/components/services/FleetGraph.vue'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { formatCompact, formatDuration, formatNumber } from '@/lib/core/format'
import { startNs, endNs, timeRange, customRange } from '@/lib/core/context'
import { useServiceMap, useServicesList } from '@/lib/services/servicesQueries'
import { serviceLevel, type AttentionLevel } from '@/lib/services/serviceHealth'
import { correlate } from '@/lib/core/useCorrelate'
import { grammarTerm } from '@/lib/core/queryLang'
import type { RedRow, ServiceMapEdge } from '@/lib/core/api'

const router = useRouter()
const route = useRoute()

const mapQuery = useServiceMap(null, startNs, endNs)
const nodes = computed(() => mapQuery.data.value?.nodes ?? [])
const edges = computed(() => mapQuery.data.value?.edges ?? [])
const loading = computed(() => mapQuery.isFetching.value)
const capped = computed(() => mapQuery.data.value?.capped === true)

// The graph colours its nodes with the app's ONE Attention Level model rather than deriving a
// second judgment from span counts. `/api/services/map` deliberately does not carry a level: the
// level depends on Apdex and on the previous window, neither of which is a topology fact.
const listQuery = useServicesList('', startNs, endNs)
const levels = computed<Record<string, AttentionLevel>>(() => {
  const out: Record<string, AttentionLevel> = {}
  for (const row of (listQuery.data.value ?? []) as RedRow[]) {
    // `RedRow` is a closed interface and `ServiceHealthRow` an open one; the spread is the cast.
    out[row.service] = serviceLevel({ ...row }).level
  }
  return out
})

function openService(service: string): void {
  router.push({ path: `/services/${encodeURIComponent(service)}`, query: route.query })
}

// An edge's traces: both services in one filter, error-sorted, so the first result is a failure.
function openEdgeTraces(e: ServiceMapEdge): void {
  const q = [grammarTerm('service.name', e.to), 'status:error'].filter(Boolean).join(' ')
  router.push(correlate({ path: '/traces', query: { q, sort: 'errors' } }))
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Backend › Map">
    <template #toolbar>
      <NavTabs class="shrink-0 text-xs" aria-label="Backend sections">
        <NavTabItem :to="{ path: '/services', query: route.query }" :active="false">Services</NavTabItem>
        <NavTabItem :to="{ path: '/services/errors', query: route.query }" :active="false">Issues</NavTabItem>
        <NavTabItem :to="{ path: '/services/map', query: route.query }" :active="true">Map</NavTabItem>
      </NavTabs>
    </template>

    <main class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-5">
      <div class="flex flex-wrap items-center gap-2.5 text-xs text-muted-foreground">
        <span class="font-mono tabular-nums text-foreground/80">{{ formatNumber(nodes.length) }} services</span>
        <span class="text-border">·</span>
        <span class="font-mono tabular-nums">{{ formatNumber(edges.length) }} edges</span>
        <span class="text-border">·</span>
        <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
        <Spinner v-if="loading" size="sm">loading…</Spinner>
      </div>

      <!-- The cap is stated, never hidden: a truncated map that looks complete is worse than none. -->
      <p v-if="capped" data-testid="map-capped" class="text-[11px] text-sev-warn">
        This window has more spans than the map reads in one pass, so some edges are missing or
        undercounted. Narrow the time range for a complete picture.
      </p>

      <FleetGraph :nodes="nodes" :edges="edges" :levels="levels" :loading="loading" @select="openService" />

      <section class="flex flex-col gap-2">
        <span class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Edges</span>
        <div class="overflow-hidden rounded-xl border border-border bg-card">
          <Table v-if="edges.length" container-class="overflow-visible" class="border-collapse font-mono text-xs">
            <TableHeader>
              <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
                <TableHead>From</TableHead>
                <TableHead>To</TableHead>
                <TableHead class="text-right">Calls</TableHead>
                <TableHead class="text-right">Calls/s</TableHead>
                <TableHead class="text-right">Errors</TableHead>
                <TableHead class="text-right">p50</TableHead>
                <TableHead class="text-right">p95</TableHead>
                <TableHead class="text-right">p99</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow
                v-for="e in edges"
                :key="e.from + '->' + e.to"
                data-testid="map-edge-row"
                role="button"
                tabindex="0"
                class="cursor-pointer border-border/60"
                @click="openEdgeTraces(e)"
                @keydown.enter.prevent="openEdgeTraces(e)"
              >
                <TableCell class="py-1.5 text-muted-foreground">{{ e.from }}</TableCell>
                <TableCell class="py-1.5 text-foreground">{{ e.to }}</TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatCompact(e.count) }}</TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ e.rate.toFixed(e.rate >= 10 ? 1 : 2) }}/s</TableCell>
                <TableCell
                  class="py-1.5 text-right tabular-nums"
                  :class="e.error_count > 0 ? 'text-sev-error' : 'text-muted-foreground'"
                >
                  {{ (e.error_rate * 100).toFixed(2) }}%
                </TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatDuration(Number(e.p50)) }}</TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatDuration(Number(e.p95)) }}</TableCell>
                <TableCell class="py-1.5 text-right tabular-nums text-foreground/80">{{ formatDuration(Number(e.p99)) }}</TableCell>
              </TableRow>
            </TableBody>
          </Table>
          <EmptyState
            v-else
            title="No calls between services in range"
            description="Either nothing called anything, or the traces did not span two services."
            class="h-auto py-8"
          />
        </div>
        <!-- Latency on an edge is the callee's own span duration, not the caller's wall clock.
             Saying so once beats letting someone conclude the network is free. -->
        <p class="text-[11px] text-muted-foreground">
          Edge latency is the callee's own span duration. The caller waits longer than this by
          however much the network and its own queueing add.
        </p>
      </section>
    </main>
  </AppShell>
</template>

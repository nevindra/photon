<script setup lang="ts">
// `/services/:service/dependencies` — what this service waits on, and who waits on it.
//
// Split off the Overview page because it answers a different question. Overview asks "is this
// service healthy"; this asks "is it healthy because of something it depends on". Its own route
// means its own queries, so Overview no longer waits on a dependency scan to paint.
//
// The picture on top is one hop in both directions, and it merges the two sources the backend
// keeps apart on purpose: service-to-service edges (`/api/services/map`, derived from cross-service
// parentage) and database/external calls (`/api/services/:service/dependencies`, derived from
// CLIENT-span attributes). To someone debugging, "what is this waiting on" has one answer. The
// tables below keep them separate, because once you know the answer the distinction matters again.
import { computed } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import ServiceScaffold from '@/components/services/ServiceScaffold.vue'
import DependencyTable from '@/components/services/DependencyTable.vue'
import FocusedDependencyMap, { type MapSideNode } from '@/components/services/FocusedDependencyMap.vue'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { useServiceDependencies, useServiceMap } from '@/lib/services/servicesQueries'
import { useServiceParam } from '@/lib/services/useServiceParam'
import { startNs, endNs } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'
import { grammarTerm } from '@/lib/core/queryLang'
import type { DepRow } from '@/lib/core/api'

const router = useRouter()
const route = useRoute()
const service = useServiceParam()

const deps = useServiceDependencies(service, startNs, endNs)
const databaseRows = computed(() => deps.data.value?.database ?? [])
const externalRows = computed(() => deps.data.value?.external ?? [])

const map = useServiceMap(service, startNs, endNs)
const callers = computed(() => (map.data.value?.edges ?? []).filter((e) => e.to === service.value))
const callees = computed(() => (map.data.value?.edges ?? []).filter((e) => e.from === service.value))

// The two dependency tables, normalized into the map's box shape.
function depNode(row: DepRow, kind: 'database' | 'external'): MapSideNode {
  return {
    key: `${kind}:${row.name}`,
    label: row.name,
    kind,
    count: row.count ?? 0,
    errorRate: row.error_rate ?? 0,
    p99: row.p99 ?? null,
  }
}
const dependencyNodes = computed<MapSideNode[]>(() => [
  ...databaseRows.value.map((r) => depNode(r, 'database')),
  ...externalRows.value.map((r) => depNode(r, 'external')),
])

const pending = computed(() => deps.isPending.value || map.isPending.value)
const empty = computed(
  () =>
    !pending.value &&
    !databaseRows.value.length &&
    !externalRows.value.length &&
    !callers.value.length &&
    !callees.value.length,
)

// Service-scoped for now; a dependency-scoped filter follows once a peer/db-system grammar field
// is confirmed.
function onOpenTraces(): void {
  const q = grammarTerm('service.name', service.value) ?? ''
  router.push(correlate({ path: '/traces', query: { q, sort: 'slowest' } }))
}

// Only service boxes are clickable — a database has no page of its own, which is the same reason
// `CONTEXT.md` refuses to call it a Service.
function onSelect(node: MapSideNode): void {
  if (node.kind !== 'service') return
  router.push({ path: `/services/${encodeURIComponent(node.label)}`, query: route.query })
}
</script>

<template>
  <ServiceScaffold :service="service" crumb-suffix="Dependencies">
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
      <div v-if="pending" class="flex items-center justify-center py-16">
        <Spinner size="lg">Loading dependencies…</Spinner>
      </div>
      <EmptyState
        v-else-if="empty"
        title="No calls in or out in range"
        description="Nothing called this service, and it made no client or producer spans, in the selected window."
      />
      <template v-else>
        <FocusedDependencyMap
          :service="service"
          :callers="callers"
          :callees="callees"
          :dependencies="dependencyNodes"
          @select="onSelect"
        />

        <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
          <DependencyTable title="Database calls" :rows="databaseRows" @open-traces="onOpenTraces" />
          <DependencyTable title="External calls" :rows="externalRows" @open-traces="onOpenTraces" />
        </div>
      </template>
    </main>
  </ServiceScaffold>
</template>

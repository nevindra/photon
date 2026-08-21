<script setup lang="ts">
// `/services/:service/errors/:fingerprint` — one Backend Issue.
//
// The Issue's identity is the PAIR in the URL: the same fingerprint under a different service is
// a different Issue, with a different owner and a different fix. Both params are therefore read
// and both are sent; neither is inferred from the other.
//
// Layout mirrors `RumErrorDetailView` — hero, occurrences over time, tag breakdowns, a sample
// stack, recent occurrences — because the two are the same page about two runtimes. What differs
// is what a sample row is: there, a browser session; here, a span you can open in a waterfall.
import { computed } from 'vue'
import { useRoute, RouterLink } from 'vue-router'
import ServiceScaffold from '@/components/services/ServiceScaffold.vue'
import BarChart from '@/components/charts/BarChart.vue'
import { Meter } from '@/components/ui/meter'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { formatCompact, formatDuration, formatNumber } from '@/lib/core/format'
import { startNs, endNs } from '@/lib/core/context'
import { useBackendIssueDetail } from '@/lib/services/servicesQueries'
import { useServiceParam, useRouteParam } from '@/lib/services/useServiceParam'
import { correlate } from '@/lib/core/useCorrelate'
import { grammarTerm } from '@/lib/core/queryLang'
import type { BackendIssueTag } from '@/lib/core/api'

const route = useRoute()
const service = useServiceParam()
const fingerprint = useRouteParam('fingerprint')

const detailQuery = useBackendIssueDetail(service, fingerprint, startNs, endNs)
const detail = computed(() => detailQuery.data.value ?? null)
const loading = computed(() => detailQuery.isLoading.value)
const empty = computed(() => !loading.value && !!detail.value && detail.value.occurrences === 0)

const issuesPath = computed(() => ({
  path: `/services/${encodeURIComponent(service.value)}/errors`,
  query: route.query,
}))

// `series` arrives as `[epoch ms, count]` pairs — already the chart's x domain, so no conversion.
const chartBuckets = computed(() =>
  (detail.value?.series ?? []).map(([t, count]) => ({
    t,
    segments: [{ key: 'count', label: 'Occurrences', color: 'hsl(var(--sev-error))', value: count }],
  })),
)
const startMs = computed(() => Number(startNs.value) / 1_000_000)
const endMs = computed(() => Number(endNs.value) / 1_000_000)

// Timestamps on this payload are nanosecond STRINGS. Parse through BigInt so a 19-digit value
// keeps its precision on the way to a Date.
function nsToDate(ns: string): Date {
  return new Date(Number(BigInt(ns) / 1_000_000n))
}

function tagTotal(tag: BackendIssueTag): number {
  return tag.values.reduce((sum, v) => sum + v.count, 0)
}
function shareFor(tag: BackendIssueTag, count: number): number {
  const total = tagTotal(tag)
  return total > 0 ? count / total : 0
}

// Logs around one occurrence: its own trace, which is the tightest honest filter — a span id is
// not a log field, and a service-wide log search would bury the one request that failed.
function logsHref(traceId: string): string {
  return correlate({ path: '/logs', query: { q: grammarTerm('trace_id', traceId) ?? '' } })
}
</script>

<template>
  <ServiceScaffold :service="service" crumb-suffix="Issue">
    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div v-if="loading && !detail" class="flex flex-1 items-center justify-center">
        <Spinner size="lg">Loading issue…</Spinner>
      </div>

      <EmptyState
        v-else-if="empty"
        title="No occurrences in range"
        description="This issue did not fire inside the selected window. Widen the time range to see it."
        class="h-auto flex-1"
      />

      <template v-else-if="detail">
        <!-- Hero: what threw, and how much -->
        <section class="px-5 pt-5">
          <div class="rounded-xl border border-border bg-card p-4">
            <div class="flex items-start justify-between gap-4">
              <div class="min-w-0">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="font-semibold text-sev-error">{{ detail.error_type }}</span>
                  <RouterLink
                    :to="issuesPath"
                    class="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground hover:text-foreground"
                  >
                    {{ detail.service }}
                  </RouterLink>
                  <span class="font-mono text-[10px] text-muted-foreground">{{ detail.fingerprint }}</span>
                </div>
                <p class="mt-1 font-mono text-sm text-foreground">{{ detail.message || '—' }}</p>
              </div>
              <RelatedMenu
                :entity="{
                  kind: 'backendIssue',
                  fields: { service: detail.service, traceId: detail.events[0]?.trace_id },
                }"
              />
            </div>
            <dl class="mt-4 grid grid-cols-2 gap-3 text-sm sm:grid-cols-5">
              <div>
                <dt class="text-xs text-muted-foreground">Events</dt>
                <dd class="tabular-nums text-foreground">{{ formatNumber(detail.occurrences) }}</dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground" title="Distinct traces — requests that went wrong">Traces</dt>
                <dd class="tabular-nums text-foreground">{{ formatNumber(detail.traces) }}</dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground">Operations</dt>
                <dd class="tabular-nums text-foreground">{{ formatNumber(detail.operations) }}</dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground">First seen</dt>
                <dd class="text-foreground">{{ nsToDate(detail.first_seen).toLocaleString() }}</dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground">Last seen</dt>
                <dd class="text-foreground">{{ nsToDate(detail.last_seen).toLocaleString() }}</dd>
              </div>
            </dl>
          </div>
        </section>

        <!-- Occurrences over time -->
        <section class="px-5 pt-6">
          <span class="block pb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">Occurrences</span>
          <div class="rounded-xl border border-border bg-card p-4">
            <BarChart :buckets="chartBuckets" :start-ms="startMs" :end-ms="endMs" :loading="loading" />
          </div>
        </section>

        <!-- Tag breakdowns: where it fires -->
        <section v-if="detail.tags.length" class="px-5 pt-6">
          <span class="block pb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">Breakdown</span>
          <div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div v-for="tag in detail.tags" :key="tag.field" class="rounded-xl border border-border bg-card p-4">
              <h3 class="mb-2 font-mono text-xs text-muted-foreground">{{ tag.field }}</h3>
              <ul v-if="tag.values.length" class="space-y-1.5">
                <li v-for="v in tag.values" :key="v.value" class="flex items-center gap-2 text-xs">
                  <span class="w-28 shrink-0 truncate font-mono text-foreground" :title="v.value">{{ v.value }}</span>
                  <Meter :value="shareFor(tag, v.count)" class="flex-1" />
                  <span class="w-10 shrink-0 text-right tabular-nums text-muted-foreground">{{ formatCompact(v.count) }}</span>
                </li>
              </ul>
              <p v-else class="text-xs text-muted-foreground">Not recorded on these spans.</p>
            </div>
          </div>
        </section>

        <!-- Sample stack -->
        <section class="px-5 pt-6">
          <span class="block pb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">Sample stack</span>
          <pre
            v-if="detail.sample_stack"
            data-testid="issue-stack"
            class="overflow-x-auto rounded-xl border border-border bg-card p-4 font-mono text-xs leading-relaxed text-foreground"
          >{{ detail.sample_stack }}</pre>
          <!-- Not a failure: a service that sets `status = ERROR` without recording an exception
               event has no stack to show, and saying so beats an empty box. -->
          <p v-else data-testid="issue-no-stack" class="text-xs text-muted-foreground">
            No stack trace. These spans were marked as errors without an <code class="font-mono">exception</code>
            event, so there is nothing recorded to show.
          </p>
        </section>

        <!-- Recent occurrences -->
        <section class="px-5 pb-5 pt-6">
          <span class="block pb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">Recent occurrences</span>
          <div class="overflow-hidden rounded-xl border border-border bg-card">
            <Table container-class="overflow-visible" class="border-collapse font-mono text-xs">
              <TableHeader>
                <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
                  <TableHead>Time</TableHead>
                  <TableHead>Operation</TableHead>
                  <TableHead class="text-right">Duration</TableHead>
                  <TableHead>Host</TableHead>
                  <TableHead>Env</TableHead>
                  <TableHead class="text-right"><span class="sr-only">Jump</span></TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow
                  v-for="ev in detail.events"
                  :key="ev.span_id"
                  data-testid="issue-event-row"
                  class="border-border/60"
                >
                  <TableCell class="py-1.5 text-muted-foreground">{{ nsToDate(ev.timestamp).toLocaleTimeString() }}</TableCell>
                  <TableCell class="py-1.5 text-foreground" :title="ev.status_message">{{ ev.operation }}</TableCell>
                  <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">
                    {{ ev.duration_nanos ? formatDuration(Number(ev.duration_nanos)) : '—' }}
                  </TableCell>
                  <TableCell class="py-1.5 text-muted-foreground">{{ ev.host || '—' }}</TableCell>
                  <TableCell class="py-1.5 text-muted-foreground">{{ ev.environment || '—' }}</TableCell>
                  <TableCell class="py-1.5 text-right font-sans">
                    <RouterLink :to="`/traces/${ev.trace_id}`" class="text-brand hover:underline">Open trace</RouterLink>
                    <a :href="logsHref(ev.trace_id)" class="ml-3 text-brand hover:underline">Open logs</a>
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
          </div>
        </section>
      </template>
    </main>
  </ServiceScaffold>
</template>

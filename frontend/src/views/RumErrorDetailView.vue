<script setup lang="ts">
// RUM error issue detail (`/rum/:appId/errors/:fingerprint`): one issue (grouped by exception
// fingerprint) — hero summary, an occurrences-over-time chart, per-tag breakdowns (browser/device/
// route/connection), a sample stack trace, and a table of recent sample events. Each event jumps to
// its trace waterfall (when a `trace_id` was captured) or to Logs filtered by its session, via
// `correlate()` so the active time window rides along. Mirrors RumPageDetailView's AppShell + `#lead`
// back-arrow shell and its app/route(-here fingerprint) param normalization.
import { computed } from 'vue'
import { useRoute, RouterLink } from 'vue-router'
import { ArrowLeft } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import BarChart from '@/components/charts/BarChart.vue'
import { Meter } from '@/components/ui/meter'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { api, type RumErrorTag } from '@/lib/core/api'
import { formatNumber } from '@/lib/core/format'
import { startNs, endNs } from '@/lib/core/context'
import { useRumErrorDetail } from '@/lib/rum/rumQueries'
import { correlate } from '@/lib/core/useCorrelate'

const route = useRoute()

// Vue Router decodes both params automatically; normalize the array form defensively (same
// pattern RumPageDetailView uses for its app/route params).
const app = computed<string>(() => {
  const a = route.params.appId
  return ((Array.isArray(a) ? a[0] : a) ?? '').trim()
})
const fingerprint = computed<string>(() => {
  const f = route.params.fingerprint
  return ((Array.isArray(f) ? f[0] : f) ?? '').trim()
})
const appBase = computed(() => '/rum/' + encodeURIComponent(app.value))
const errorsPath = computed(() => ({ path: `${appBase.value}/errors`, query: route.query }))
// Keep the app-level crumb (sub-page of the same RUM app; scope set by the vitals view).
const crumb = computed(() => 'Frontend › ' + app.value)

const detailQuery = useRumErrorDetail(app, fingerprint, startNs, endNs)
const detail = computed(() => detailQuery.data.value ?? null)
const loading = computed(() => detailQuery.isLoading.value)
const empty = computed(() => !loading.value && !!detail.value && detail.value.occurrences === 0)

// Occurrence series → BarChart buckets. `series[].t`, `first_seen`, `last_seen`, and
// `events[].timestamp` are all epoch NANOSECONDS from the API (a DataFusion `cast(timestamp,
// Int64)` / histogram bucket-start over the ns window bounds — see `photon-query`'s
// `rum_error_detail`/`histogram_over`), so each is divided by 1e6 for ms display — consistent
// with startMs/endMs below.
const chartBuckets = computed(() =>
  (detail.value?.series ?? []).map((b) => ({
    t: b.t / 1_000_000,
    segments: [{ key: 'count', label: 'Occurrences', color: 'hsl(var(--sev-error))', value: b.count }],
  })),
)
// startNs/endNs ARE decimal-nanosecond strings (the app-wide window bounds); BarChart wants ms.
const startMs = computed(() => Number(startNs.value) / 1_000_000)
const endMs = computed(() => Number(endNs.value) / 1_000_000)

// Each tag's value bars are self-relative shares of that tag's own total (mirrors
// FacetFieldGroup's per-field max-count normalization), rendered with the generic Meter (not
// VitalsDistributionBar — that component's good/needs/poor colouring is CWV-rating semantics and
// doesn't apply to a plain browser/device/route/connection breakdown).
function tagTotal(tag: RumErrorTag): number {
  return tag.values.reduce((sum, v) => sum + v.count, 0)
}
function shareFor(tag: RumErrorTag, count: number): number {
  const total = tagTotal(tag)
  return total > 0 ? count / total : 0
}

// "Open logs" for one event: filter Logs by this session, carrying the active time window + scope.
function logsHref(session: string): string {
  return correlate({ path: '/logs', query: { q: session ? `session.id:${session}` : '' } })
}
// Plain path (no correlate): a trace waterfall reads its own span times, not the outer window.
function traceHref(traceId?: string | null): string {
  return traceId ? `/traces/${traceId}` : ''
}
</script>

<template>
  <AppShell :mock="api.mock" :crumb="crumb">
    <!-- Back-to-issues button folds into the ContextBar's lead slot (no second bar). -->
    <template #lead>
      <RouterLink
        :to="errorsPath"
        data-testid="back-to-errors"
        aria-label="Back to issues"
        class="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
      >
        <ArrowLeft class="size-4" />
      </RouterLink>
    </template>

    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div v-if="loading && !detail" class="flex flex-1 items-center justify-center">
        <Spinner size="lg">Loading issue…</Spinner>
      </div>

      <EmptyState
        v-else-if="empty"
        title="No occurrences in range"
        description="Widen the time range to see this issue's events."
        class="h-auto flex-1"
      />

      <template v-else-if="detail">
        <!-- Hero: exception identity + summary stats -->
        <section class="px-5 pt-5">
          <div class="rounded-xl border border-border bg-card p-4">
            <div class="flex items-start justify-between gap-4">
              <div class="min-w-0">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="font-semibold text-sev-error">{{ detail.exception_type }}</span>
                  <span class="rounded bg-surface-2 px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-muted-foreground">{{ detail.error_kind }}</span>
                </div>
                <p class="mt-1 truncate font-mono text-sm text-foreground" :title="detail.message">{{ detail.message }}</p>
              </div>
              <RelatedMenu :entity="{ kind: 'rumError', fields: { service: app, traceId: detail.events[0]?.trace_id ?? undefined } }" />
            </div>
            <dl class="mt-4 grid grid-cols-2 gap-3 text-sm sm:grid-cols-4">
              <div>
                <dt class="text-xs text-muted-foreground">Occurrences</dt>
                <dd class="tabular-nums text-foreground">{{ formatNumber(detail.occurrences) }}</dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground">Sessions</dt>
                <dd class="tabular-nums text-foreground">{{ formatNumber(detail.sessions) }}</dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground">First seen</dt>
                <dd class="text-foreground">{{ new Date(detail.first_seen / 1_000_000).toLocaleString() }}</dd>
              </div>
              <div>
                <dt class="text-xs text-muted-foreground">Last seen</dt>
                <dd class="text-foreground">{{ new Date(detail.last_seen / 1_000_000).toLocaleString() }}</dd>
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

        <!-- Tag breakdowns -->
        <section v-if="detail.tags.length" class="px-5 pt-6">
          <span class="block pb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">Breakdown</span>
          <div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div v-for="tag in detail.tags" :key="tag.field" class="rounded-xl border border-border bg-card p-4">
              <h3 class="mb-2 font-mono text-xs text-muted-foreground">{{ tag.field }}</h3>
              <ul class="space-y-1.5">
                <li v-for="v in tag.values" :key="v.value" class="flex items-center gap-2 text-xs">
                  <span class="w-24 shrink-0 truncate font-mono text-foreground" :title="v.value">{{ v.value }}</span>
                  <Meter :value="shareFor(tag, v.count)" class="flex-1" />
                  <span class="w-10 shrink-0 text-right tabular-nums text-muted-foreground">{{ formatNumber(v.count) }}</span>
                </li>
              </ul>
            </div>
          </div>
        </section>

        <!-- Sample stack -->
        <section v-if="detail.sample_stack" class="px-5 pt-6">
          <span class="block pb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">Sample stack</span>
          <pre class="overflow-x-auto rounded-xl border border-border bg-card p-4 font-mono text-xs leading-relaxed text-foreground">{{ detail.sample_stack }}</pre>
        </section>

        <!-- Sample events -->
        <section class="px-5 pb-5 pt-6">
          <span class="block pb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">Recent events</span>
          <div class="overflow-hidden rounded-xl border border-border bg-card">
            <Table container-class="overflow-visible" class="border-collapse font-mono text-xs">
              <TableHeader>
                <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
                  <TableHead>Time</TableHead>
                  <TableHead>Route</TableHead>
                  <TableHead>Browser</TableHead>
                  <TableHead>Device</TableHead>
                  <TableHead>Session</TableHead>
                  <TableHead class="text-right"><span class="sr-only">Jump</span></TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="(ev, i) in detail.events" :key="i" data-testid="rum-event-row" class="border-border/60">
                  <TableCell class="py-1.5 text-muted-foreground">{{ new Date(ev.timestamp / 1_000_000).toLocaleTimeString() }}</TableCell>
                  <TableCell class="py-1.5 font-sans text-foreground">{{ ev.route }}</TableCell>
                  <TableCell class="py-1.5 text-muted-foreground">{{ ev.browser }}</TableCell>
                  <TableCell class="py-1.5 text-muted-foreground">{{ ev.device }}</TableCell>
                  <TableCell class="py-1.5 text-muted-foreground">{{ ev.session }}</TableCell>
                  <TableCell class="py-1.5 text-right font-sans">
                    <RouterLink v-if="ev.trace_id" :to="traceHref(ev.trace_id)" class="text-brand hover:underline">Open trace</RouterLink>
                    <a :href="logsHref(ev.session)" class="ml-3 text-brand hover:underline">Open logs</a>
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
          </div>
        </section>
      </template>
    </main>
  </AppShell>
</template>

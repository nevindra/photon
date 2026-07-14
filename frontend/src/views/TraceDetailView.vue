<script setup>
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import TraceWaterfall from '@/components/traces/TraceWaterfall.vue'
import SpanDetailPanel from '@/components/traces/SpanDetailPanel.vue'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { Card } from '@/components/ui/card'
import { ArrowLeft, FileText, Copy } from 'lucide-vue-next'
import { api } from '@/lib/core/api'
import { useTrace } from '@/lib/traces/tracesQueries'
import { getTraceTree } from '@/lib/traces/traceTree'
import { formatDuration, formatFull, formatNumber } from '@/lib/core/format'
import { serviceColorClass } from '@/lib/services/serviceColor'
import { useCopy } from '@/lib/core/useCopy'
import { correlate } from '@/lib/core/useCorrelate'

const route = useRoute()
const router = useRouter()

const selectedSpanId = ref(null)
const collapseHealthy = ref(false)

// The route param is the source of truth: deep-linking `/traces/:id?t=` loads directly, and
// navigating to another id (or the same id from a different `?t=`) re-fetches — all handled by
// useTrace's reactive query key. `?t=` is an optional decimal-nanosecond hint that narrows
// candidate-file selection.
const traceId = computed(() => {
  const id = route.params.traceId
  return ((Array.isArray(id) ? id[0] : id) ?? '').trim()
})
const timeHintNs = computed(() => {
  const t = route.query.t
  return typeof t === 'string' && t ? t : undefined
})
// Optional deep-link span selection: `/traces/:id?span=<id>` pre-selects + scrolls to it (see
// TraceWaterfall's `initialSpanId` prop).
const initialSpanId = computed(() => {
  const s = route.query.span
  return typeof s === 'string' && s ? s : undefined
})

const traceQuery = useTrace(traceId, timeHintNs)

// api.getTrace re-throws only a real 404 ("trace not found"); every other failure falls back to
// the mock and resolves. So a settled error is a genuine not-found; anything else is a load error.
const loading = computed(() => traceQuery.isFetching.value)
const notFound = computed(() => traceQuery.error.value?.status === 404)
const errorMsg = computed(() =>
  traceQuery.error.value && traceQuery.error.value.status !== 404 ? 'Failed to load trace.' : '',
)

const spans = computed(() => traceQuery.data.value?.spans ?? [])
const loadedTraceId = computed(() => traceQuery.data.value?.trace_id ?? traceId.value)

// Build the trace tree once (memoised on the spans array ref) and share it with the waterfall via
// its `tree` prop, so the summary strip, breakdown band, and waterfall don't each rebuild it.
const trace = computed(() => getTraceTree(spans.value))
const selectedSpan = computed(
  () => spans.value.find((s) => s.span_id === selectedSpanId.value) ?? null,
)
const selectedNode = computed(() => trace.value.nodes.get(selectedSpanId.value) ?? null)
const selectedParentNode = computed(() =>
  selectedNode.value?.parentId ? (trace.value.nodes.get(selectedNode.value.parentId) ?? null) : null,
)
const shortId = computed(() => (loadedTraceId.value ? loadedTraceId.value.slice(0, 12) : ''))
// AppShell/ContextBar breadcrumb — time itself is global now (lib/context.js via the ContextBar in
// AppShell), this view has no local time window of its own to migrate.
const crumb = computed(() => 'Traces' + (shortId.value ? ' › ' + shortId.value : ''))

// "Time by service" breakdown band: proportions of total self-time, guarded against a zero total
// (e.g. every span has zero duration).
const totalSelfNs = computed(() => trace.value.serviceSelfTime.reduce((sum, s) => sum + s.selfNs, 0n))
const serviceBreakdown = computed(() => {
  const total = totalSelfNs.value
  if (total <= 0n) return []
  return trace.value.serviceSelfTime.map((s) => ({
    service: s.service,
    selfNs: s.selfNs,
    widthPct: (Number(s.selfNs) / Number(total)) * 100,
  }))
})

// A new trace clears the previous span selection.
watch(traceId, () => {
  selectedSpanId.value = null
})

// Esc closes the span panel. The waterfall itself owns j/k/←/→ (row nav + collapse).
function onWindowKeydown(event) {
  if (event.key === 'Escape') selectedSpanId.value = null
}
onMounted(() => window.addEventListener('keydown', onWindowKeydown))
onUnmounted(() => window.removeEventListener('keydown', onWindowKeydown))

function backToTraces() {
  router.push('/traces')
}

function openLogs(query) {
  router.push(correlate({ path: '/logs', query: { q: query } }))
}

function viewAllLogs() {
  openLogs(`trace_id:${loadedTraceId.value}`)
}

const { copy } = useCopy()

async function copyTraceId() {
  if (loadedTraceId.value) await copy(loadedTraceId.value, 'trace ID')
}
</script>

<template>
  <AppShell active="traces" :mock="api.mock" :crumb="crumb">
    <!-- The old header row (back button + Traces/root breadcrumb + trace-id chip + view-all-logs)
         folds into the one ContextBar: back button → lead slot, trace-id chip + view-all-logs →
         actions slot. The crumb already reads "Traces › <short id>"; the root op is still shown in
         the summary strip's "Root" cell below. -->
    <template #lead>
      <Button
        data-testid="back-to-traces"
        size="icon"
        variant="ghost"
        class="size-7 shrink-0"
        aria-label="Back to traces"
        @click="backToTraces"
      >
        <ArrowLeft class="size-4" />
      </Button>
    </template>
    <template #actions>
      <div v-if="loadedTraceId" class="flex shrink-0 items-center gap-2">
        <button
          type="button"
          class="flex items-center gap-1.5 rounded-md bg-muted px-2 py-1 font-mono text-[11px] text-muted-foreground transition-colors hover:text-foreground"
          title="Copy trace ID"
          @click="copyTraceId"
        >
          {{ shortId }}…
          <Copy class="size-3" />
        </button>
        <Button variant="ghost" size="sm" @click="viewAllLogs">
          <FileText class="mr-1.5 size-3.5" />
          View all logs
        </Button>
      </div>
    </template>

    <!-- Summary strip: hairline-divided stats. -->
    <div
      v-if="loadedTraceId && spans.length"
      class="flex items-stretch gap-0 border-b border-border px-5 py-3"
    >
      <div class="flex flex-col justify-center gap-0.5 pr-4">
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">Root</span>
        <span class="font-mono text-xs text-foreground">{{ trace.rootService }} · {{ trace.rootName }}</span>
      </div>
      <div class="flex flex-col justify-center gap-0.5 border-l border-border px-4">
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">Duration</span>
        <span class="font-mono text-base font-semibold text-foreground">{{ formatDuration(trace.durationNs) }}</span>
      </div>
      <div class="flex flex-col justify-center gap-0.5 border-l border-border px-4">
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">Spans</span>
        <span class="font-mono text-xs text-foreground">{{ formatNumber(trace.spanCount) }}</span>
      </div>
      <div class="flex flex-col justify-center gap-0.5 border-l border-border px-4">
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">Services</span>
        <span class="font-mono text-xs text-foreground">{{ formatNumber(trace.serviceCount) }}</span>
      </div>
      <div v-if="trace.errorCount" class="flex flex-col justify-center gap-0.5 border-l border-border px-4">
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">Errors</span>
        <span class="font-mono text-xs text-sev-error">{{ formatNumber(trace.errorCount) }}</span>
      </div>
      <div class="ml-auto flex flex-col justify-center gap-0.5 text-right">
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">Started</span>
        <span class="font-mono text-xs text-muted-foreground">{{ formatFull(trace.startNs) }}</span>
      </div>
    </div>

    <!-- "Time by service" breakdown band — the one intentional splash of color. -->
    <div v-if="loadedTraceId && spans.length" class="border-b border-border px-5 py-3">
      <div class="mb-1.5 flex items-center justify-between gap-3">
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">Time by service</span>
        <span v-if="serviceBreakdown.length" class="font-mono text-[10px] text-muted-foreground/70">
          self-time · {{ formatDuration(totalSelfNs) }} across {{ formatNumber(trace.serviceCount) }} service{{
            trace.serviceCount === 1 ? '' : 's'
          }}
        </span>
      </div>
      <div class="flex h-3.5 overflow-hidden rounded-sm bg-muted">
        <div
          v-for="seg in serviceBreakdown"
          :key="seg.service"
          data-testid="breakdown-segment"
          :class="serviceColorClass(seg.service)"
          class="h-full transition-[filter] hover:brightness-110"
          :style="{ width: seg.widthPct + '%' }"
          :title="`${seg.service} · ${formatDuration(seg.selfNs)}`"
        />
      </div>
      <div v-if="serviceBreakdown.length" class="mt-2 flex flex-wrap gap-x-4 gap-y-1">
        <span
          v-for="seg in serviceBreakdown"
          :key="'legend-' + seg.service"
          class="flex items-center gap-1.5 font-mono text-xs text-muted-foreground"
        >
          <span :class="serviceColorClass(seg.service)" class="size-2 shrink-0 rounded-full" />
          {{ seg.service }}
          <span class="text-foreground">{{ formatDuration(seg.selfNs) }}</span>
        </span>
      </div>
    </div>

    <!-- Body. -->
    <div class="flex min-h-0 flex-1">
      <template v-if="loading">
        <div class="flex flex-1 flex-col gap-4 p-5" data-testid="trace-skeleton">
          <div class="flex gap-4">
            <Skeleton class="h-8 w-40" />
            <Skeleton class="h-8 w-24" />
            <Skeleton class="h-8 w-24" />
          </div>
          <Skeleton class="h-3.5 w-full" />
          <div class="space-y-2">
            <Skeleton
              v-for="i in 6"
              :key="i"
              class="h-3"
              :style="{ width: 92 - i * 9 + '%' }"
            />
          </div>
        </div>
      </template>
      <template v-else-if="errorMsg">
        <div class="flex flex-1">
          <EmptyState
            :title="errorMsg"
            description="Check the trace ID and try again."
          />
        </div>
      </template>
      <template v-else-if="notFound">
        <div class="flex flex-1">
          <EmptyState
            title="Trace not found"
            description="No spans for this trace ID in the hot store."
          />
        </div>
      </template>
      <template v-else-if="loadedTraceId && spans.length">
        <TraceWaterfall
          :spans="spans"
          :tree="trace"
          :selected-span-id="selectedSpanId"
          :initial-span-id="initialSpanId"
          v-model:collapse-healthy="collapseHealthy"
          @select-span="selectedSpanId = $event"
        />
        <SpanDetailPanel
          :span="selectedSpan"
          :trace-id="loadedTraceId"
          :node="selectedNode"
          :parent-node="selectedParentNode"
          @close="selectedSpanId = null"
          @view-logs="openLogs($event.query)"
        />
      </template>
      <template v-else>
        <div class="flex flex-1">
          <EmptyState
            title="Open a trace"
            description="Pick a trace from the explorer, or jump here from a log line's trace link."
          />
        </div>
      </template>
    </div>
  </AppShell>
</template>

<script setup lang="ts">
// The Overview page's logs panel, and it closes a real hole: a service can be on fire in its logs
// while its spans look completely normal. A dropped background job, a config parse failure, a
// connection pool warning every 200ms — none of those produce an errored SERVER span, so none of
// them move a single number on the RED charts above.
//
// It is deliberately a SUMMARY, not a log viewer: a severity-stacked volume strip and the most
// frequent error messages. Reading individual lines is `/logs`'s job, and `correlate()` is how you
// get there with the window and Site intact.
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { useQuery } from '@tanstack/vue-query'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { api } from '@/lib/core/api'
import { grammarTerm } from '@/lib/core/queryLang'
import { contextQueryTerm, startNs, endNs } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'
import { formatCompact } from '@/lib/core/format'

const props = defineProps<{ service: string }>()
const router = useRouter()

const base = computed(() =>
  [grammarTerm('service', props.service), contextQueryTerm()].filter(Boolean).join(' '),
)
// `level:error,fatal` — the two severities worth putting in front of someone looking at a health
// page. Warnings are in the volume strip, which is where a rising warn rate shows up.
const errorQuery = computed(() => [base.value, 'level:error,fatal'].filter(Boolean).join(' '))

const histogram = useQuery({
  queryKey: computed(() => ['service', 'logs-histogram', base.value, startNs.value, endNs.value]),
  queryFn: ({ signal }) => api.histogram(base.value, startNs.value, endNs.value, 32, { signal }),
  enabled: computed(() => !!props.service),
})

const topErrors = useQuery({
  queryKey: computed(() => ['service', 'logs-top-errors', errorQuery.value, startNs.value, endNs.value]),
  queryFn: ({ signal }) => api.facet('body', errorQuery.value, startNs.value, endNs.value, 5, { signal }),
  enabled: computed(() => !!props.service),
})

const buckets = computed(() => histogram.data.value ?? [])
const maxTotal = computed(() => Math.max(1, ...buckets.value.map((b) => b.total ?? 0)))
const totals = computed(() => {
  const sum = (k: 'error' | 'fatal' | 'warn' | 'total') =>
    buckets.value.reduce((a, b) => a + (b[k] ?? 0), 0)
  return { errors: sum('error') + sum('fatal'), warns: sum('warn'), total: sum('total') }
})

// Severity stack, worst at the top of each bar. Literal class strings — Tailwind cannot see
// computed names.
const LAYERS = [
  { key: 'fatal', cls: 'bg-sev-fatal' },
  { key: 'error', cls: 'bg-sev-error' },
  { key: 'warn', cls: 'bg-sev-warn' },
  { key: 'info', cls: 'bg-primary/50' },
  { key: 'debug', cls: 'bg-muted-foreground/30' },
] as const

const errors = computed(() => topErrors.data.value?.values ?? [])
const loading = computed(() => histogram.isPending.value || topErrors.isPending.value)

function openLogs(extra?: string): void {
  const q = [base.value, extra].filter(Boolean).join(' ')
  router.push(correlate({ path: '/logs', query: { q } }))
}
// A message is opened as a quoted free-text term: log bodies contain spaces, and the grammar reads
// a bare quoted phrase as a body substring — which is exactly the match wanted here.
function openMessage(message: string): void {
  if (message.includes('"')) return openLogs('level:error,fatal')
  openLogs(`level:error,fatal "${message}"`)
}
</script>

<template>
  <section class="flex flex-col overflow-hidden rounded-xl border border-border bg-card shadow-1">
    <header class="flex items-center justify-between gap-2 px-4 pb-2 pt-3.5">
      <h3 class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Logs</h3>
      <button type="button" class="text-[11px] text-primary hover:underline" @click="openLogs()">
        Open in Logs ↗
      </button>
    </header>

    <div v-if="loading" class="flex flex-1 items-center justify-center py-10">
      <Spinner size="sm">loading…</Spinner>
    </div>

    <EmptyState
      v-else-if="!totals.total"
      title="No logs in range"
      description="This service recorded no log lines in the selected window."
      class="h-auto flex-1"
    />

    <template v-else>
      <div class="px-4 pb-3">
        <div class="flex items-center gap-3 pb-2 font-mono text-[11px] tabular-nums">
          <span class="text-muted-foreground">{{ formatCompact(totals.total) }} lines</span>
          <span v-if="totals.errors" class="text-sev-error">{{ formatCompact(totals.errors) }} error</span>
          <span v-if="totals.warns" class="text-sev-warn">{{ formatCompact(totals.warns) }} warn</span>
        </div>
        <div class="flex h-14 items-end gap-px" data-testid="log-volume">
          <div
            v-for="(b, i) in buckets"
            :key="i"
            class="flex min-w-0 flex-1 flex-col justify-end"
            :style="{ height: ((b.total ?? 0) / maxTotal) * 100 + '%' }"
            :title="`${b.total} lines`"
          >
            <span
              v-for="l in LAYERS"
              :key="l.key"
              :class="l.cls"
              :style="{ height: b.total ? ((b[l.key] ?? 0) / b.total) * 100 + '%' : '0%' }"
            />
          </div>
        </div>
      </div>

      <div class="border-t border-border px-4 py-3">
        <h4 class="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          Most frequent errors
        </h4>
        <p v-if="!errors.length" class="text-[11px] text-muted-foreground/70">No error lines in range</p>
        <ul v-else class="space-y-1">
          <li v-for="e in errors" :key="e.value" class="flex items-baseline gap-2 font-mono text-[11px]">
            <button
              type="button"
              data-testid="log-message"
              class="min-w-0 flex-1 truncate text-left text-foreground/80 hover:text-primary hover:underline"
              :title="e.value"
              @click="openMessage(e.value)"
            >
              {{ e.value }}
            </button>
            <span class="shrink-0 tabular-nums text-muted-foreground">{{ formatCompact(e.count) }}</span>
          </li>
        </ul>
      </div>
    </template>
  </section>
</template>

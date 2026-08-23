<script setup lang="ts">
// Releases, newest first, each with what changed after it.
//
// The point of this list is NOT that a deploy happened — the version string alone would say that.
// It is the before/after: the same four numbers measured over the window before the release and
// the window after, so "did this make things worse" is answered on the row rather than by eyeballing
// a chart and guessing where the release sits.
//
// Two rules keep it honest:
//   1. A comparison needs both sides. If the release is younger than one comparison window, or the
//      window before it falls outside the range being looked at, the row says "too recent" rather
//      than diffing against a half-empty window. Half a window against a full one is a fabricated
//      regression.
//   2. Direction is per metric, not per number. Throughput going UP is neutral, error rate going up
//      is bad, Apdex going up is good. A single "green means bigger" rule would paint a traffic
//      spike as an improvement.
import { computed } from 'vue'
import { ArrowDownRight, ArrowUpRight, GitCommitHorizontal, Minus, Radio } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { EmptyState } from '@/components/ui/empty-state'
import { formatDuration, relative } from '@/lib/core/format'
import type { Deployment, ServiceTimeseriesBucket } from '@/lib/core/api'

const props = withDefaults(
  defineProps<{
    deployments: Deployment[]
    /** The service's RED buckets over the same window. Empty = no comparison, just the list. */
    buckets?: ServiceTimeseriesBucket[]
    /** How far either side of a release to measure, in minutes. */
    compareMinutes?: number
    /** Show the service name on each row (the fleet view; a service's own tab does not need it). */
    showService?: boolean
  }>(),
  { buckets: () => [], compareMinutes: 30, showService: false },
)

const emit = defineEmits<{ (e: 'select', d: Deployment): void }>()

/** One side of a comparison: the buckets in `[from, to)` folded into the four headline numbers. */
interface Side {
  buckets: number
  rate: number
  errorRate: number
  p99: number
  apdex: number | null
}

function fold(rows: ServiceTimeseriesBucket[]): Side | null {
  if (!rows.length) return null
  let count = 0
  let errors = 0
  let rate = 0
  let p99 = 0
  let apdexSum = 0
  let apdexN = 0
  for (const b of rows) {
    count += b.count ?? 0
    errors += b.error_count ?? 0
    rate += b.rate ?? 0
    // p99 of a set of p99s is not a p99. Taking the WORST bucket is at least a real measurement
    // that happened, which averaging is not — and a release that adds a tail shows up in it.
    p99 = Math.max(p99, Number(b.p99 ?? 0))
    if (b.apdex !== null && b.apdex !== undefined) {
      apdexSum += b.apdex
      apdexN += 1
    }
  }
  return {
    buckets: rows.length,
    rate: rate / rows.length,
    errorRate: count ? errors / count : 0,
    p99,
    apdex: apdexN ? apdexSum / apdexN : null,
  }
}

const sorted = computed(() => [...props.deployments].sort((a, b) => b.detected_at_ms - a.detected_at_ms))

const windowMs = computed(() => Math.max(1, props.compareMinutes) * 60_000)

/** The earliest/latest bucket we actually hold, so a truncated comparison can be refused. */
const coverage = computed(() => {
  if (!props.buckets.length) return null
  let lo = Infinity
  let hi = -Infinity
  for (const b of props.buckets) {
    lo = Math.min(lo, b.ts)
    hi = Math.max(hi, b.ts)
  }
  return { lo, hi }
})

interface Delta {
  key: string
  label: string
  before: string
  after: string
  /** -1 worse, 0 neutral/unchanged, 1 better. Drives the tint, nothing else does. */
  direction: -1 | 0 | 1
  /** Signed relative change, for the arrow. `null` when the before side was zero. */
  change: number | null
}

/** Below this, a move is noise: instruments wobble and nobody should read a tint into it. */
const NOISE = 0.05

function delta(
  key: string,
  label: string,
  before: number,
  after: number,
  fmt: (n: number) => string,
  higherIsBetter: 0 | 1 | -1,
): Delta {
  const change = before === 0 ? (after === 0 ? 0 : null) : (after - before) / before
  let direction: -1 | 0 | 1 = 0
  if (higherIsBetter !== 0 && change !== null && Math.abs(change) >= NOISE) {
    direction = (change > 0 ? higherIsBetter : -higherIsBetter) as -1 | 0 | 1
  }
  return { key, label, before: fmt(before), after: fmt(after), direction, change }
}

const pct = (n: number) => `${(n * 100).toFixed(2)}%`
const per = (n: number) => `${n.toFixed(n >= 10 ? 1 : 2)}/s`
const dur = (n: number) => formatDuration(n)
const three = (n: number) => n.toFixed(3)

interface Row {
  deploy: Deployment
  /** `null` when there is not enough data on both sides to compare honestly. */
  deltas: Delta[] | null
  /** Why there is no comparison, in one short phrase. */
  reason: string
}

const rows = computed<Row[]>(() =>
  sorted.value.map((deploy) => {
    const cov = coverage.value
    if (!cov) return { deploy, deltas: null, reason: 'no RED data in range' }
    const at = deploy.detected_at_ms
    const from = at - windowMs.value
    const to = at + windowMs.value
    // Name the side that is missing, not just "no data": "too recent" and "the window before it is
    // outside the range" are fixed by opposite actions — wait, versus widen the time range.
    if (to > cov.hi) return { deploy, deltas: null, reason: 'too recent to compare' }
    if (from < cov.lo) {
      return { deploy, deltas: null, reason: 'the window before it is outside the time range' }
    }
    const before = fold(props.buckets.filter((b) => b.ts >= from && b.ts < at))
    const after = fold(props.buckets.filter((b) => b.ts >= at && b.ts < to))
    if (!before || !after) return { deploy, deltas: null, reason: 'no traffic on one side' }
    return {
      deploy,
      reason: '',
      deltas: [
        delta('error_rate', 'Error rate', before.errorRate, after.errorRate, pct, -1),
        delta('p99', 'p99', before.p99, after.p99, dur, -1),
        delta(
          'apdex',
          'Apdex',
          before.apdex ?? 0,
          after.apdex ?? 0,
          (n) => (before.apdex === null || after.apdex === null ? '—' : three(n)),
          before.apdex === null || after.apdex === null ? 0 : 1,
        ),
        // Throughput is context, never a verdict: more traffic is not an improvement and less
        // traffic is not a regression. It is here because a diff in the other three means something
        // different at 10x the load.
        delta('rate', 'Throughput', before.rate, after.rate, per, 0),
      ],
    }
  }),
)

function tint(d: Delta): string {
  if (d.direction > 0) return 'text-success'
  if (d.direction < 0) return 'text-sev-error'
  return 'text-muted-foreground'
}

function arrow(d: Delta) {
  if (d.change === null || Math.abs(d.change) < NOISE) return Minus
  return d.change > 0 ? ArrowUpRight : ArrowDownRight
}

const shortSha = (sha: string | null) => (sha ? sha.slice(0, 7) : '')

// `relative` speaks nanoseconds like the rest of the app; this table stores milliseconds.
const ago = (ms: number) => relative(BigInt(Math.round(ms)) * 1_000_000n)
</script>

<template>
  <div v-if="rows.length" data-testid="deployment-timeline" class="flex flex-col">
    <article
      v-for="row in rows"
      :key="row.deploy.id"
      data-testid="deployment-row"
      :data-deploy="row.deploy.id"
      class="flex gap-3 border-b border-border/60 py-3 last:border-b-0"
    >
      <!-- The rail. A dot per release, so the eye reads the list as a sequence in time. -->
      <div class="flex w-4 shrink-0 flex-col items-center pt-1">
        <span class="size-2 rounded-full bg-brand" />
        <span class="mt-1 w-px flex-1 bg-border" />
      </div>

      <div class="flex min-w-0 flex-1 flex-col gap-2">
        <header class="flex flex-wrap items-center gap-2">
          <button
            type="button"
            class="rounded-md font-mono text-sm font-medium text-foreground hover:text-brand focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            @click="emit('select', row.deploy)"
          >
            {{ row.deploy.version }}
          </button>
          <span v-if="showService" class="font-mono text-xs text-muted-foreground">{{ row.deploy.service }}</span>
          <Badge v-if="row.deploy.environment" variant="outline" class="text-[10px]">
            {{ row.deploy.environment }}
          </Badge>
          <!-- `source` is how the row was first noticed, so an enriched detected row still says
               auto-detected — and that is the honest label: nobody told us, we inferred it. -->
          <Badge
            :variant="row.deploy.source === 'ci' ? 'secondary' : 'outline'"
            class="gap-1 text-[10px]"
            :data-source="row.deploy.source"
          >
            <component :is="row.deploy.source === 'ci' ? GitCommitHorizontal : Radio" class="size-3" />
            {{ row.deploy.source === 'ci' ? 'from CI' : 'auto-detected' }}
          </Badge>
          <span class="ml-auto font-mono text-[11px] text-muted-foreground">
            {{ ago(row.deploy.detected_at_ms) }}
          </span>
        </header>

        <p v-if="row.deploy.message" class="truncate text-xs text-foreground/80">
          {{ row.deploy.message }}
        </p>
        <p
          v-if="row.deploy.commit_sha || row.deploy.author"
          class="flex flex-wrap items-center gap-2 font-mono text-[11px] text-muted-foreground"
        >
          <a
            v-if="row.deploy.url && row.deploy.commit_sha"
            :href="row.deploy.url"
            target="_blank"
            rel="noreferrer noopener"
            class="rounded text-brand hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >{{ shortSha(row.deploy.commit_sha) }}</a
          >
          <span v-else-if="row.deploy.commit_sha">{{ shortSha(row.deploy.commit_sha) }}</span>
          <span v-if="row.deploy.author">{{ row.deploy.author }}</span>
        </p>

        <div v-if="row.deltas" data-testid="deployment-deltas" class="flex flex-wrap gap-x-6 gap-y-1.5">
          <div v-for="d in row.deltas" :key="d.key" :data-delta="d.key" class="flex flex-col gap-0.5">
            <span class="text-[10px] uppercase tracking-wider text-muted-foreground">{{ d.label }}</span>
            <span class="flex items-center gap-1 font-mono text-xs tabular-nums" :class="tint(d)">
              <span class="text-muted-foreground">{{ d.before }}</span>
              <component :is="arrow(d)" class="size-3" />
              <span data-delta-after>{{ d.after }}</span>
            </span>
          </div>
        </div>
        <p v-else data-testid="deployment-no-compare" class="text-[11px] text-muted-foreground">
          No before/after — {{ row.reason }}.
        </p>
      </div>
    </article>
  </div>

  <EmptyState
    v-else
    data-testid="deployment-empty"
    title="No releases in this window"
    description="A release is noticed the first time a service reports a service.version nobody has seen before. A service that never sets service.version never shows up here."
    class="h-auto py-10"
  />
</template>

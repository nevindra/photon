<script setup lang="ts">
// One objective: how much budget is left, and how fast it is going.
//
// The remaining-budget bar is the headline because it is the only number on the service page that
// is already a decision. "Error rate 0.4%" needs context to act on; "12% of the month left, gone in
// three hours" does not.
//
// Three rules this card holds to:
//
//   1. **Burn rate is a pair, never a number.** A single window either cries wolf (short) or
//      notices an outage an hour late (long). The card shows the long window's rate and marks it
//      firing only when the short window confirms it, which is what the API computes.
//   2. **A latency objective says what it actually measured.** The rollup histogram only knows its
//      own bucket edges, so a 300ms threshold is measured at 200ms — and the card says so rather
//      than letting someone believe a line was held that was never tested.
//   3. **Partial is stated, not hidden.** A budget computed over four hours of a thirty-day window
//      is not a thirty-day budget, and rendering it as one is how a green card ends up over an
//      outage.
import { computed } from 'vue'
import { AlertTriangle, Flame } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { formatCompact } from '@/lib/core/format'
import type { SloBudget } from '@/lib/core/api'

const props = defineProps<{ budget: SloBudget }>()

const slo = computed(() => props.budget.slo)

/** The promise, spelled the way people say it out loud. */
const targetLabel = computed(() => {
  const pct = slo.value.target * 100
  // 99.9 not 99.90000000000001 — the target is a stored float and three nines is 0.999.
  const text = `${Number(pct.toFixed(4))}%`
  return slo.value.kind === 'latency'
    ? `${text} under ${slo.value.threshold_ms}ms`
    : `${text} of requests succeed`
})

const remainingPct = computed(() => {
  const f = props.budget.remaining_fraction
  return f === null ? null : f * 100
})

/**
 * Bar width. Clamped to `[0, 100]` — an overspent budget is a full red bar, not a bar that runs
 * off the card. The NUMBER beside it is not clamped, because "−240%" is a real thing to be told.
 */
const barPct = computed(() => Math.min(100, Math.max(0, remainingPct.value ?? 0)))

const tone = computed(() => {
  const pct = remainingPct.value
  if (pct === null) return 'unknown'
  if (pct <= 0) return 'error'
  if (props.budget.fast.firing) return 'error'
  if (pct < 25 || props.budget.slow.firing) return 'warn'
  return 'ok'
})

const BAR: Record<string, string> = {
  ok: 'bg-success',
  warn: 'bg-sev-warn',
  error: 'bg-sev-error',
  unknown: 'bg-muted-foreground/40',
}
const TEXT: Record<string, string> = {
  ok: 'text-success',
  warn: 'text-sev-warn',
  error: 'text-sev-error',
  unknown: 'text-muted-foreground',
}

/** Seconds → the coarsest useful unit. An exhaustion time in seconds is not actionable. */
function untilLabel(secs: number): string {
  if (secs <= 0) return 'already spent'
  const mins = secs / 60
  if (mins < 60) return `${Math.round(mins)}m`
  const hours = mins / 60
  if (hours < 48) return `${hours < 10 ? hours.toFixed(1) : Math.round(hours)}h`
  return `${Math.round(hours / 24)}d`
}

const exhausts = computed(() => {
  const s = props.budget.exhausts_in_secs
  return s === null ? null : untilLabel(s)
})

const burnLabel = (rate: number) => (rate >= 100 ? '99+×' : `${rate.toFixed(rate >= 10 ? 0 : 1)}×`)

/** The latency threshold that was actually measured, when it is not the one configured. */
const measuredNote = computed(() => {
  const m = props.budget.measured_threshold_ms
  if (slo.value.kind !== 'latency' || m === null || m === slo.value.threshold_ms) return null
  return `measured at ${m}ms — the nearest bucket edge at or below ${slo.value.threshold_ms}ms`
})
</script>

<template>
  <article
    data-testid="slo-card"
    :data-tone="tone"
    class="flex flex-col gap-2.5 rounded-xl border border-border bg-card p-4"
  >
    <header class="flex flex-wrap items-baseline gap-2">
      <h3 class="text-sm font-medium text-foreground">{{ slo.name }}</h3>
      <Badge variant="outline" class="text-[10px]">{{ slo.window_days }}d</Badge>
      <Badge v-if="slo.operation" variant="outline" class="max-w-[14rem] truncate font-mono text-[10px]">
        {{ slo.operation }}
      </Badge>
      <span class="ml-auto font-mono text-[11px] text-muted-foreground">{{ targetLabel }}</span>
    </header>

    <!-- Budget left. The one number this card exists for. -->
    <div class="flex flex-col gap-1.5">
      <div class="flex items-baseline justify-between">
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">Error budget left</span>
        <span
          data-testid="slo-remaining"
          class="font-mono text-lg font-medium tabular-nums"
          :class="TEXT[tone]"
        >
          {{ remainingPct === null ? '—' : `${remainingPct.toFixed(remainingPct < 10 && remainingPct > -10 ? 1 : 0)}%` }}
        </span>
      </div>
      <div class="h-1.5 overflow-hidden rounded-full bg-muted">
        <div
          data-testid="slo-bar"
          class="h-full rounded-full transition-[width] duration-300"
          :class="BAR[tone]"
          :style="{ width: `${barPct}%` }"
        />
      </div>
      <p class="font-mono text-[11px] text-muted-foreground">
        {{ formatCompact(Math.max(0, Math.round(budget.remaining_events))) }} of
        {{ formatCompact(Math.round(budget.budget_events)) }} allowed
        <template v-if="budget.sli !== null">
          · SLI {{ (budget.sli * 100).toFixed(3) }}%
        </template>
      </p>
    </div>

    <!-- Burn. The long window's rate; firing needs the short one to agree. -->
    <div class="flex flex-wrap items-center gap-x-4 gap-y-1.5 border-t border-border/60 pt-2.5">
      <div v-for="w in [budget.fast, budget.slow]" :key="w.long" :data-burn="w.long" class="flex items-center gap-1.5">
        <Flame v-if="w.firing" class="size-3 text-sev-error" />
        <span class="text-[10px] uppercase tracking-wider text-muted-foreground">{{ w.long }}</span>
        <span
          class="font-mono text-xs tabular-nums"
          :class="w.firing ? 'text-sev-error' : 'text-foreground/80'"
          :title="`${w.short} confirming window: ${burnLabel(w.short_rate)}`"
        >
          {{ burnLabel(w.long_rate) }}
        </span>
      </div>
      <span v-if="exhausts" data-testid="slo-exhausts" class="ml-auto font-mono text-[11px]" :class="TEXT[tone]">
        {{ exhausts === 'already spent' ? 'budget spent' : `gone in ${exhausts}` }}
      </span>
    </div>

    <p v-if="measuredNote" data-testid="slo-measured-note" class="text-[11px] text-muted-foreground">
      {{ measuredNote }}
    </p>

    <p
      v-if="budget.partial"
      data-testid="slo-partial"
      class="flex items-start gap-1.5 text-[11px] text-sev-warn"
    >
      <AlertTriangle class="mt-px size-3 shrink-0" />
      <span>
        Measured over part of the window — the rollup does not reach back {{ slo.window_days }} days
        yet, so this is not a full {{ slo.window_days }}-day budget.
      </span>
    </p>
  </article>
</template>

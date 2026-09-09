<script setup lang="ts">
// "Did it work?" — after an apply, the stack's APM services 15 minutes before against 15 minutes
// after: traffic, error rate, p99, and one verdict. This is the tie-in the whole feature exists
// for: the deploy and the proof it worked, on one page, from the telemetry already flowing.
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { Activity, ArrowRight } from 'lucide-vue-next'
import { StatusPill } from '@/components/ui/status-pill'
import { Spinner } from '@/components/ui/spinner'
import { useDeployImpact } from '@/lib/deploy/deployQueries'
import { VERDICT_META } from '@/lib/deploy/impact'
import { formatDuration } from '@/lib/core/format'
import { correlate } from '@/lib/core/useCorrelate'
import { stampMs } from '@/lib/deploy/audit'

const props = defineProps<{ services: string[]; appliedMs: number | null; title?: string }>()
const { summary, loading, windows } = useDeployImpact(() => props.services, () => props.appliedMs)
const pct = (v: number) => `${(v * 100).toFixed(1)}%`
const rate = (v: number) => `${v.toFixed(1)}/s`
const dir = (before: number, after: number, badWhenUp: boolean) => (after === before ? '' : (after > before) === badWhenUp ? 'text-sev-error' : 'text-success')
const windowLabel = computed(() => (props.appliedMs ? `15 min either side of ${stampMs(props.appliedMs)}` : ''))
</script>

<template>
  <section v-if="appliedMs && services.length" class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="deploy-impact">
    <header class="flex flex-wrap items-center gap-3 border-b border-border px-4 py-2.5">
      <Activity class="size-4 text-brand" />
      <h3 class="text-sm font-medium text-foreground">{{ title ?? 'Did it work?' }}</h3>
      <span class="text-xs text-muted-foreground">{{ windowLabel }}</span>
      <template v-if="summary">
        <StatusPill :tone="VERDICT_META[summary.verdict].tone" :solid="summary.verdict === 'look'" class="ml-auto" data-testid="impact-verdict" :data-verdict="summary.verdict">{{ VERDICT_META[summary.verdict].label }}</StatusPill>
      </template>
    </header>
    <div v-if="loading && !summary" class="flex justify-center py-6"><Spinner size="md">Comparing…</Spinner></div>
    <template v-else-if="summary">
      <!-- No signal is one line, not a card's worth of empty: the header already names the window. -->
      <p v-if="summary.verdict === 'nosignal'" class="px-4 py-2.5 text-xs text-muted-foreground" data-testid="impact-nosignal"><span class="text-foreground">{{ summary.headline }}</span> {{ services.join(', ') }} sent no requests in these windows — nothing to compare. The verdict fills in as traffic arrives.</p>
      <p v-else class="px-4 pt-3 text-sm text-foreground">{{ summary.headline }}</p>
      <table v-if="summary.verdict !== 'nosignal'" class="mt-2 w-full text-xs">
        <thead><tr class="text-[10px] uppercase tracking-wider text-muted-foreground"><th class="px-4 py-1.5 text-left font-medium">Service</th><th class="px-2 py-1.5 text-right font-medium">Traffic</th><th class="px-2 py-1.5 text-right font-medium">Error rate</th><th class="px-2 py-1.5 text-right font-medium">p99</th><th class="px-4 py-1.5 text-left font-medium">Reading</th></tr></thead>
        <tbody>
          <tr v-for="s in summary.services" :key="s.service" class="border-t border-border/60" :data-service="s.service" :data-verdict="s.verdict ?? 'none'">
            <td class="px-4 py-1.5"><RouterLink :to="correlate({ path: `/services/${encodeURIComponent(s.service)}` })" class="font-mono text-foreground hover:underline">{{ s.service }}</RouterLink></td>
            <td class="whitespace-nowrap px-2 py-1.5 text-right tabular-nums text-muted-foreground">{{ rate(s.rateBefore) }} <ArrowRight class="inline size-3" /> <span :class="dir(s.rateBefore, s.rateAfter, false)">{{ rate(s.rateAfter) }}</span></td>
            <td class="whitespace-nowrap px-2 py-1.5 text-right tabular-nums text-muted-foreground">{{ pct(s.errorRateBefore) }} <ArrowRight class="inline size-3" /> <span :class="dir(s.errorRateBefore, s.errorRateAfter, true)">{{ pct(s.errorRateAfter) }}</span></td>
            <td class="whitespace-nowrap px-2 py-1.5 text-right tabular-nums text-muted-foreground">{{ formatDuration(s.p99Before) }} <ArrowRight class="inline size-3" /> <span :class="dir(s.p99Before, s.p99After, true)">{{ formatDuration(s.p99After) }}</span></td>
            <td class="px-4 py-1.5 text-muted-foreground">{{ s.reasons.join(' · ') }}</td>
          </tr>
        </tbody>
      </table>
      <p v-if="summary.verdict !== 'nosignal'" class="px-4 py-2 text-[11px] text-muted-foreground">Same numbers as the Backend world for these windows. A prompt to look, not an SLO — the release marker is on each service's charts.</p>
    </template>
  </section>
</template>

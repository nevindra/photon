<script setup lang="ts">
// One card in the Site band. It is a FILTER, not a link: clicking sets the global tenant context,
// clicking the active one clears it (ADR-0004 — the Site picker narrows the board, it does not
// navigate away from it).
//
// Every number here comes from the same merged rows the table renders, so the band cannot disagree
// with the table under it. What the card adds is the part no row carries: whether the Site is still
// pushing at all.
import { computed } from 'vue'
import { Card } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Sparkline } from '@/components/ui/sparkline'
import { StatusDot } from '@/components/ui/status-dot'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { relative, formatCompact } from '@/lib/core/format'
import { STATUS_META } from '@/lib/services/serviceHealth'
import type { SiteStat } from '@/lib/services/servicesOverview'

const props = defineProps<{ site: SiteStat; active?: boolean }>()
const emit = defineEmits<{ select: [key: string | null] }>()

const STATUS_TONE = { up: 'success', stale: 'warning', down: 'error' } as const

const meta = computed(() => STATUS_META[props.site.level])
const spark = computed(() => props.site.spark.map((p) => p[1]))

// `last_seen_ms: 0` is the backend's "never reported" — through `relative()` that would render as
// the distance from the epoch ("488000h ago").
const lastSeen = computed(() => {
  const ms = props.site.lastSeenMs
  if (ms == null) return null
  return ms > 0 ? relative(BigInt(ms) * 1_000_000n) : 'Never reported'
})

const rateText = computed(() => {
  const r = props.site.rate
  return (r >= 100 ? formatCompact(Math.round(r)) : r.toFixed(1)) + '/s'
})
</script>

<template>
  <Card
    interactive
    role="button"
    tabindex="0"
    :aria-pressed="!!active"
    data-testid="site-card"
    :data-site="site.key ?? '__all__'"
    :data-level="site.level"
    :class="[
      'relative flex w-52 shrink-0 flex-col gap-2 overflow-hidden p-3 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring',
      active && 'border-primary/60 ring-1 ring-primary/30',
      site.stale && 'bg-muted/40',
    ]"
    @click="emit('select', site.key)"
    @keydown.enter.prevent="emit('select', site.key)"
    @keydown.space.prevent="emit('select', site.key)"
  >
    <!-- The card's own Attention Level, as a rail rather than a fill: the band must not out-shout
         the attention cards below it. -->
    <span class="absolute inset-y-2 left-0 w-[3px] rounded-r-full" :class="meta.rail" />

    <div class="flex items-center gap-1.5 pl-1.5">
      <StatusDot v-if="site.status" :tone="STATUS_TONE[site.status]" />
      <span class="min-w-0 flex-1 truncate text-[13px] font-medium text-foreground">{{ site.name }}</span>
      <Badge v-if="site.stale" variant="warning" data-testid="site-stale" class="px-1.5 py-0 text-[10px]">stale</Badge>
      <Tooltip v-else-if="site.mode">
        <TooltipTrigger as-child>
          <Badge variant="outline" class="cursor-help px-1.5 py-0 text-[10px]">{{ site.mode }}</Badge>
        </TooltipTrigger>
        <TooltipContent class="max-w-56 text-xs">
          {{
            site.mode.startsWith('full')
              ? 'Raw telemetry is mirrored here — drill in without leaving Central.'
              : 'Health summary only. Raw telemetry stays on this Site.'
          }}
        </TooltipContent>
      </Tooltip>
    </div>

    <div class="flex items-end justify-between gap-2 pl-1.5">
      <div class="min-w-0">
        <p class="font-mono text-lg font-semibold leading-none tabular-nums text-foreground">
          {{ site.stale ? '—' : rateText }}
        </p>
        <p class="mt-1 text-[11px] text-muted-foreground">
          {{ site.services }} {{ site.services === 1 ? 'service' : 'services' }}
        </p>
      </div>
      <Sparkline v-if="!site.stale" :points="spark" class="shrink-0 text-primary/70" :width="64" :height="22" />
    </div>

    <div class="flex items-center gap-2 border-t border-border pl-1.5 pt-2 text-[11px] text-muted-foreground">
      <span v-if="site.attention" data-testid="site-attention" :class="meta.text">
        {{ site.attention }} need{{ site.attention === 1 ? 's' : '' }} attention
      </span>
      <span v-else-if="site.stale" class="text-sev-warn">No recent data</span>
      <span v-else class="text-success">All OK</span>
      <span v-if="lastSeen" class="ml-auto shrink-0">{{ lastSeen }}</span>
    </div>
  </Card>
</template>

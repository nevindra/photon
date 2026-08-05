<script setup lang="ts">
// One tenant on the Home tenant board. Structural template is HostCard: interactive Card,
// truncated title, `mt-auto border-t` relative-time footer. `status !== 'up'` gets the same
// error-tinted border HostCard uses for a degraded resource, plus an explicit unreachable line —
// a stale/down tenant has no live rows to show, so the rows below would otherwise read as "fine".
// `compact` renders the same data as one full-width horizontal strip — used when the tenant
// filter narrows the board to a single tenant, where a lone grid card leaves 2/3 dead space.
import { computed } from 'vue'
import { Card } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { StatusDot } from '@/components/ui/status-dot'
import { Sparkline } from '@/components/ui/sparkline'
import { relative, formatCompact, formatBytes } from '@/lib/core/format'
import type { TenantSummary } from '@/lib/core/api'

const props = defineProps<{ tenant: TenantSummary; compact?: boolean }>()
const emit = defineEmits<{ select: [tenant: TenantSummary] }>()

const STATUS_TONE = { up: 'success', stale: 'warning', down: 'error' } as const

const SUMMARY_TIP =
  'Health summary only: liveness, ingest rate, open incidents, disk usage. Raw telemetry stays on the tenant — use "Open UI" to explore it there.'
// mode arrives as `summary`, `full`, or `full:<signals>` (e.g. `full:traces`) from the tenant's config.
const modeTip = computed(() => {
  const m = props.tenant.mode ?? 'summary'
  if (!m.startsWith('full')) return SUMMARY_TIP
  const subset = m.includes(':') ? m.slice(m.indexOf(':') + 1).split(',') : null
  const what = subset ? subset.join(' + ') : 'logs, traces, and metrics'
  return `Mirrors the tenant's raw ${what} here (plus the health summary) — click the card to explore.`
})

// Backend sends `last_seen_ms: 0` for "never reported" — rendering that through relative()
// would show the distance from the epoch ("488000h ago").
const lastSeen = computed(() =>
  props.tenant.last_seen_ms > 0 ? relative(BigInt(props.tenant.last_seen_ms) * 1_000_000n) : 'Never',
)

const unreachable = computed(() => props.tenant.status !== 'up')
const borderClass = computed(() => (unreachable.value ? 'border-sev-error/40' : ''))
const spark = computed(() => props.tenant.spark.map((p) => p[1]))

function select(): void {
  emit('select', props.tenant)
}
</script>

<template>
  <Card
    v-if="compact"
    interactive
    role="button"
    tabindex="0"
    data-testid="tenant-card"
    :data-tenant="tenant.name"
    :class="['flex cursor-pointer flex-wrap items-center gap-x-6 gap-y-2 px-4 py-3 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring', borderClass]"
    @click="select"
    @keydown.enter.prevent="select"
    @keydown.space.prevent="select"
  >
    <div class="flex min-w-0 items-center gap-2">
      <StatusDot :tone="STATUS_TONE[tenant.status]" />
      <span class="truncate font-medium text-foreground">{{ tenant.name }}</span>
      <Tooltip>
        <TooltipTrigger as-child>
          <Badge variant="outline" class="cursor-help">{{ tenant.mode ?? 'summary' }}</Badge>
        </TooltipTrigger>
        <TooltipContent class="max-w-64 text-xs">{{ modeTip }}</TooltipContent>
      </Tooltip>
    </div>
    <p v-if="unreachable" class="text-xs text-sev-error">Unreachable — no heartbeat</p>
    <div class="flex items-center gap-6 text-xs text-muted-foreground">
      <span>Ingest <span class="tabular-nums text-foreground">{{ formatCompact(tenant.ingest_rows_per_sec) }}/s</span></span>
      <span>Incidents <span class="tabular-nums text-foreground">{{ tenant.open_incidents }}</span></span>
      <span>Hot <span class="tabular-nums text-foreground">{{ formatBytes(tenant.hot_bytes) }}</span></span>
    </div>
    <Sparkline :points="spark" class="h-6 w-32 text-primary" />
    <span class="ml-auto flex shrink-0 items-center gap-4 text-xs text-muted-foreground">
      <a
        v-if="tenant.ui_url"
        :href="tenant.ui_url"
        target="_blank"
        rel="noopener"
        class="text-primary hover:underline"
        @click.stop
      >
        Open UI ↗
      </a>
      {{ lastSeen }}
    </span>
  </Card>

  <Card
    v-else
    interactive
    role="button"
    tabindex="0"
    data-testid="tenant-card"
    :data-tenant="tenant.name"
    :class="['flex cursor-pointer flex-col gap-3 p-4 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring', borderClass]"
    @click="select"
    @keydown.enter.prevent="select"
    @keydown.space.prevent="select"
  >
    <div class="flex items-center justify-between gap-2">
      <span class="truncate font-medium text-foreground">{{ tenant.name }}</span>
      <div class="flex shrink-0 items-center gap-2">
        <Tooltip>
        <TooltipTrigger as-child>
          <Badge variant="outline" class="cursor-help">{{ tenant.mode ?? 'summary' }}</Badge>
        </TooltipTrigger>
        <TooltipContent class="max-w-64 text-xs">{{ modeTip }}</TooltipContent>
      </Tooltip>
        <StatusDot :tone="STATUS_TONE[tenant.status]" />
      </div>
    </div>

    <p v-if="unreachable" class="text-xs text-sev-error">Unreachable — no heartbeat</p>

    <div class="flex flex-col gap-1 text-xs">
      <div class="flex items-center justify-between text-muted-foreground">
        <span>Ingest</span>
        <span class="tabular-nums text-foreground">{{ formatCompact(tenant.ingest_rows_per_sec) }}/s</span>
      </div>
      <div class="flex items-center justify-between text-muted-foreground">
        <span>Open incidents</span>
        <span class="tabular-nums text-foreground">{{ tenant.open_incidents }}</span>
      </div>
      <div class="flex items-center justify-between text-muted-foreground">
        <span>Hot bytes</span>
        <span class="tabular-nums text-foreground">{{ formatBytes(tenant.hot_bytes) }}</span>
      </div>
    </div>

    <Sparkline :points="spark" class="text-primary" />

    <a
      v-if="tenant.ui_url"
      :href="tenant.ui_url"
      target="_blank"
      rel="noopener"
      class="text-[11px] text-primary hover:underline"
      @click.stop
    >
      Open UI ↗
    </a>

    <div class="mt-auto border-t border-border pt-2 text-xs text-muted-foreground">
      {{ lastSeen }}
    </div>
  </Card>
</template>

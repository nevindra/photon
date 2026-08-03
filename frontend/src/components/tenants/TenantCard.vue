<script setup lang="ts">
// One tenant on the Home tenant board. Structural template is HostCard: interactive Card,
// truncated title, `mt-auto border-t` relative-time footer. `status !== 'up'` gets the same
// error-tinted border HostCard uses for a degraded resource, plus an explicit unreachable line —
// a stale/down tenant has no live rows to show, so the rows below would otherwise read as "fine".
import { computed } from 'vue'
import { Card } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { StatusDot } from '@/components/ui/status-dot'
import { Sparkline } from '@/components/ui/sparkline'
import { relative, formatCompact, formatBytes } from '@/lib/core/format'
import type { TenantSummary } from '@/lib/core/api'

const props = defineProps<{ tenant: TenantSummary }>()
const emit = defineEmits<{ select: [tenant: TenantSummary] }>()

const STATUS_TONE = { up: 'success', stale: 'warning', down: 'error' } as const

const unreachable = computed(() => props.tenant.status !== 'up')
const borderClass = computed(() => (unreachable.value ? 'border-sev-error/40' : ''))
const spark = computed(() => props.tenant.spark.map((p) => p[1]))

function select(): void {
  emit('select', props.tenant)
}
</script>

<template>
  <Card
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
        <Badge variant="outline">{{ tenant.mode ?? 'summary' }}</Badge>
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
      v-if="tenant.mode === 'summary' && tenant.ui_url"
      :href="tenant.ui_url"
      target="_blank"
      rel="noopener"
      class="text-[11px] text-primary hover:underline"
      @click.stop
    >
      Open UI ↗
    </a>

    <div class="mt-auto border-t border-border pt-2 text-xs text-muted-foreground">
      {{ relative(BigInt(tenant.last_seen_ms) * 1_000_000n) }}
    </div>
  </Card>
</template>

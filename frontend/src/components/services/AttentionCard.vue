<script setup lang="ts">
// One "needs attention" card on the Services index.
//
// It carries no query of its own any more. The sparkline, the reasons, the Spread and the incident
// count all come off the merged `FleetRow` the table already has — the old card fetched a 16-bucket
// timeseries per card, which meant the strip's cost scaled with how bad things were.
import { computed } from 'vue'
import HealthPill from './HealthPill.vue'
import { Badge } from '@/components/ui/badge'
import { Sparkline } from '@/components/ui/sparkline'
import { Card } from '@/components/ui/card'
import { STATUS_META, reasonText } from '@/lib/services/serviceHealth'
import { formatCompact, formatDuration, relative } from '@/lib/core/format'
import type { FleetRow } from '@/lib/services/servicesOverview'

const props = defineProps<{ row: FleetRow; max?: number }>()
const emit = defineEmits<{ open: [row: FleetRow] }>()

const meta = computed(() => STATUS_META[props.row.level])
// Three lines is the cap: past that the card stops being scannable and the detail page is one
// click away.
const reasons = computed(() => props.row.reasons.slice(0, props.max ?? 3).map(reasonText))
const spark = computed(() => props.row.error_spark.map((p) => p[1]))

// "unhealthy in 1 of 3 sites" — the question you cannot ask from inside a single Site. Hidden when
// there is only one Site, where it says nothing.
const spreadText = computed(() => {
  const { unhealthy, total } = props.row.spread
  return total > 1 ? `${unhealthy} of ${total} sites` : null
})

const since = computed(() => {
  const inc = props.row.incidents
  if (!inc.length) return null
  const oldest = Math.min(...inc.map((i) => i.started_at))
  return relative(BigInt(oldest) * 1_000_000_000n)
})
</script>

<template>
  <Card
    interactive
    role="button"
    tabindex="0"
    data-testid="attention-card"
    :data-service="row.service"
    :data-level="row.level"
    class="relative flex flex-col gap-2 overflow-hidden p-3 pl-4 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
    @click="emit('open', row)"
    @keydown.enter.prevent="emit('open', row)"
    @keydown.space.prevent="emit('open', row)"
  >
    <span class="absolute inset-y-0 left-0 w-1" :class="meta.rail" />

    <div class="flex items-start justify-between gap-2">
      <div class="flex min-w-0 flex-wrap items-center gap-1.5">
        <span class="truncate font-medium text-foreground">{{ row.service }}</span>
        <Badge v-if="row.environment" variant="outline" class="px-1.5 py-0 text-[10px]">{{ row.environment }}</Badge>
      </div>
      <HealthPill :level="row.level" class="shrink-0" />
    </div>

    <ul class="space-y-0.5 font-mono text-[11px]" :class="meta.text">
      <li v-for="r in reasons" :key="r">{{ r }}</li>
    </ul>

    <div class="flex flex-wrap items-center gap-1.5">
      <Badge v-if="spreadText" variant="outline" data-testid="attention-spread" class="px-1.5 py-0 text-[10px]">
        {{ spreadText }}
      </Badge>
      <Badge v-if="row.incidents.length" variant="error" data-testid="attention-incidents" class="px-1.5 py-0 text-[10px]">
        {{ row.incidents.length }} firing
      </Badge>
      <Badge v-if="row.spanBased" variant="secondary" class="px-1.5 py-0 text-[10px]" title="Counted from all spans — this service emits no server or consumer spans">
        span-based
      </Badge>
    </div>

    <div class="mt-auto flex items-center justify-between gap-2 border-t border-border pt-2 text-[11px] text-muted-foreground">
      <span class="tabular-nums">{{ formatCompact(row.count) }} req · p99 {{ formatDuration(row.p99) }}</span>
      <span v-if="since" class="shrink-0">{{ since }}</span>
      <Sparkline v-else :points="spark" class="shrink-0 text-sev-error/70" :width="56" :height="18" />
    </div>
  </Card>
</template>

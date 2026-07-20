<script setup lang="ts">
// A single row in AlertRulesTable. Owns its own `useIncidents({status:'triggered', rule_id})` query
// (server-side filtered per rule) so the status pill reflects THIS rule's open-incident count —
// mirrors MonitorRow's per-row `useHeartbeats(monitor.id)` pattern rather than the parent table
// computing counts from one global incidents fetch. Channel names still come from the parent
// (`channels`, from one shared `useChannels()`) since AlertChannel has no per-id filtered list route.
import { computed } from 'vue'
import { StatusPill } from '@/components/ui/status-pill'
import { Switch } from '@/components/ui/switch'
import { TableRow, TableCell } from '@/components/ui/table'
import { signalColor } from '@/lib/core/signalMeta'
import { useIncidents, useToggleRule } from '@/lib/alertsQueries'
import type { AlertRule, AlertChannel } from '@/lib/core/api'

const props = defineProps<{
  rule: AlertRule
  channels: AlertChannel[]
}>()
const emit = defineEmits<{ edit: [rule: AlertRule] }>()

const triggeredQuery = useIncidents(
  computed(() => ({ status: 'triggered' as const, rule_id: props.rule.id })),
)
const triggeredCount = computed(() => triggeredQuery.data.value?.length ?? 0)

const pillTone = computed(() => {
  if (!props.rule.enabled) return 'neutral'
  return triggeredCount.value > 0 ? 'error' : 'success'
})
const pillLabel = computed(() => {
  if (!props.rule.enabled) return 'paused'
  return triggeredCount.value > 0 ? `triggered · ${triggeredCount.value}` : 'ok'
})

const channelNames = computed(() =>
  props.rule.channel_ids
    .map((id) => props.channels.find((c) => c.id === id)?.name)
    .filter((n): n is string => !!n),
)

// The scope hint under the rule name, e.g. "svc: checkout-api" / "app: storefront" / "by host.name" —
// only shown when the condition carries an obvious, single scope field (traces/rum name it directly;
// metrics/logs fall back to their group_by, if any).
const conditionScope = computed(() => {
  const c = props.rule.condition
  if (c.signal === 'traces') return `svc: ${c.service}`
  if (c.signal === 'rum') return `app: ${c.app_id}`
  if (c.signal === 'metrics' && c.group_by?.length) return `by ${c.group_by.join(', ')}`
  if (c.signal === 'logs' && c.group_by) return `by ${c.group_by}`
  return null
})

const CMP_SYMBOL: Record<string, string> = { gt: '>', gte: '>=', lt: '<', lte: '<=' }

// snake/lower enum value -> the PascalCase spelling Rust's `{:?}` Debug prints for the declared
// variant name ('p50' -> 'P50', 'error_rate' -> 'ErrorRate', 'vital_lcp_p75' -> 'VitalLcpP75') —
// the wire value is lowercase/snake_case via serde rename_all, but Debug prints the Rust identifier.
function pascal(s: string): string {
  return s
    .split('_')
    .map((seg) => seg.charAt(0).toUpperCase() + seg.slice(1))
    .join('')
}

// Mirrors `Condition::summary()` in crates/photon-alerts/src/model.rs exactly, e.g.
// "Avg(system.cpu.utilization) > 0.9".
const conditionSummary = computed(() => {
  const c = props.rule.condition
  const sym = CMP_SYMBOL[c.cmp] ?? c.cmp
  switch (c.signal) {
    case 'metrics':
      return `${pascal(c.agg)}(${c.metric_name}) ${sym} ${c.threshold}`
    case 'logs':
      return `count(${c.query}) ${sym} ${c.threshold}`
    case 'traces':
      return `${pascal(c.kind)}(${c.service}) ${sym} ${c.threshold}`
    case 'rum':
      return `${pascal(c.kind)}(${c.app_id}) ${sym} ${c.threshold}`
    default:
      return ''
  }
})

function fmtFor(secs: number): string {
  if (secs <= 0) return '0m'
  if (secs % 3600 === 0) return `${secs / 3600}h`
  if (secs % 60 === 0) return `${secs / 60}m`
  return `${secs}s`
}

const toggle = useToggleRule()
function onToggle(enabled: boolean) {
  toggle.mutate({ id: props.rule.id, enabled })
}
</script>

<template>
  <TableRow class="cursor-pointer" @click="emit('edit', rule)">
    <TableCell>
      <StatusPill :tone="pillTone">{{ pillLabel }}</StatusPill>
    </TableCell>
    <TableCell>
      <div class="font-medium text-foreground">{{ rule.name }}</div>
      <div class="mt-0.5 text-xs text-muted-foreground">
        {{ rule.severity }}<span v-if="conditionScope"> · {{ conditionScope }}</span>
      </div>
    </TableCell>
    <TableCell>
      <span
        class="inline-flex items-center gap-1.5 rounded-md border border-border bg-muted px-2 py-0.5 text-xs font-medium capitalize"
      >
        <span class="size-1.5 shrink-0 rounded-sm" :style="{ background: signalColor(rule.signal) }" />
        {{ rule.signal }}
      </span>
    </TableCell>
    <TableCell>
      <code
        class="whitespace-nowrap rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-xs text-muted-foreground"
      >{{ conditionSummary }}</code>
    </TableCell>
    <TableCell class="whitespace-nowrap text-xs text-muted-foreground">{{ fmtFor(rule.for_secs) }}</TableCell>
    <TableCell class="text-xs text-muted-foreground">
      {{ channelNames.length ? channelNames.join(', ') : '—' }}
    </TableCell>
    <TableCell class="text-right" @click.stop>
      <Switch :model-value="rule.enabled" :disabled="toggle.isPending.value" @update:model-value="onToggle" />
    </TableCell>
  </TableRow>
</template>

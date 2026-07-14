<script setup>
import { computed } from 'vue'
import StatePill from './StatePill.vue'
import HeartbeatBar from './HeartbeatBar.vue'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { useHeartbeats } from '@/lib/uptime/uptimeQueries'
import { TableRow, TableCell } from '@/components/ui/table'

const props = defineProps({ monitor: { type: Object, required: true } })
defineEmits(['select'])

const hbQ = useHeartbeats(computed(() => props.monitor.id), '24h')
const hb = computed(() => hbQ.data.value)
const heartbeats = computed(() => hb.value?.heartbeats ?? [])
const warn = computed(() => props.monitor.last_state === 'down')
const uptimeText = computed(() => {
  const m = props.monitor
  if (!m.enabled || m.last_state === 'pending') return '—'
  const p = hb.value?.uptime_pct
  return p == null ? '—' : p === 100 ? '100%' : p.toFixed(2) + '%'
})
const latencyText = computed(() =>
  props.monitor.last_latency_ms != null ? props.monitor.last_latency_ms + ' ms' : '—',
)
</script>

<template>
  <TableRow class="cursor-pointer" @click="$emit('select', monitor.id)">
    <TableCell>
      <button
        type="button"
        class="block text-left font-medium text-foreground focus-visible:outline-none focus-visible:underline"
        @click.stop="$emit('select', monitor.id)"
      >
        {{ monitor.name }}
      </button>
      <div class="mt-0.5 truncate font-mono text-xs text-muted-foreground">{{ monitor.target }}</div>
      <!-- Cross-signal jump-off, shown only when a monitor is associated with a backend service. -->
      <div v-if="monitor.service" class="mt-1.5" @click.stop>
        <RelatedMenu :entity="{ kind: 'monitor', fields: { service: monitor.service, host: monitor.host } }" />
      </div>
    </TableCell>
    <TableCell><StatePill :state="monitor.last_state" :paused="!monitor.enabled" /></TableCell>
    <TableCell class="text-right">
      <span class="font-medium tabular-nums" :class="warn ? 'text-sev-error' : 'text-foreground'">{{ uptimeText }}</span>
    </TableCell>
    <TableCell class="text-right tabular-nums text-muted-foreground">{{ latencyText }}</TableCell>
    <TableCell>
      <span class="rounded border border-border px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">{{ monitor.type }}</span>
    </TableCell>
    <TableCell><div class="w-40"><HeartbeatBar :heartbeats="heartbeats" :max="32" size="sm" /></div></TableCell>
  </TableRow>
</template>

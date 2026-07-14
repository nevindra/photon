<script setup>
import { computed } from 'vue'
import StatePill from './StatePill.vue'
import HeartbeatBar from './HeartbeatBar.vue'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { Card } from '@/components/ui/card'
import { useHeartbeats } from '@/lib/uptime/uptimeQueries'

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
  <Card
    role="button"
    tabindex="0"
    class="cursor-pointer p-4 transition-colors hover:border-foreground/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
    @click="$emit('select', monitor.id)"
    @keydown.enter.prevent="$emit('select', monitor.id)"
    @keydown.space.prevent="$emit('select', monitor.id)"
  >
    <div class="flex items-start justify-between gap-3">
      <div class="min-w-0">
        <div class="truncate font-medium text-foreground">{{ monitor.name }}</div>
        <div class="truncate font-mono text-xs text-muted-foreground">{{ monitor.target }}</div>
      </div>
      <StatePill :state="monitor.last_state" :paused="!monitor.enabled" />
    </div>
    <div class="mt-3 flex items-baseline gap-2 text-xs text-muted-foreground">
      <span class="text-lg font-semibold tabular-nums" :class="warn ? 'text-sev-error' : 'text-foreground'">{{ uptimeText }}</span>
      <span>·</span>
      <span class="tabular-nums">{{ latencyText }}</span>
      <span>·</span>
      <span class="uppercase tracking-wide">{{ monitor.type }}</span>
    </div>
    <div class="mt-3"><HeartbeatBar :heartbeats="heartbeats" :max="34" size="sm" /></div>
    <!-- Cross-signal jump-off, shown only when a monitor is associated with a backend service. -->
    <div v-if="monitor.service" class="mt-3" @click.stop @keydown.stop>
      <RelatedMenu :entity="{ kind: 'monitor', fields: { service: monitor.service, host: monitor.host } }" />
    </div>
  </Card>
</template>

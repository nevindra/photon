<script setup>
// One "needs attention" card. Owns a coarse (16-bucket) per-service rate timeseries for the
// sparkline — deliberately only rendered for the ≤3 attention services, so this fetch is bounded.
import { computed } from 'vue'
import HealthPill from './HealthPill.vue'
import { Sparkline } from '@/components/ui/sparkline'
import { useServiceTimeseries } from '@/lib/services/servicesQueries'
import { serviceHealth } from '@/lib/services/serviceHealth'
import { formatNumber } from '@/lib/core/format'

const props = defineProps({
  row: { type: Object, required: true },
  prevRow: { type: Object, default: null },
  startNs: { type: [String, Number], required: true },
  endNs: { type: [String, Number], required: true },
})
const emit = defineEmits(['open-service'])

const health = computed(() => serviceHealth(props.row))
const headline = computed(() => health.value.reasons[0] ?? '')
const isCrit = computed(() => health.value.status === 'critical')

// error-rate trend vs the previous window (the "▲NN%" hint).
const trend = computed(() => {
  const cur = props.row.error_rate ?? 0
  const prev = props.prevRow?.error_rate ?? 0
  if (!prev) return null
  return (cur - prev) / prev
})

const ts = useServiceTimeseries(() => props.row.service, () => props.startNs, () => props.endNs, 16)
const points = computed(() => (ts.data.value ?? []).map((b) => b.rate ?? 0))
const sparkColor = computed(() => (isCrit.value ? 'hsl(0 72% 51%)' : 'hsl(38 92% 50%)'))
</script>

<template>
  <button
    type="button"
    :data-service="row.service"
    class="flex flex-col rounded-xl border border-border bg-gradient-to-b to-transparent p-3 text-left transition-colors hover:border-foreground/40"
    :class="isCrit ? 'from-sev-error-soft' : 'from-sev-warn-soft'"
    @click="emit('open-service', row.service)"
  >
    <div class="flex items-center justify-between">
      <span class="font-medium text-foreground">{{ row.service }}</span>
      <HealthPill :status="health.status" />
    </div>
    <div class="mt-2 font-mono text-xs" :class="isCrit ? 'text-sev-error' : 'text-sev-warn'">
      {{ headline }}
      <span v-if="trend != null" class="text-muted-foreground">· {{ trend > 0 ? '▲' : '▼' }}{{ Math.abs(trend * 100).toFixed(0) }}%</span>
    </div>
    <div class="mt-2 flex items-center justify-between text-[11px] text-muted-foreground">
      <span class="font-mono tabular-nums">{{ formatNumber(row.count ?? 0) }} req</span>
      <Sparkline :points="points" :color="sparkColor" />
    </div>
  </button>
</template>

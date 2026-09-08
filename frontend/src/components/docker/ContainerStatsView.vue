<script setup lang="ts">
// Live container stats over the active Window: CPU, memory (against its limit), network in/out
// and block I/O. Read from the same `container.*` metrics the agent ships as OTLP, so what you
// see here is what an alert rule or the Attention Board would see.
import { computed } from 'vue'
import MiniAreaChart from '@/components/charts/MiniAreaChart.vue'
import { Spinner } from '@/components/ui/spinner'
import { useContainerStats } from '@/lib/deploy/deployQueries'
import { startNs, endNs } from '@/lib/core/context'
import { formatBytes, formatRate } from '@/lib/core/format'
import type { ContainerStatPoint } from '@/lib/core/api'

const props = defineProps<{ host: string; containerId: string }>()
const q = useContainerStats(() => props.host, () => props.containerId, startNs, endNs)
const stats = computed(() => q.data.value ?? null)

function last(points: ContainerStatPoint[] | undefined): number | null {
  if (!points) return null
  for (let i = points.length - 1; i >= 0; i--) if (points[i].v != null) return points[i].v
  return null
}
const pct = (v: number | null) => (v == null ? '—' : v.toFixed(1) + '%')
const bytes = (v: number | null) => (v == null ? '—' : formatBytes(v))
const rate = (v: number | null) => (v == null ? '—' : formatRate(v))

const CARDS = computed(() => {
  const s = stats.value
  if (!s) return []
  return [
    { key: 'cpu', label: 'CPU', value: pct(last(s.cpu_pct)), points: s.cpu_pct, fmt: (v: number) => v.toFixed(1) + '%' },
    { key: 'mem', label: 'Memory', value: bytes(last(s.mem_bytes)), sub: s.mem_limit_bytes ? `limit ${formatBytes(s.mem_limit_bytes)}` : 'no limit', points: s.mem_bytes, fmt: (v: number) => formatBytes(v) },
    { key: 'rx', label: 'Network in', value: rate(last(s.net_rx_bps)), points: s.net_rx_bps, fmt: (v: number) => formatRate(v) },
    { key: 'tx', label: 'Network out', value: rate(last(s.net_tx_bps)), points: s.net_tx_bps, fmt: (v: number) => formatRate(v) },
    { key: 'br', label: 'Block read', value: rate(last(s.block_read_bps)), points: s.block_read_bps, fmt: (v: number) => formatRate(v) },
    { key: 'bw', label: 'Block write', value: rate(last(s.block_write_bps)), points: s.block_write_bps, fmt: (v: number) => formatRate(v) },
  ]
})
</script>

<template>
  <div class="p-4" data-testid="container-stats">
    <div v-if="!stats" class="flex items-center justify-center py-10"><Spinner size="md">Loading stats…</Spinner></div>
    <div v-else class="grid gap-3 sm:grid-cols-2">
      <div v-for="c in CARDS" :key="c.key" class="rounded-xl border border-border bg-card p-3 shadow-1" :data-stat="c.key">
        <div class="flex items-baseline justify-between gap-2">
          <span class="text-xs font-medium text-foreground">{{ c.label }}</span>
          <span class="font-mono text-sm tabular-nums text-foreground">{{ c.value }}</span>
        </div>
        <p v-if="c.sub" class="text-[11px] text-muted-foreground">{{ c.sub }}</p>
        <div class="mt-2">
          <MiniAreaChart :points="c.points" :label="c.label" :height="56" :format-value="c.fmt" />
        </div>
      </div>
    </div>
    <p class="mt-3 text-[11px] text-muted-foreground">Over the active time window. The same <code class="font-mono">container.*</code> metrics feed alert rules and retention.</p>
  </div>
</template>

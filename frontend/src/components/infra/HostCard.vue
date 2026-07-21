<script setup lang="ts">
// One host in the /infra fleet card grid (replaces a HostTable row): name + a small "⚠ <RESOURCE>"
// flag naming the single worst-degraded resource, labeled CPU/MEM/DSK/GPU Meter rows (a null
// resource skips its row — GPU only when the host reports one), and a relative last-seen footer.
// Card idiom matches StatTile via the shared `ui/card` Card primitive (rounded-lg border bg-card
// p-4, hover lift); a warn/error border tint mirrors the flag's severity. No GPU device names —
// not part of this API (see HostStatTiles/HostResourcePanels on /infra/:host for those).
import { computed } from 'vue'
import { Card } from '@/components/ui/card'
import { Meter } from '@/components/ui/meter'
import { relative } from '@/lib/core/format'
import { formatPct, utilAccent } from '@/lib/infra/hostStats'
import type { InfraHost } from '@/lib/core/api'

const props = defineProps<{ host: InfraHost }>()
const emit = defineEmits<{ select: [host: string] }>()

interface Row {
  key: string
  label: string
  value: number
}

const rows = computed<Row[]>(() => {
  const h = props.host
  const list: Row[] = []
  if (h.cpuUtil != null) list.push({ key: 'cpu', label: 'CPU', value: h.cpuUtil })
  if (h.memUtil != null) list.push({ key: 'mem', label: 'MEM', value: h.memUtil })
  if (h.diskUtil != null) list.push({ key: 'disk', label: 'DSK', value: h.diskUtil })
  if (h.gpuUtil != null) list.push({ key: 'gpu', label: 'GPU', value: h.gpuUtil })
  return list
})

// Single worst-degraded row: highest severity first (error beats warning), then highest value.
const worst = computed<{ label: string; accent: 'error' | 'warning' } | null>(() => {
  let best: { label: string; value: number; accent: 'error' | 'warning' } | null = null
  for (const r of rows.value) {
    const accent = utilAccent(r.value)
    if (!accent) continue
    const rank = accent === 'error' ? 1 : 0
    const bestRank = best ? (best.accent === 'error' ? 1 : 0) : -1
    if (!best || rank > bestRank || (rank === bestRank && r.value > best.value)) {
      best = { label: r.label, value: r.value, accent }
    }
  }
  return best ? { label: best.label, accent: best.accent } : null
})

const borderClass = computed(() => {
  if (worst.value?.accent === 'error') return 'border-sev-error/40'
  if (worst.value?.accent === 'warning') return 'border-sev-warn/40'
  return ''
})

function select(): void {
  emit('select', props.host.host)
}
</script>

<template>
  <Card
    interactive
    role="button"
    tabindex="0"
    data-testid="infra-host-card"
    :data-host="host.host"
    :class="['flex cursor-pointer flex-col gap-3 p-4 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring', borderClass]"
    @click="select"
    @keydown.enter.prevent="select"
    @keydown.space.prevent="select"
  >
    <div class="flex items-center justify-between gap-2">
      <span class="truncate font-medium text-foreground">{{ host.host }}</span>
      <span
        v-if="worst"
        data-testid="host-card-flag"
        class="shrink-0 text-xs font-medium"
        :class="worst.accent === 'error' ? 'text-sev-error' : 'text-sev-warn'"
      >
        ⚠ {{ worst.label }}
      </span>
    </div>

    <div class="flex flex-col gap-2">
      <div v-for="r in rows" :key="r.key" class="flex items-center gap-3 text-xs">
        <span class="w-8 shrink-0 font-mono text-muted-foreground">{{ r.label }}</span>
        <Meter :value="r.value" :tone="utilAccent(r.value) ?? 'info'" class="flex-1" />
        <span class="w-10 shrink-0 text-right tabular-nums text-muted-foreground">{{ formatPct(r.value) }}</span>
      </div>
    </div>

    <div class="mt-auto border-t border-border pt-2 text-xs text-muted-foreground">
      {{ relative(BigInt(host.lastSeenNs)) }}
    </div>
  </Card>
</template>

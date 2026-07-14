<!-- frontend/src/components/metrics/MetricLegendTable.vue -->
<script setup>
import { ref, computed } from 'vue'
import { seriesColor, seriesLabelKey } from '@/lib/core/seriesColor'
import { formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  series: { type: Array, default: () => [] },
  unit: { type: String, default: '' },
  highlightKey: { type: String, default: null },
})
const emit = defineEmits(['highlight'])

function displayLabel(labels) {
  const ks = Object.keys(labels || {})
  return ks.length ? ks.map((k) => `${k} = ${labels[k]}`).join(', ') : '(all series)'
}
function stats(points) {
  const vs = points.map((p) => p.v).filter((v) => v != null)
  if (!vs.length) return { last: null, min: null, avg: null, max: null }
  const last = vs[vs.length - 1]
  const min = Math.min(...vs)
  const max = Math.max(...vs)
  const avg = vs.reduce((a, b) => a + b, 0) / vs.length
  return { last, min, avg, max }
}

const rows = computed(() =>
  props.series.map((s) => {
    const key = seriesLabelKey(s.labels)
    return { key, label: displayLabel(s.labels), swatch: seriesColor(key).swatch, ...stats(s.points) }
  }),
)
const worstMax = computed(() => Math.max(-Infinity, ...rows.value.map((r) => r.max ?? -Infinity)))

const sortKey = ref(null)
const sortDir = ref('desc')
function onSort(k) {
  if (sortKey.value === k) sortDir.value = sortDir.value === 'desc' ? 'asc' : 'desc'
  else { sortKey.value = k; sortDir.value = 'desc' }
}
const sortedRows = computed(() => {
  if (!sortKey.value) return rows.value
  const dir = sortDir.value === 'desc' ? -1 : 1
  return [...rows.value].sort((a, b) => ((a[sortKey.value] ?? 0) - (b[sortKey.value] ?? 0)) * dir)
})

const NUM_COLS = ['last', 'min', 'avg', 'max']
const COL_LABEL = { last: 'Last', min: 'Min', avg: 'Avg', max: 'Max' }
function fmt(v) { return v == null ? '—' : formatNumber(Math.round(v * 100) / 100) }
</script>

<template>
  <div class="mt-3 border-t border-border pt-1">
    <table class="w-full text-[12px]">
      <thead>
        <tr class="text-[10px] uppercase tracking-[0.08em] text-muted-foreground">
          <th class="px-2.5 py-1.5 text-left font-medium">Series</th>
          <th
            v-for="c in NUM_COLS" :key="c"
            :data-testid="'legend-sort-' + c"
            class="cursor-pointer select-none px-2.5 py-1.5 text-right font-medium transition-colors hover:text-foreground"
            @click="onSort(c)"
          >{{ COL_LABEL[c] }}<span v-if="sortKey === c" class="text-foreground"> {{ sortDir === 'desc' ? '↓' : '↑' }}</span></th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="r in sortedRows" :key="r.key" data-testid="legend-row"
          :class="cn('border-t border-border/60 transition-colors', highlightKey === r.key ? 'bg-muted' : 'hover:bg-muted/50')"
          @mouseenter="emit('highlight', r.key)" @mouseleave="emit('highlight', null)"
        >
          <td class="px-2.5 py-2 text-foreground">
            <span :class="cn('mr-2 inline-block size-[9px] rounded-full align-middle', r.swatch)" />{{ r.label }}
          </td>
          <td v-for="c in NUM_COLS" :key="c" class="px-2.5 py-2 text-right font-mono tabular-nums text-foreground/70">
            <span
              v-if="c === 'max' && r.max != null && r.max === worstMax"
              data-testid="legend-max-worst" class="font-semibold text-sev-error"
            >{{ fmt(r.max) }}</span>
            <template v-else>{{ fmt(r[c]) }}</template>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<!-- frontend/src/components/metrics/MetricCatalog.vue -->
<script setup>
import { ref, computed } from 'vue'
import { Search } from 'lucide-vue-next'
import { formatNumber } from '@/lib/core/format'
import { seriesColor } from '@/lib/core/seriesColor'

const props = defineProps({
  entries: { type: Array, default: () => [] },
  loading: { type: Boolean, default: false },
})
const emit = defineEmits(['open'])

const search = ref('')
const typeFilter = ref('')
const TYPES = ['gauge', 'sum', 'histogram', 'exp_histogram', 'summary']

const filtered = computed(() => {
  const q = search.value.trim().toLowerCase()
  return props.entries.filter((e) => {
    if (typeFilter.value && e.type !== typeFilter.value) return false
    if (!q) return true
    return e.name.toLowerCase().includes(q) || (e.unit || '').toLowerCase().includes(q) || e.type.includes(q)
  })
})
// Phase 3: no per-metric preview series in the catalog response — draw a muted baseline sparkline.
const SPARK = '0,12 14,9 28,11 42,7 56,10 70,8'
</script>

<template>
  <div class="overflow-hidden rounded-xl border border-border bg-card">
    <div class="flex items-center gap-2 border-b border-border p-2.5">
      <div class="flex h-8 flex-1 items-center gap-2 rounded-lg border border-border px-2.5 focus-within:border-foreground/40">
        <Search class="size-3.5 text-muted-foreground" />
        <input
          v-model="search" data-testid="catalog-search" placeholder="filter metrics by name / unit / type…"
          class="w-full bg-transparent text-[13px] outline-none placeholder:text-muted-foreground"
        />
      </div>
      <select v-model="typeFilter" class="h-8 rounded-lg border border-border bg-background px-2 font-mono text-[12px]">
        <option value="">type: all</option>
        <option v-for="t in TYPES" :key="t" :value="t">{{ t }}</option>
      </select>
    </div>
    <table class="w-full text-[13px]">
      <thead>
        <tr class="text-[10px] uppercase tracking-[0.08em] text-muted-foreground">
          <th class="px-3 py-2 text-left font-medium">Metric</th>
          <th class="px-3 py-2 text-left font-medium">Type</th>
          <th class="px-3 py-2 text-left font-medium">Unit</th>
          <th class="px-3 py-2 text-right font-medium">Series</th>
          <th class="px-3 py-2 text-left font-medium">Preview</th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="e in filtered" :key="e.name" data-testid="catalog-row"
          class="cursor-pointer border-t border-border/60 transition-colors hover:bg-muted/60" @click="emit('open', e.name)"
        >
          <td class="px-3 py-2 font-mono text-foreground">{{ e.name }}</td>
          <td class="px-3 py-2">
            <span class="inline-flex rounded border border-border/70 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground">{{ e.type }}</span>
          </td>
          <td class="px-3 py-2 font-mono text-muted-foreground">{{ e.unit || '—' }}</td>
          <td class="px-3 py-2 text-right font-mono tabular-nums text-muted-foreground">{{ formatNumber(e.series_count) }}</td>
          <td class="px-3 py-2">
            <svg width="70" height="16" aria-hidden="true"><polyline :points="SPARK" fill="none" :stroke="seriesColor(e.name).stroke" stroke-opacity="0.55" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
          </td>
        </tr>
        <tr v-if="!filtered.length"><td colspan="5" class="px-3 py-8 text-center text-[12px] text-muted-foreground">{{ loading ? 'Loading…' : 'No metrics' }}</td></tr>
      </tbody>
    </table>
  </div>
</template>

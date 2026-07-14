<!-- frontend/src/components/metrics/MetricQueryRow.vue -->
<script setup>
import { computed } from 'vue'
import { ChevronDown, Plus, Sigma, Code2, Filter } from 'lucide-vue-next'
import MetricPicker from './MetricPicker.vue'
import MetricPresets from './MetricPresets.vue'
import MetricVizSwitcher from './MetricVizSwitcher.vue'
import SearchBar from '@/components/common/SearchBar.vue'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import {
  DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem,
} from '@/components/ui/dropdown-menu'
import { AGG_OPTIONS, aggOptionsForType, buildMetricCatalog, groupByDisabled, METRIC_EXAMPLE_QUERIES } from '@/lib/metrics/metricFields'
import { ALL_VIZ, availableViz } from '@/lib/metrics/metricViz'

const props = defineProps({
  metric: { type: String, default: '' },
  catalog: { type: Array, default: () => [] },
  agg: { type: [String, null], default: null },
  defaultAgg: { type: String, default: 'avg' },
  groupBy: { type: Array, default: () => [] },
  filter: { type: String, default: '' },
  filterError: { type: Object, default: null },
  metricType: { type: String, default: '' },
  isMonotonic: { type: [Boolean, null], default: null },
  attributeKeys: { type: Array, default: () => [] },
  services: { type: Array, default: () => [] },
  catalogLoading: { type: Boolean, default: false },
  viz: { type: String, default: 'line' },
  seriesCount: { type: Number, default: 0 },
  favorites: { type: Array, default: () => [] },
  recent: { type: Array, default: () => [] },
})
const emit = defineEmits(['update:metric', 'update:agg', 'update:groupBy', 'update:filter', 'update:viz', 'toggle-favorite'])

const effectiveAgg = computed(() => props.agg ?? props.defaultAgg)
const aggChoices = computed(() => aggOptionsForType(props.metricType, props.isMonotonic))
const groupByValue = computed({
  get: () => props.groupBy[0] ?? '',
  set: (v) => emit('update:groupBy', v ? [v] : []),
})
const filterCatalog = computed(() => buildMetricCatalog(props.attributeKeys, props.services))
const groupDisabled = computed(() => groupByDisabled(props.metricType))
const availableVizIds = computed(() => availableViz({ type: props.metricType, seriesCount: props.seriesCount }))
</script>

<template>
  <div class="rounded-xl border border-border bg-card p-3">
    <div class="flex flex-wrap items-center gap-2">
      <span data-testid="query-badge" class="flex size-[22px] items-center justify-center rounded-md bg-foreground font-mono text-[12px] font-semibold text-background">A</span>

      <MetricPicker
        :model-value="metric" :catalog="catalog" :loading="catalogLoading"
        :favorites="favorites" :recent="recent"
        @update:model-value="emit('update:metric', $event)"
        @toggle-favorite="emit('toggle-favorite', $event)"
      />

      <!-- aggregation -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <button type="button" data-testid="agg-trigger" class="flex h-[34px] items-center gap-1.5 rounded-lg border border-border px-2.5 text-[13px] transition-colors hover:border-foreground/40">
            <span class="text-[11px] text-muted-foreground">agg</span>
            <span class="font-mono text-foreground">{{ effectiveAgg }}</span>
            <span v-if="agg == null" data-testid="agg-auto-badge" class="rounded border border-border bg-muted px-1 py-0.5 font-mono text-[9px] font-medium uppercase tracking-[0.08em] text-muted-foreground">auto</span>
            <ChevronDown class="size-3.5 text-muted-foreground" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuItem data-testid="agg-option-auto" @select="emit('update:agg', null)">Auto ({{ AGG_OPTIONS[defaultAgg] || defaultAgg }})</DropdownMenuItem>
          <DropdownMenuItem v-for="a in aggChoices" :key="a" :data-testid="'agg-option-' + a" @select="emit('update:agg', a)">{{ AGG_OPTIONS[a] || a }}</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <!-- group by (native select for testability; disabled for summary — quantiles aren't re-aggregatable) -->
      <label
        class="flex h-[34px] items-center gap-1.5 rounded-lg border border-border px-2.5 text-[13px]"
        :class="groupDisabled ? 'opacity-50' : ''"
        :title="groupDisabled ? 'Summary quantiles can’t be re-aggregated across series' : ''"
        data-testid="groupby-trigger"
      >
        <span class="text-[11px] text-muted-foreground">group by</span>
        <select v-model="groupByValue" :disabled="groupDisabled" class="bg-transparent font-mono text-foreground outline-none">
          <option value="">none</option>
          <option v-for="k in attributeKeys" :key="k" :value="k" :data-testid="'groupby-option-' + k">{{ k }}</option>
        </select>
      </label>

      <!-- grammar filter -->
      <div class="flex min-w-[180px] flex-1 items-center gap-2">
        <Filter class="size-3.5 shrink-0 text-muted-foreground" />
        <SearchBar
          class="flex-1"
          :model-value="filter" :error="filterError" :services="services"
          :catalog="filterCatalog" :example-queries="METRIC_EXAMPLE_QUERIES"
          placeholder="filter…" @update:model-value="emit('update:filter', $event)"
        />
      </div>
    </div>

    <MetricPresets
      v-if="metric"
      class="mt-2.5"
      :metric-type="metricType"
      :is-monotonic="isMonotonic"
      :current-agg="agg"
      @apply="emit('update:agg', $event.agg)"
    />

    <!-- folded footer: Phase-6 power tools, disabled placeholders -->
    <div class="mt-2.5 flex items-center gap-1.5 border-t border-dashed border-border pt-2.5">
      <Tooltip>
        <TooltipTrigger as-child>
          <button type="button" data-testid="add-query" disabled class="flex items-center gap-1 rounded px-2 py-1 text-[12px] text-muted-foreground opacity-50"><Plus class="size-3.5" /> Add query</button>
        </TooltipTrigger>
        <TooltipContent>Coming soon</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger as-child>
          <button type="button" data-testid="add-formula" disabled class="flex items-center gap-1 rounded px-2 py-1 text-[12px] text-muted-foreground opacity-50"><Sigma class="size-3.5" /> Formula</button>
        </TooltipTrigger>
        <TooltipContent>Coming soon</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger as-child>
          <button type="button" data-testid="raw-sql" disabled class="flex items-center gap-1 rounded px-2 py-1 text-[12px] text-muted-foreground opacity-50"><Code2 class="size-3.5" /> Raw SQL</button>
        </TooltipTrigger>
        <TooltipContent>Coming soon</TooltipContent>
      </Tooltip>
      <MetricVizSwitcher
        class="ml-auto"
        :model-value="viz"
        :available="availableVizIds"
        :all-viz="ALL_VIZ"
        @update:model-value="emit('update:viz', $event)"
      />
    </div>
  </div>
</template>

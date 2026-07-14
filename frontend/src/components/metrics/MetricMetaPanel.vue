<!-- frontend/src/components/metrics/MetricMetaPanel.vue -->
<script setup>
import { computed } from 'vue'
import { Waypoints } from 'lucide-vue-next'
import { formatNumber, relative } from '@/lib/core/format'

const props = defineProps({
  metadata: { type: Object, default: null },
  loading: { type: Boolean, default: false },
})
const emit = defineEmits(['view-exemplars'])

// `relative` takes epoch NANOSECONDS (it does its own ns→ms conversion via
// BigInt division internally) — pass the decimal-ns wire string straight through
// as a BigInt, don't pre-convert to ms ourselves.
const lastSeen = computed(() => {
  if (!props.metadata?.last_seen) return '—'
  return relative(BigInt(props.metadata.last_seen)) // e.g. "4s ago"
})
const cardinality = computed(() => (props.metadata ? formatNumber(props.metadata.series_count) : '—'))
</script>

<template>
  <div class="rounded-xl border border-border bg-muted/40 p-3.5">
    <div v-if="!metadata" data-testid="meta-empty" class="py-8 text-center text-[12px] text-muted-foreground">
      {{ loading ? 'Loading…' : 'Select a metric' }}
    </div>
    <template v-else>
      <div class="break-words font-mono text-[13px] font-semibold text-foreground">{{ metadata.name }}</div>
      <div class="mt-2 flex flex-wrap gap-1.5">
        <span class="rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-[0.08em] text-muted-foreground">{{ metadata.type }}</span>
        <span v-if="metadata.temporality" class="rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-[0.08em] text-muted-foreground">{{ metadata.temporality }}</span>
      </div>
      <dl class="mt-3.5">
        <div class="flex items-center justify-between border-t border-border/60 py-1.5 text-[12px]">
          <dt class="text-muted-foreground">Unit</dt><dd class="font-mono text-foreground">{{ metadata.unit || '—' }}</dd>
        </div>
        <div class="flex items-center justify-between border-t border-border/60 py-1.5 text-[12px]">
          <dt class="text-muted-foreground">Series</dt><dd class="font-mono tabular-nums text-foreground">{{ cardinality }}</dd>
        </div>
        <div class="flex items-center justify-between border-t border-border/60 py-1.5 text-[12px]">
          <dt class="text-muted-foreground">Last received</dt><dd class="font-mono text-foreground">{{ lastSeen }}</dd>
        </div>
      </dl>
      <div class="mt-3.5">
        <div class="mb-2 text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">Attributes</div>
        <div class="flex flex-wrap gap-1.5">
          <span
            v-for="k in metadata.attribute_keys" :key="k" data-testid="attr-chip"
            class="rounded-full border border-border bg-muted px-2 py-0.5 font-mono text-[10.5px] text-muted-foreground"
          >{{ k }}</span>
        </div>
      </div>
      <button
        type="button" data-testid="view-exemplars"
        class="mt-4 flex h-[34px] w-full items-center justify-center gap-2 rounded-lg bg-foreground text-[13px] font-medium text-background transition-colors hover:bg-foreground/90"
        @click="emit('view-exemplars')"
      >
        <Waypoints class="size-3.5" /> View exemplar traces
      </button>
      <p class="mt-2 text-[11px] leading-relaxed text-muted-foreground">
        ◆ diamonds on the chart mark exemplars — jump to the trace behind a spike.
      </p>
    </template>
  </div>
</template>

<script setup>
// The Services pages' Attention Level pill. Same five levels, same StatusPill skin the Attention
// Board uses, so `payments-api` reads identically on `/home` and on `/services` — that was the
// whole point of collapsing the two vocabularies into one.
//
// `show-label: false` collapses it to the bare dot for dense table cells; the level still reaches
// the DOM as `data-level` so a test (or a stylesheet) can find it.
import { computed } from 'vue'
import { StatusPill } from '@/components/ui/status-pill'
import { STATUS_META } from '@/lib/services/serviceHealth'

const props = defineProps({
  level: { type: String, default: 'unknown' }, // 'down' | 'critical' | 'warning' | 'ok' | 'unknown'
  showLabel: { type: Boolean, default: true },
})
const meta = computed(() => STATUS_META[props.level] ?? STATUS_META.unknown)
</script>

<template>
  <StatusPill
    v-if="showLabel"
    :tone="meta.tone"
    :solid="meta.solid"
    :data-level="level"
    class="text-[10px] font-bold uppercase tracking-wide"
  >
    {{ meta.label }}
  </StatusPill>
  <span
    v-else
    :data-level="level"
    :title="meta.label"
    :class="['inline-block size-2 shrink-0 rounded-full', meta.dot]"
  />
</template>

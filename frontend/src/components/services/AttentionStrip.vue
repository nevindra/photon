<script setup lang="ts">
// The "needs attention" strip: every subject whose level is not OK, worst first, capped.
//
// The overflow chip is not decoration. Capping without saying so is how a page claims a fleet has
// eight problems when it has thirty — the chip names the remainder and hands the reader to the
// table, which lists all of them.
import { computed } from 'vue'
import AttentionCard from './AttentionCard.vue'
import { attentionRows } from '@/lib/services/servicesOverview'
import type { FleetRow } from '@/lib/services/servicesOverview'

const props = withDefaults(defineProps<{ rows?: FleetRow[]; max?: number }>(), { max: 8 })
const emit = defineEmits<{ open: [row: FleetRow]; overflow: [] }>()

const cards = computed(() => attentionRows(props.rows ?? [], props.max))
const total = computed(() => (props.rows ?? []).filter((r) => r.level !== 'ok' && r.level !== 'unknown').length)
const hidden = computed(() => Math.max(0, total.value - cards.value.length))
</script>

<template>
  <section v-if="cards.length" data-testid="attention-strip" aria-label="Needs attention">
    <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
      <AttentionCard v-for="row in cards" :key="row.id" :row="row" @open="emit('open', $event)" />
    </div>
    <button
      v-if="hidden"
      type="button"
      data-testid="attention-overflow"
      class="mt-2 rounded-full border border-border px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:border-foreground/30 hover:text-foreground"
      @click="emit('overflow')"
    >
      +{{ hidden }} more in the table ↓
    </button>
  </section>
</template>

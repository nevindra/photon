<script setup>
import { computed } from 'vue'
import AttentionCard from './AttentionCard.vue'
import { attentionServices } from '@/lib/services/serviceHealth'

const props = defineProps({
  rows: { type: Array, default: () => [] },
  prevRows: { type: Array, default: () => [] },
  startNs: { type: [String, Number], required: true },
  endNs: { type: [String, Number], required: true },
  max: { type: Number, default: 3 },
})
const emit = defineEmits(['open-service'])

const cards = computed(() => attentionServices(props.rows, props.max))
const prevByService = computed(() => Object.fromEntries(props.prevRows.map((r) => [r.service, r])))
</script>

<template>
  <div v-if="cards.length" class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
    <AttentionCard
      v-for="row in cards"
      :key="row.service"
      :row="row"
      :prev-row="prevByService[row.service] ?? null"
      :start-ns="startNs"
      :end-ns="endNs"
      @open-service="emit('open-service', $event)"
    />
  </div>
</template>

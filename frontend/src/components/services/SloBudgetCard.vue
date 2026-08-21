<script setup lang="ts">
// One objective's card, with its own budget read.
//
// It exists as its own component only because a composable cannot be called inside a `v-for`: the
// panel renders one of these per objective, and each owns its query so the first card can paint
// while a slower one is still loading. Its whole job is fetch-then-hand-to-`SloCard`.
import { computed } from 'vue'
import SloCard from './SloCard.vue'
import { useSloBudget } from '@/lib/services/servicesQueries'

const props = defineProps<{ service: string; sloId: string }>()

const budgetQuery = useSloBudget(
  () => props.service,
  () => props.sloId,
)
const budget = computed(() => budgetQuery.data.value ?? null)
</script>

<template>
  <SloCard v-if="budget" :budget="budget" />
  <!-- A skeleton, not an empty slot: a card appearing late in a grid shifts everything after it. -->
  <div
    v-else
    data-testid="slo-card-loading"
    class="h-32 animate-pulse rounded-xl border border-border bg-card"
  />
</template>

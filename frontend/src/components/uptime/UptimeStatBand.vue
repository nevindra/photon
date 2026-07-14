<script setup>
import { computed } from 'vue'
import { StatTile } from '@/components/ui/stat-tile'

const props = defineProps({ monitors: { type: Array, default: () => [] } })

// paused = disabled monitors (regardless of last_state); up/down only count enabled ones.
const up = computed(() => props.monitors.filter((m) => m.enabled && m.last_state === 'up').length)
const down = computed(() => props.monitors.filter((m) => m.enabled && m.last_state === 'down').length)
const paused = computed(() => props.monitors.filter((m) => !m.enabled).length)

// Note: the original per-tile "dot" next to the label always mirrored the stripe color
// (never a distinct tone), so it's now fully represented by StatTile's `accent` stripe.
const tiles = computed(() => [
  { key: 'total', n: props.monitors.length, label: 'Monitors' },
  { key: 'up', n: up.value, label: 'Up', accent: 'success' },
  { key: 'down', n: down.value, label: 'Down', accent: 'error' },
  { key: 'paused', n: paused.value, label: 'Paused', accent: 'neutral' },
])
</script>

<template>
  <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
    <StatTile v-for="t in tiles" :key="t.key" :label="t.label" :value="t.n" :accent="t.accent" />
  </div>
</template>

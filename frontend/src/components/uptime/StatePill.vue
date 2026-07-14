<script setup>
import { computed } from 'vue'
import { StatusPill } from '@/components/ui/status-pill'

// Uptime-monitor state pill. This is a distinct domain from log severity
// (SeverityTag.vue / lib/format.js's tone map) so it maps its own state→tone
// table onto the shared StatusPill primitive.
const props = defineProps({
  state: { type: String, required: true },
  paused: { type: Boolean, default: false },
})

const label = computed(() =>
  props.paused ? 'Paused' : props.state === 'up' ? 'Up' : props.state === 'down' ? 'Down' : 'Pending',
)

const TONE = {
  paused: 'neutral',
  up: 'success',
  down: 'error',
  pending: 'warning',
}

const tone = computed(() => (props.paused ? TONE.paused : TONE[props.state] ?? TONE.pending))
</script>

<template>
  <StatusPill :tone="tone">{{ label }}</StatusPill>
</template>

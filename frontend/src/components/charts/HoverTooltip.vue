<script setup>
// Pointer-following tooltip for LINE charts (one instance per chart — cheap; avoids N per-point
// Reka tooltips). The consumer owns hover state and sets x/y + content on mousemove. Renders the
// shared ChartTooltipCard so its look matches the anchored histogram tooltips (ChartBarTooltip):
// `subtitle` becomes the card's muted header, `title` its prominent line.
import ChartTooltipCard from './ChartTooltipCard.vue'

defineProps({
  visible: { type: Boolean, default: false },
  x: { type: Number, default: 0 },
  y: { type: Number, default: 0 },
  // The prominent value line (e.g. "123 ms") — shown as the card's `total`.
  title: { type: String, default: '' },
  // The muted context line (e.g. a timestamp) — shown as the card's header.
  subtitle: { type: String, default: '' },
  // Optional breakdown rows, forwarded verbatim to ChartTooltipCard.
  rows: { type: Array, default: () => [] },
})
</script>

<template>
  <Teleport to="body">
    <div
      v-if="visible"
      role="tooltip"
      class="pointer-events-none fixed z-50 -translate-x-1/2 -translate-y-[calc(100%+10px)]"
      :style="{ left: x + 'px', top: y + 'px' }"
    >
      <ChartTooltipCard :title="subtitle" :total="title" :rows="rows" />
    </div>
  </Teleport>
</template>

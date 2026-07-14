<script setup>
// Stacked good/needs/poor distribution bar for a single Web Vital (modeled on
// `ui/meter/Meter.vue`'s track/fill chrome). Tone classes are literal Tailwind strings
// (never interpolated) so the content scanner keeps them: good=bg-success,
// needs=bg-sev-warn, poor=bg-sev-error.
const props = defineProps({ good: Number, needs: Number, poor: Number })
const total = () => Math.max(1, (props.good || 0) + (props.needs || 0) + (props.poor || 0))
const pct = (n) => ((n || 0) / total()) * 100 + '%'
</script>

<template>
  <div class="flex h-1.5 w-full gap-0.5 overflow-hidden rounded-full bg-muted" role="meter">
    <div class="bg-success" data-testid="dist-good" :style="{ width: pct(good) }" />
    <div class="bg-sev-warn" data-testid="dist-needs" :style="{ width: pct(needs) }" />
    <div class="bg-sev-error" data-testid="dist-poor" :style="{ width: pct(poor) }" />
  </div>
</template>

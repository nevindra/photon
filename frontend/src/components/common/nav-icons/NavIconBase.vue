<script setup lang="ts">
// The frame every nav icon is drawn in. Two colours, and only two: the NEUTRAL parts inherit
// `currentColor` (the rail's muted grey when idle, the foreground ink when active) and the
// ACCENT parts read `--ni-accent`, which NavRail sets per item — Photon Cyan, softened while the
// item is idle and at full strength on hover / active. An icon used outside the rail falls back
// to the plain brand token, so the two tones survive anywhere it is mounted.
</script>

<template>
  <svg
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.75"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <slot />
  </svg>
</template>

<style>
.ni-accent {
  stroke: var(--ni-accent, hsl(var(--brand)));
}
.ni-accent-fill {
  fill: var(--ni-accent, hsl(var(--brand)));
  stroke: var(--ni-accent, hsl(var(--brand)));
}
.ni-fill {
  fill: currentColor;
  stroke: none;
}
</style>

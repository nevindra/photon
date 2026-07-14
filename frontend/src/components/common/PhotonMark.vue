<script setup>
defineProps({ size: { type: Number, default: 24 } })
</script>

<template>
  <div
    class="relative inline-flex shrink-0 items-center justify-center overflow-hidden rounded-md bg-primary"
    :style="{ width: `${size}px`, height: `${size}px` }"
    aria-hidden="true"
  >
    <div
      class="rounded-full"
      style="
        width: 58%;
        height: 58%;
        background: conic-gradient(
          from 220deg,
          hsl(0 0% 100% / 0.95),
          hsl(0 0% 100% / 0.25),
          hsl(0 0% 100% / 0.05),
          hsl(0 0% 100% / 0.55),
          hsl(0 0% 100% / 0.95)
        );
      "
    />
  </div>
</template>

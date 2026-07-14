<script setup>
// Shared tooltip card body for every chart (histograms + line charts). Presentational only — the
// CONTAINER owns positioning (an anchored Reka tooltip for bar charts via ChartBarTooltip; a
// pointer-following wrapper for line charts), this owns the canonical LOOK: a light bordered
// popover surface with a muted header line, an optional prominent total, and optional breakdown
// rows (colour swatch · label · right-aligned value). One card so every chart tooltip reads alike.
defineProps({
  // Header line — a time or range, e.g. "14:22:07 – 14:22:37". Muted, small.
  title: { type: String, default: '' },
  // Prominent summary line under the header, e.g. "142 spans". Optional.
  total: { type: String, default: '' },
  // Breakdown rows: { key, label, value, swatchClass?, swatchColor? }. `swatchClass` is a Tailwind
  // background class (histogram severity/status colours); `swatchColor` is a raw CSS colour (line
  // series strokes). `value` is a pre-formatted string, right-aligned.
  rows: { type: Array, default: () => [] },
})
</script>

<template>
  <div class="rounded-lg border border-border bg-popover px-2.5 py-2 font-mono text-popover-foreground shadow-lg">
    <div v-if="title" class="whitespace-nowrap text-[10px] text-muted-foreground">{{ title }}</div>
    <div
      v-if="total"
      class="mt-0.5 whitespace-nowrap text-[11px] font-medium tabular-nums text-foreground"
    >
      {{ total }}
    </div>
    <div
      v-for="r in rows"
      :key="r.key"
      class="mt-1 flex items-center gap-2 whitespace-nowrap text-[11px]"
    >
      <span
        class="inline-block size-2 shrink-0 rounded-sm"
        :class="r.swatchClass"
        :style="r.swatchColor ? { background: r.swatchColor } : null"
      />
      <span class="text-foreground/80">{{ r.label }}</span>
      <span class="ml-auto pl-4 font-semibold tabular-nums text-foreground">{{ r.value }}</span>
    </div>
  </div>
</template>

<script setup>
// A lean titled chart card: a header row (title + optional subtitle on the left, an optional
// right-aligned `summary` slot for a headline stat/legend) above a body slot holding the chart.
// Tighter padding than ui/card, sized for the DOM-bar histograms.
defineProps({
  title: { type: String, required: true },
  subtitle: { type: String, default: '' },
})
</script>

<template>
  <section class="rounded-xl border border-border bg-card">
    <header class="flex items-start justify-between gap-3 px-4 pb-2 pt-3.5">
      <div class="min-w-0">
        <h3 class="truncate text-xs font-medium uppercase tracking-wider text-muted-foreground">
          {{ title }}
        </h3>
        <p v-if="subtitle" class="mt-0.5 truncate text-[11px] text-muted-foreground/70">
          {{ subtitle }}
        </p>
      </div>
      <div v-if="$slots.summary" data-test="chart-panel-summary" class="shrink-0 text-right">
        <slot name="summary" />
      </div>
    </header>
    <div class="px-4 pb-4 pt-1">
      <slot />
    </div>
  </section>
</template>

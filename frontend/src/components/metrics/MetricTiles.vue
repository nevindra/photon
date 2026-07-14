<script setup>
import { ArrowUp, ArrowDown, Minus } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

// KPI tiles. Each tile: a label, a preformatted value string, and an optional signed `delta`
// (fraction vs the previous equal-length window). `tone: 'up-bad'` means a RISING value is bad
// (error rate, latency) → up=red / down=green; `'up-good'` is the inverse (up=green / down=red, e.g.
// Apdex); `'neutral'` shows the delta muted (rate, volume). `comparisonLabel` (optional) prints a
// muted "vs prev …" line under the value.
defineProps({
  tiles: { type: Array, default: () => [] },
  comparisonLabel: { type: String, default: '' },
})

// A fraction like 0.123 → "12%". Rounds to whole percent; tiny non-zero deltas floor at "0%".
function fmtDelta(delta) {
  return Math.round(Math.abs(delta) * 100) + '%'
}

// Foreground colour for the delta chip.
// up-bad: up=red / down=green (error rate, latency). up-good: up=green / down=red (apdex).
function deltaClass(delta, tone) {
  if (delta === 0) return 'text-muted-foreground'
  if (tone === 'up-bad') return delta > 0 ? 'text-sev-error' : 'text-green-600 dark:text-green-500'
  if (tone === 'up-good') return delta > 0 ? 'text-green-600 dark:text-green-500' : 'text-sev-error'
  return 'text-muted-foreground'
}

// Faint tinted background matching the tone. Literal strings only.
function deltaBgClass(delta, tone) {
  if (delta === 0) return 'bg-muted'
  if (tone === 'up-bad') return delta > 0 ? 'bg-sev-error-soft' : 'bg-green-500/10'
  if (tone === 'up-good') return delta > 0 ? 'bg-green-500/10' : 'bg-sev-error-soft'
  return 'bg-muted'
}
</script>

<template>
  <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
    <div
      v-for="tile in tiles"
      :key="tile.label"
      data-testid="metric-tile"
      class="flex flex-col gap-1.5 rounded-xl border border-border bg-card px-4 py-3.5"
    >
      <span class="text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
        {{ tile.label }}
      </span>
      <div class="flex items-baseline gap-2">
        <span class="font-mono text-2xl font-semibold tabular-nums text-foreground">
          {{ tile.value }}
        </span>
        <span
          v-if="tile.delta != null"
          data-testid="metric-delta"
          :class="
            cn(
              'inline-flex items-center gap-0.5 rounded-full px-1.5 py-0.5 font-mono text-xs tabular-nums',
              deltaClass(tile.delta, tile.tone),
              deltaBgClass(tile.delta, tile.tone),
            )
          "
        >
          <component :is="tile.delta > 0 ? ArrowUp : tile.delta < 0 ? ArrowDown : Minus" class="size-3" />
          {{ fmtDelta(tile.delta) }}
        </span>
      </div>
      <span v-if="comparisonLabel" class="text-[10px] text-muted-foreground/70">{{ comparisonLabel }}</span>
    </div>
  </div>
</template>

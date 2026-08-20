<script setup lang="ts">
// The KPI card. Layout, top to bottom: a header row (optional `icon` in a soft rounded
// tile, the label, an optional `action` on the right — a menu button, a link), the headline
// value with its `secondary`/`delta` reading beside it, an optional `sub` context line, and
// an optional `footer` slot on its own hairline-separated strip (counts, a mini bar, a CTA).
// The label reads as the card's title (foreground, not muted) — the number below it is the
// data, so the label doesn't have to fight for attention by being loud.
import type { HTMLAttributes } from 'vue'
import { computed } from 'vue'
import { ArrowUp, ArrowDown, Minus } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  label: string
  value: string | number
  mono?: boolean
  accent?: 'success' | 'error' | 'warning' | 'info' | 'neutral'
  delta?: number | string
  deltaDirection?: 'up' | 'down' | 'flat'
  /// A second, co-equal reading of the same resource rendered beside `value` at a smaller size —
  /// e.g. the across-device mean next to a worst-device max. Use it when the headline alone is
  /// ambiguous, not for units or context (that's `sub`). Mutually exclusive with `delta` in
  /// practice: both occupy the slot to the right of the value.
  secondary?: string
  sub?: string
  class?: HTMLAttributes['class']
}>()

// `accent` tints the icon tile and paints a short rail down the left edge. Colour is
// spent only when the tile is actually saying something (warning / critical / healthy).
const TONE = {
  success: { rail: 'bg-success', tile: 'bg-success-soft text-success' },
  error: { rail: 'bg-sev-error', tile: 'bg-sev-error-soft text-sev-error' },
  warning: { rail: 'bg-sev-warn', tile: 'bg-sev-warn-soft text-sev-warn' },
  info: { rail: 'bg-primary', tile: 'bg-brand-soft text-brand' },
  neutral: { rail: 'bg-muted-foreground', tile: 'bg-muted text-muted-foreground' },
}

const DeltaIcon = computed(() => {
  if (props.deltaDirection === 'up') return ArrowUp
  if (props.deltaDirection === 'down') return ArrowDown
  return Minus
})

const deltaColorClass = computed(() => {
  if (props.deltaDirection === 'up') return 'text-success'
  if (props.deltaDirection === 'down') return 'text-sev-error'
  return 'text-muted-foreground'
})

const tileToneClass = computed(() =>
  props.accent ? TONE[props.accent].tile : 'bg-muted text-muted-foreground',
)
</script>

<template>
  <div
    :class="
      cn(
        'relative overflow-hidden rounded-xl border border-border bg-card shadow-1 transition-[border-color] duration-150 ease-out',
        props.class,
      )
    "
  >
    <div
      v-if="props.accent"
      class="absolute inset-y-3 left-0 w-[3px] rounded-r-full"
      :class="TONE[props.accent].rail"
    />
    <div class="p-4">
      <div class="flex items-center gap-2">
        <span
          v-if="$slots.icon"
          :class="cn('flex size-7 shrink-0 items-center justify-center rounded-lg [&_svg]:size-4', tileToneClass)"
        >
          <slot name="icon" />
        </span>
        <p data-slot="label" class="min-w-0 flex-1 truncate text-[13px] font-medium text-foreground">{{ props.label }}</p>
        <div v-if="$slots.action" class="-mr-1 shrink-0 text-muted-foreground"><slot name="action" /></div>
      </div>

      <div class="mt-2.5 flex items-end justify-between gap-2">
        <div class="min-w-0 space-y-1">
          <div class="flex items-baseline gap-2">
            <p data-slot="value" :class="cn('text-[26px] font-semibold leading-none tracking-tight tabular-nums text-foreground', props.mono && 'font-mono')">
              {{ props.value }}
            </p>
            <span v-if="props.secondary" class="text-xs font-medium tabular-nums text-muted-foreground">
              {{ props.secondary }}
            </span>
            <div v-if="props.delta !== undefined" class="flex items-center gap-0.5">
              <component :is="DeltaIcon" :class="cn('size-3.5', deltaColorClass)" />
              <span :class="cn('text-xs font-medium', deltaColorClass)">{{ props.delta }}</span>
            </div>
          </div>
          <p v-if="props.sub" class="truncate text-[11px] text-muted-foreground">{{ props.sub }}</p>
        </div>
        <div v-if="$slots.spark" class="shrink-0 pb-0.5 text-primary/70"><slot name="spark" /></div>
      </div>
    </div>

    <div
      v-if="$slots.footer"
      class="flex items-center gap-2 border-t border-border px-4 py-2.5 text-[11px] text-muted-foreground"
    >
      <slot name="footer" />
    </div>
  </div>
</template>

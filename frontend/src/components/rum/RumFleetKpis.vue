<script setup lang="ts">
// The fleet KPI strip at the top of the RUM executive summary (`/rum`). Chrome mirrors
// `ui/stat-tile/StatTile.vue` (rounded-lg border bg-card shadow-1 + a left accent bar) so it reads
// as the same system, but each tile carries richer sub-content than StatTile exposes: an optional
// coloured delta, a free-text sub-line, or a good/needs/poor mini distribution bar. Tiles with a
// `to` become buttons that emit `navigate` (the view routes them through `correlate()`).
import VitalsDistributionBar from './VitalsDistributionBar.vue'
import { cn } from '@/lib/core/utils'

type Accent = 'success' | 'error' | 'warning' | 'info' | 'neutral'
type Tone = 'good' | 'needs' | 'poor'

export interface Kpi {
  key: string
  label: string
  value: string
  accent: Accent
  valueTone?: Tone | null
  delta?: { text: string; tone: 'good' | 'bad' | 'neutral' }
  sub?: string
  dist?: { good: number; needs: number; poor: number }
  to?: string
}

defineProps<{ kpis: Kpi[] }>()
const emit = defineEmits<{ navigate: [to: string] }>()

const ACCENT: Record<Accent, string> = {
  success: 'bg-success',
  error: 'bg-sev-error',
  warning: 'bg-sev-warn',
  info: 'bg-primary',
  neutral: 'bg-muted-foreground',
}
const VALUE_TONE: Record<Tone, string> = {
  good: 'text-success',
  needs: 'text-sev-warn',
  poor: 'text-sev-error',
}
const DELTA_TONE = {
  good: 'text-success',
  bad: 'text-sev-error',
  neutral: 'text-muted-foreground',
}
</script>

<template>
  <div class="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-5">
    <component
      :is="k.to ? 'button' : 'div'"
      v-for="k in kpis"
      :key="k.key"
      :type="k.to ? 'button' : undefined"
      :class="
        cn(
          'relative overflow-hidden rounded-lg border border-border bg-card p-4 text-left shadow-1',
          k.to &&
            'cursor-pointer transition-[transform,box-shadow] duration-150 ease-out hover:shadow-2 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring motion-safe:hover:-translate-y-0.5',
        )
      "
      @click="k.to && emit('navigate', k.to)"
    >
      <span class="absolute inset-y-0 left-0 w-1" :class="ACCENT[k.accent]" />
      <p class="text-xs text-muted-foreground">{{ k.label }}</p>
      <div class="mt-2 flex items-baseline gap-2">
        <span :class="cn('text-2xl font-semibold tabular-nums', k.valueTone ? VALUE_TONE[k.valueTone] : 'text-foreground')">
          {{ k.value }}
        </span>
        <span v-if="k.delta" :class="cn('text-xs font-medium tabular-nums', DELTA_TONE[k.delta.tone])">
          {{ k.delta.text }}
        </span>
      </div>
      <div v-if="k.dist" class="mt-2.5 flex items-center gap-2">
        <VitalsDistributionBar class="w-16 shrink-0" :good="k.dist.good" :needs="k.dist.needs" :poor="k.dist.poor" />
        <span v-if="k.sub" class="truncate text-[11px] text-muted-foreground">{{ k.sub }}</span>
      </div>
      <p v-else-if="k.sub" class="mt-2 truncate text-[11px] text-muted-foreground">{{ k.sub }}</p>
    </component>
  </div>
</template>

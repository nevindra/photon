<script setup lang="ts">
// A latency heatmap: time across, latency bands up, cell darkness = how many requests landed there.
//
// This is the chart that exists because percentile lines cannot draw a distribution splitting in
// two. When half the requests are served from cache in 2ms and half fall through to a cold path at
// 800ms, p50/p90/p99 all sit in the empty gap between the modes and describe a latency no request
// actually experienced. Here you see two bands.
//
// Drawn as a plain CSS grid rather than through a chart engine: it is a grid of rectangles with a
// colour scale, and every axis label is a string the backend already sent. A canvas would add a
// dependency, lose text selection and hover targets, and gain nothing.
//
// The Apdex **T** and **4T** lines are drawn on these same axes on purpose. The Apdex score and
// this chart otherwise describe the same requests in two vocabularies that never meet, and a
// reader cannot check one against the other.
import { computed } from 'vue'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { formatCompact, formatDuration } from '@/lib/core/format'
import type { LatencyHeatmap } from '@/lib/core/api'

const props = defineProps<{
  data?: LatencyHeatmap | null
  loading?: boolean
  /** Apdex threshold in ms. Draws the T and 4T guide lines when set. */
  thresholdMs?: number | null
}>()

// Band index → its inclusive upper edge in ns; the overflow band has none.
const bounds = computed(() => (props.data?.bounds ?? []).map((b) => Number(b)))
const bandCount = computed(() => bounds.value.length + 1)

// Rows render top (slowest) to bottom (fastest), the way latency is read everywhere else.
const rows = computed(() =>
  Array.from({ length: bandCount.value }, (_, i) => bandCount.value - 1 - i),
)

const columns = computed(() => props.data?.columns ?? [])

// band → t → count, for O(1) cell lookup. Cells arrive sparse.
const grid = computed(() => {
  const m = new Map<number, number>()
  for (const [t, band, count] of props.data?.cells ?? []) m.set(band * 1e6 + t, count)
  return m
})
const at = (band: number, t: number): number => grid.value.get(band * 1e6 + t) ?? 0

// The scale is per-COLUMN, not global: a heatmap scaled globally shows one busy minute and leaves
// every other column near-black, which hides exactly the shape shifts the chart is for.
const columnMax = computed(() => {
  const out = new Array(columns.value.length).fill(0)
  for (const [t, , count] of props.data?.cells ?? []) out[t] = Math.max(out[t] ?? 0, count)
  return out
})

function intensity(band: number, t: number): number {
  const n = at(band, t)
  if (!n) return 0
  const max = columnMax.value[t] || 1
  // Square root, not linear: the tail bands hold few requests and are the ones worth seeing.
  return Math.sqrt(n / max)
}

function bandLabel(band: number): string {
  if (band >= bounds.value.length) return `> ${formatDuration(bounds.value[bounds.value.length - 1])}`
  return `≤ ${formatDuration(bounds.value[band])}`
}

function timeLabel(t: number): string {
  const ms = columns.value[t]
  return ms == null ? '' : new Date(ms).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' })
}

// Which band a guide line sits above — the boundary is drawn under the first band whose upper edge
// covers the threshold, so the line reads as "everything above here is worse than T".
function bandOf(ms: number): number {
  const ns = ms * 1e6
  const i = bounds.value.findIndex((b) => ns <= b)
  return i === -1 ? bounds.value.length : i
}
const guides = computed(() => {
  const t = props.thresholdMs
  if (!t) return []
  return [
    { label: 'T', band: bandOf(t), title: `Apdex T = ${t}ms — satisfied at or below this` },
    { label: '4T', band: bandOf(t * 4), title: `Apdex 4T = ${t * 4}ms — frustrated above this` },
  ]
})

// Four evenly spaced x labels: one per column is unreadable at 48 columns.
const labelEvery = computed(() => Math.max(1, Math.ceil(columns.value.length / 4)))

const empty = computed(() => !props.loading && !(props.data?.cells ?? []).length)
</script>

<template>
  <div class="flex flex-col gap-1">
    <div v-if="empty" class="flex h-40 items-center justify-center text-xs text-muted-foreground">
      No spans in range
    </div>

    <div v-else class="overflow-x-auto">
      <div class="min-w-[32rem]">
        <div
          v-for="band in rows"
          :key="band"
          class="flex items-stretch gap-1"
          :data-band="band"
        >
          <span class="w-20 shrink-0 py-px text-right font-mono text-[10px] leading-4 tabular-nums text-muted-foreground">
            {{ bandLabel(band) }}
          </span>
          <div
            class="flex min-w-0 flex-1 gap-px"
            :class="guides.some((g) => g.band === band) && 'border-t border-dashed border-foreground/40'"
          >
            <Tooltip v-for="(_, t) in columns" :key="t">
              <TooltipTrigger as-child>
                <div
                  class="h-4 min-w-0 flex-1 rounded-[1px] bg-primary transition-opacity"
                  :data-cell="`${band}:${t}`"
                  :style="{ opacity: intensity(band, t) || 0.04 }"
                />
              </TooltipTrigger>
              <TooltipContent class="text-xs tabular-nums">
                {{ formatCompact(at(band, t)) }} spans · {{ bandLabel(band) }} · {{ timeLabel(t) }}
              </TooltipContent>
            </Tooltip>
          </div>
          <span
            v-if="guides.find((g) => g.band === band)"
            class="w-6 shrink-0 text-[10px] font-medium leading-4 text-foreground/70"
            :title="guides.find((g) => g.band === band)?.title"
          >
            {{ guides.find((g) => g.band === band)?.label }}
          </span>
          <span v-else class="w-6 shrink-0" />
        </div>

        <div class="mt-1 flex gap-1">
          <span class="w-20 shrink-0" />
          <div class="flex min-w-0 flex-1 justify-between font-mono text-[10px] text-muted-foreground">
            <span v-for="t in Math.floor(columns.length / labelEvery) + 1" :key="t">
              {{ timeLabel((t - 1) * labelEvery) }}
            </span>
          </div>
          <span class="w-6 shrink-0" />
        </div>
      </div>
    </div>

    <p v-if="data?.open_spans" data-testid="open-spans" class="text-[11px] text-sev-warn">
      {{ formatCompact(data.open_spans) }} spans never closed, so they are in no band above.
    </p>
  </div>
</template>

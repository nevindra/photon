<script setup>
// LCP attribution: a single horizontal bar split into the four LCP sub-parts (TTFB, resource
// load delay, resource load time, element render delay), each segment sized by its share of the
// total, plus a legend and an actionable insight line naming the dominant segment + the LCP
// element. Missing/null sub-parts are omitted (no segment, no legend entry). Tone classes are
// literal Tailwind strings (never interpolated) so the content scanner keeps them: four calm,
// distinct hues with white text.
import { computed } from 'vue'

const props = defineProps({
  ttfb: { type: Number, default: null },
  resourceLoadDelay: { type: Number, default: null },
  resourceLoadTime: { type: Number, default: null },
  elementRenderDelay: { type: Number, default: null },
  element: { type: String, default: null },
})

// The four sub-parts, in the order they occur in the LCP timeline. `color` is a literal bg class.
const PARTS = [
  { key: 'ttfb', label: 'TTFB', color: 'bg-sky-500' },
  { key: 'resourceLoadDelay', label: 'Resource load delay', color: 'bg-violet-500' },
  { key: 'resourceLoadTime', label: 'Resource load time', color: 'bg-teal-500' },
  { key: 'elementRenderDelay', label: 'Element render delay', color: 'bg-amber-500' },
]

const present = computed(() =>
  PARTS.map((p) => ({ ...p, ms: props[p.key] })).filter(
    (s) => s.ms != null && Number.isFinite(s.ms),
  ),
)

const totalMs = computed(() => present.value.reduce((a, s) => a + s.ms, 0))

const segments = computed(() =>
  present.value.map((s) => ({
    ...s,
    pct: totalMs.value > 0 ? (s.ms / totalMs.value) * 100 : 0,
  })),
)

// The largest sub-part — what to preload / shrink / defer to move LCP the most.
const dominant = computed(() => {
  const segs = segments.value
  if (!segs.length) return null
  return segs.reduce((a, b) => (b.ms > a.ms ? b : a))
})

function fmtMs(ms) {
  return `${Math.round(ms)} ms`
}
</script>

<template>
  <div v-if="segments.length" class="flex flex-col gap-3">
    <!-- Segmented bar -->
    <div class="flex h-9 w-full overflow-hidden rounded-lg bg-muted" role="img" aria-label="LCP sub-part breakdown">
      <div
        v-for="s in segments"
        :key="s.key"
        data-testid="lcp-segment"
        :data-part="s.key"
        :class="[s.color, 'flex min-w-0 items-center justify-center px-1 text-[11px] font-medium text-white']"
        :style="{ width: s.pct + '%' }"
        :title="`${s.label} · ${fmtMs(s.ms)}`"
      >
        <span class="truncate tabular-nums">{{ fmtMs(s.ms) }}</span>
      </div>
    </div>

    <!-- Legend -->
    <ul class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
      <li v-for="s in segments" :key="s.key" class="flex items-center gap-1.5">
        <span :class="[s.color, 'size-2.5 shrink-0 rounded-sm']" />
        <span>{{ s.label }}</span>
        <span class="tabular-nums text-foreground">{{ fmtMs(s.ms) }}</span>
      </li>
    </ul>

    <!-- Insight line -->
    <p v-if="dominant" data-testid="lcp-insight" class="text-xs text-muted-foreground">
      <span class="font-medium text-foreground">{{ dominant.label }}</span>
      dominates ({{ Math.round(dominant.pct) }}%)<template v-if="element"> ·
        element <code class="rounded bg-muted px-1 py-0.5 font-mono text-[11px] text-foreground">{{ element }}</code></template>
    </p>
  </div>
</template>

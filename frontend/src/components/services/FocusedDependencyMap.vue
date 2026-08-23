<script setup lang="ts">
// The focused map: who calls this service, and what it calls.
//
// **Always one hop.** A service map that tries to show the whole fleet around one service becomes
// unreadable at about twenty nodes, which is exactly when you need it. Three columns of one hop
// stay readable at any fleet size, and the fleet graph at `/services/map` is where you go when you
// want the shape of the whole thing.
//
// **Services, databases and external calls share one picture.** To someone debugging at 3am the
// distinction does not matter: `postgres` timing out and `stripe.com` timing out are the same
// question, "what is this service waiting on". They are still labelled, because the distinction
// matters once you know the answer — a database is not a Service, and only the Service side is
// clickable.
//
// SVG is hand-authored rather than laid out by a library: three fixed columns need no algorithm,
// and the layout dependency is worth paying for exactly once, on the fleet graph.
import { computed } from 'vue'
import { formatCompact, formatDuration } from '@/lib/core/format'
import type { ServiceMapEdge } from '@/lib/core/api'

/** One box on the left or right column, normalized across the three source shapes. */
export interface MapSideNode {
  key: string
  label: string
  kind: 'service' | 'database' | 'external'
  count: number
  errorRate: number
  /** Nanosecond string, or `null` when the source has no percentile. */
  p99: string | null
}

const props = withDefaults(
  defineProps<{
    service: string
    /** Edges INTO this service. */
    callers: ServiceMapEdge[]
    /** Edges OUT of this service, to other services. */
    callees: ServiceMapEdge[]
    /** Database + external dependencies — drawn on the right beside the service callees. */
    dependencies?: MapSideNode[]
    loading?: boolean
    /** How many boxes each column shows before folding the rest into a "+N more" line. */
    max?: number
  }>(),
  { dependencies: () => [], loading: false, max: 8 },
)

const emit = defineEmits<{ select: [MapSideNode] }>()

// --- geometry -------------------------------------------------------------------------------
const BOX_W = 168
const BOX_H = 38
const GAP = 10
const COL_GAP = 96
const PAD = 12

const edgeNode = (e: ServiceMapEdge, side: 'from' | 'to'): MapSideNode => ({
  key: `service:${e[side]}`,
  label: e[side],
  kind: 'service',
  count: e.count,
  errorRate: e.error_rate,
  p99: e.p99,
})

const left = computed<MapSideNode[]>(() =>
  [...props.callers].sort((a, b) => b.count - a.count).map((e) => edgeNode(e, 'from')),
)
const right = computed<MapSideNode[]>(() =>
  [...props.callees.map((e) => edgeNode(e, 'to')), ...props.dependencies].sort(
    (a, b) => b.count - a.count,
  ),
)

const leftShown = computed(() => left.value.slice(0, props.max))
const rightShown = computed(() => right.value.slice(0, props.max))
const leftHidden = computed(() => left.value.length - leftShown.value.length)
const rightHidden = computed(() => right.value.length - rightShown.value.length)

const rows = computed(() => Math.max(1, leftShown.value.length, rightShown.value.length))
const height = computed(() => PAD * 2 + rows.value * BOX_H + (rows.value - 1) * GAP + 20)
const width = BOX_W * 3 + COL_GAP * 2 + PAD * 2

const colX = { left: PAD, center: PAD + BOX_W + COL_GAP, right: PAD + (BOX_W + COL_GAP) * 2 }

/** Column boxes are centred as a block, so a short column lines up with the middle one. */
function boxY(index: number, total: number): number {
  const blockH = total * BOX_H + (total - 1) * GAP
  const top = PAD + (height.value - 20 - PAD * 2 - blockH) / 2
  return top + index * (BOX_H + GAP)
}
const centerY = computed(() => boxY(0, 1))

// --- edge styling ---------------------------------------------------------------------------
const maxCount = computed(() =>
  Math.max(1, ...leftShown.value.map((n) => n.count), ...rightShown.value.map((n) => n.count)),
)

// Throughput is the LINE WEIGHT and error rate is the HUE, never both on one channel: a thin red
// line and a thick grey one must be readable as two different facts.
function strokeWidth(count: number): number {
  return 1 + 3.5 * Math.sqrt(count / maxCount.value)
}
function stroke(errorRate: number): string {
  if (errorRate >= 0.05) return 'hsl(var(--sev-error))'
  if (errorRate > 0) return 'hsl(var(--sev-warn))'
  return 'hsl(var(--border))'
}

function curve(x1: number, y1: number, x2: number, y2: number): string {
  const dx = Math.max(32, (x2 - x1) / 2)
  return `M ${x1},${y1} C ${x1 + dx},${y1} ${x2 - dx},${y2} ${x2},${y2}`
}

const KIND_LABEL: Record<MapSideNode['kind'], string> = {
  service: 'service',
  database: 'db',
  external: 'ext',
}

function title(n: MapSideNode): string {
  const parts = [`${n.label} · ${formatCompact(n.count)} calls`]
  if (n.errorRate > 0) parts.push(`${(n.errorRate * 100).toFixed(2)}% errors`)
  if (n.p99) parts.push(`p99 ${formatDuration(Number(n.p99))}`)
  return parts.join(' · ')
}
</script>

<template>
  <div class="overflow-hidden rounded-xl border border-border bg-card">
    <header class="flex items-baseline justify-between px-4 pb-2 pt-3.5">
      <h3 class="text-xs font-medium uppercase tracking-wider text-muted-foreground">One hop</h3>
      <span class="font-mono text-[10px] text-muted-foreground">
        {{ left.length }} in · {{ right.length }} out
      </span>
    </header>

    <div class="overflow-x-auto px-4 pb-4">
      <svg
        data-testid="focused-map"
        :viewBox="`0 0 ${width} ${height}`"
        :width="width"
        :height="height"
        role="img"
        :aria-label="`Callers and callees of ${service}`"
        class="max-w-full"
      >
        <!-- Curves are drawn first so boxes sit on top of them. -->
        <g fill="none">
          <path
            v-for="(n, i) in leftShown"
            :key="'lc-' + n.key"
            :d="curve(colX.left + BOX_W, boxY(i, leftShown.length) + BOX_H / 2, colX.center, centerY + BOX_H / 2)"
            :stroke="stroke(n.errorRate)"
            :stroke-width="strokeWidth(n.count)"
            stroke-linecap="round"
            opacity="0.85"
          />
          <path
            v-for="(n, i) in rightShown"
            :key="'rc-' + n.key"
            :d="curve(colX.center + BOX_W, centerY + BOX_H / 2, colX.right, boxY(i, rightShown.length) + BOX_H / 2)"
            :stroke="stroke(n.errorRate)"
            :stroke-width="strokeWidth(n.count)"
            stroke-linecap="round"
            opacity="0.85"
          />
        </g>

        <!-- Callers -->
        <g
          v-for="(n, i) in leftShown"
          :key="n.key"
          :data-map-node="n.label"
          data-side="caller"
          class="cursor-pointer"
          @click="emit('select', n)"
        >
          <title>{{ title(n) }}</title>
          <rect
            :x="colX.left"
            :y="boxY(i, leftShown.length)"
            :width="BOX_W"
            :height="BOX_H"
            rx="8"
            fill="hsl(var(--surface-1))"
            stroke="hsl(var(--border))"
          />
          <text :x="colX.left + 10" :y="boxY(i, leftShown.length) + 16" class="fill-foreground text-[11px]">
            {{ n.label }}
          </text>
          <text :x="colX.left + 10" :y="boxY(i, leftShown.length) + 30" class="fill-muted-foreground font-mono text-[10px]">
            {{ formatCompact(n.count) }} calls
          </text>
        </g>

        <!-- The service itself -->
        <g>
          <rect
            :x="colX.center"
            :y="centerY"
            :width="BOX_W"
            :height="BOX_H"
            rx="8"
            fill="hsl(var(--brand-soft))"
            stroke="hsl(var(--brand))"
          />
          <text :x="colX.center + 10" :y="centerY + 23" class="fill-foreground text-[12px] font-medium">
            {{ service }}
          </text>
        </g>

        <!-- Callees + dependencies -->
        <g
          v-for="(n, i) in rightShown"
          :key="n.key"
          :data-map-node="n.label"
          data-side="callee"
          :class="n.kind === 'service' ? 'cursor-pointer' : ''"
          @click="n.kind === 'service' && emit('select', n)"
        >
          <title>{{ title(n) }}</title>
          <rect
            :x="colX.right"
            :y="boxY(i, rightShown.length)"
            :width="BOX_W"
            :height="BOX_H"
            rx="8"
            fill="hsl(var(--surface-1))"
            :stroke="n.kind === 'service' ? 'hsl(var(--border))' : 'hsl(var(--border))'"
            :stroke-dasharray="n.kind === 'service' ? undefined : '3 3'"
          />
          <text :x="colX.right + 10" :y="boxY(i, rightShown.length) + 16" class="fill-foreground text-[11px]">
            {{ n.label }}
          </text>
          <text :x="colX.right + 10" :y="boxY(i, rightShown.length) + 30" class="fill-muted-foreground font-mono text-[10px]">
            {{ KIND_LABEL[n.kind] }} · {{ formatCompact(n.count) }}
          </text>
        </g>
      </svg>
    </div>

    <!-- A picture that quietly drops rows is a picture you cannot trust. The tables below carry
         every row; this says how many are missing from the drawing. -->
    <p
      v-if="leftHidden > 0 || rightHidden > 0"
      data-testid="focused-map-overflow"
      class="px-4 pb-3 text-[11px] text-muted-foreground"
    >
      <span v-if="leftHidden > 0">{{ leftHidden }} more caller{{ leftHidden === 1 ? '' : 's' }}</span>
      <span v-if="leftHidden > 0 && rightHidden > 0"> and </span>
      <span v-if="rightHidden > 0">{{ rightHidden }} more downstream</span>
      not drawn. The tables below list every one.
    </p>
  </div>
</template>

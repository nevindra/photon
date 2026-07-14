<script setup>
// The shared shell every chart family renders through. It owns the boring-but-load-bearing
// parts so LineChart / BarChart can stay thin: sizing (VueUse `useElementSize`), the uPlot
// instance lifecycle (`useUplot`), theme repaint (`useChartTheme`), and ALL of the interactive
// chrome that lives in the DOM *around* the canvas — the floating tooltip, the bottom-center
// toggle legend, marker / reference-line / outage-band overlays, brush-select → emit, and the
// empty / loading states.
//
// The one thing it does NOT own is what the chart looks like: the caller hands in a
// `buildOptions(UplotCtor, theme) => { opts, data }` (LineChart supplies the line/area builder,
// BarChart the bars builder). BaseChart calls it inside `useUplot`'s build and then AUGMENTS the
// returned opts with its own cursor + hooks wiring before uPlot is constructed — so every chart
// gets the same crosshair, tooltip, brush and overlay behaviour for free.
//
// uPlot NO-OPS in jsdom (no canvas 2D context — see useUplot), so everything here that a test
// needs to assert (overlays, legend, empty/loading, event translation) is plain Vue DOM that
// renders whether or not a real renderer ever attaches.
import { computed, ref, watch } from 'vue'
import { useElementSize } from '@vueuse/core'
import { formatNumber } from '@/lib/core/format'
import { useUplot } from './useUplot.js'
import { useChartTheme } from './useChartTheme.js'
import ChartTooltipCard from './ChartTooltipCard.vue'

const props = defineProps({
  // (UplotCtor, theme) => ({ opts, data }). The chart-specific builder (buildLineOptions /
  // buildBarOptions, already curried with the domain props by the wrapper). Called inside
  // useUplot's build with the loaded uPlot ctor + the live theme; we augment its `opts` before
  // construction. Also invoked (with a stub ctor) to detect data presence for the empty state.
  buildOptions: { type: Function, required: true },
  // Tooltip/row value formatter (LineChart/BarChart pass their `formatValue` through).
  formatValue: { type: Function, default: formatNumber },
  // Optional tooltip-header formatter for the x value. When omitted the header is a clock label
  // for a time x-scale, else the raw x value (BarChart value-mode passes its duration formatter).
  formatX: { type: Function, default: null },
  // Legend + toggle source: [{ key, label, color }] in the SAME order as the built y-series
  // (chip i drives uPlot series i+1). Empty → no legend row.
  legendItems: { type: Array, default: () => [] },
  // Optional RAW per-y-series values for the tooltip (one array per y-series, SAME order as the
  // built y-series). Stacked builders put CUMULATIVE tops in `data` so bars/areas draw correctly,
  // so the tooltip must read the de-stacked value from here instead. `null` → read straight off
  // `u.data` (unstacked charts, where data already holds raw values). BaseChart stays otherwise
  // stacking-agnostic: this is the ONLY stacking-aware lookup.
  tooltipData: { type: Array, default: null },
  // DOM overlays, all positioned from uPlot value→pixel (recomputed on draw/resize/rescale):
  markers: { type: Array, default: () => [] }, //   [{ x, label, color }]      dashed vertical + pill
  bands: { type: Array, default: () => [] }, //     [{ x0, x1, label, color }] translucent rect
  refLines: { type: Array, default: () => [] }, //  [{ y, label, color }]      dashed horizontal + pill
  loading: { type: Boolean, default: false },
  height: { type: Number, default: 240 }, //        chart pixel height (compact/mini charts pass a small value)
})

const emit = defineEmits(['select', 'legend-toggle', 'point-click'])

const container = ref(null)
// `elHeight` is the MEASURED element height (drives uPlot's canvas size); it's kept distinct from the
// `height` prop (the requested pixel height, applied to the container's style) so the two don't clash.
const { width, height: elHeight } = useElementSize(container)
const { theme, version } = useChartTheme()

// A no-op uPlot stand-in so we can run buildOptions purely to inspect its `data` (for the empty
// state) without a real engine — the builders only touch `uPlot.paths.*`, which we sentinel out.
const PROBE_UPLOT = { paths: { spline: () => null, bars: () => null } }

// --- empty / loading -------------------------------------------------------------------------
// Run the caller's builder against the probe ctor to get the shaped `data` without constructing
// uPlot; "has data" == at least one finite y value across the series arrays (data[0] is xs).
const hasData = computed(() => {
  try {
    const { data } = props.buildOptions(PROBE_UPLOT, theme.value) || {}
    if (!Array.isArray(data)) return false
    return data.slice(1).some((arr) => (arr || []).some((v) => Number.isFinite(v)))
  } catch (err) {
    // A throwing builder is a real bug (broken adapter), not "no data" — surface it in dev instead
    // of silently rendering the empty state. We still fall through to the empty state in prod.
    if (import.meta.env.DEV) console.error('[BaseChart] buildOptions threw while probing for data', err)
    return false
  }
})
const showEmpty = computed(() => !props.loading && !hasData.value)

// --- overlay pixel positions (recomputed from the live instance) -----------------------------
// uPlot's valToPos(val, scale, false) returns a CSS-pixel position relative to its root (which
// fills our container), so these map straight onto absolutely-positioned overlays. plot.* is the
// plotting-area box (axes-inset) in CSS px, derived from u.bbox / the pixel ratio.
const plot = ref({ left: 0, top: 0, width: 0, height: 0 })
const markerPx = ref([])
const bandPx = ref([])
const refLinePx = ref([])

function recomputeOverlays(u) {
  const uu = u || uplot.value
  if (!uu || !uu.bbox || typeof uu.valToPos !== 'function') return
  const ratio = uu.pxRatio || (typeof devicePixelRatio !== 'undefined' ? devicePixelRatio : 1) || 1
  plot.value = {
    left: uu.bbox.left / ratio,
    top: uu.bbox.top / ratio,
    width: uu.bbox.width / ratio,
    height: uu.bbox.height / ratio,
  }
  markerPx.value = (props.markers || []).map((m) => uu.valToPos(m.x, 'x', false))
  bandPx.value = (props.bands || []).map((b) => {
    const a = uu.valToPos(b.x0, 'x', false)
    const c = uu.valToPos(b.x1, 'x', false)
    return { left: Math.min(a, c), width: Math.abs(c - a) }
  })
  refLinePx.value = (props.refLines || []).map((r) => uu.valToPos(r.y, 'y', false))
}

// --- floating tooltip ------------------------------------------------------------------------
const tooltip = ref(null) // { show, left, top, title, rows }

const pad = (n) => String(n).padStart(2, '0')
function tooltipTitle(u, xval) {
  if (props.formatX) return props.formatX(xval)
  if (u.scales?.x?.time) {
    const d = new Date(xval * 1000)
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  }
  return String(xval)
}

// uPlot `setCursor` hook: reads the resolved cursor index/position, builds one row per series
// that has a finite value at that x (label/colour from legendItems, value via formatValue),
// sorted descending by value. Hidden when the cursor is off-plot (idx null / negative left).
function onSetCursor(u) {
  const { idx, left, top } = u.cursor || {}
  if (idx == null || left == null || left < 0) {
    tooltip.value = null
    return
  }
  const xval = u.data[0][idx]
  const rows = []
  for (let i = 1; i < u.data.length; i++) {
    // De-stack: prefer the raw per-series value (props.tooltipData is y-indexed, so series i is
    // index i-1) and fall back to u.data (unstacked charts, where data already holds raw values).
    const v = props.tooltipData?.[i - 1]?.[idx] ?? u.data[i][idx]
    if (v == null || !Number.isFinite(v)) continue
    const item = props.legendItems[i - 1]
    rows.push({
      key: item?.key ?? String(i),
      label: item?.label ?? u.series?.[i]?.label ?? String(i),
      raw: Number(v),
      value: props.formatValue(Number(v)),
      swatchColor: item?.color ?? u.series?.[i]?.stroke,
    })
  }
  rows.sort((a, b) => b.raw - a.raw)
  tooltip.value = {
    show: rows.length > 0,
    left: plot.value.left + left,
    top: plot.value.top + top,
    title: tooltipTitle(u, xval),
    rows,
  }
}

// --- brush select → emit ---------------------------------------------------------------------
// opts already set cursor.drag = { x, y:false, setScale:false } so a drag makes a selection
// WITHOUT uPlot rescaling. This `setSelect` hook maps the selection px→x-values and emits; the
// wrapper (LineChart/BarChart) translates {minX,maxX} into zoom/brush in its own units. We clear
// the selection after emit so it doesn't linger on the canvas.
function onSetSelect(u) {
  const sel = u.select
  if (!sel || !(sel.width > 0)) {
    // A zero-width selection is a click, not a brush — emit the clicked x (x-scale units) so a
    // caller can correlate at that instant. Guard for the jsdom/no-cursor path.
    const left = u.cursor?.left
    if (sel && sel.width === 0 && left != null && left >= 0 && typeof u.posToVal === 'function') {
      emit('point-click', { x: u.posToVal(left, 'x') })
    }
    return
  }
  const a = u.posToVal(sel.left, 'x')
  const b = u.posToVal(sel.left + sel.width, 'x')
  emit('select', { minX: Math.min(a, b), maxX: Math.max(a, b) })
  if (typeof u.setSelect === 'function') u.setSelect({ width: 0, height: 0 }, false)
}

// Theme the built-in crosshair line (a DOM element uPlot styles via CSS we don't control) once
// the instance is live. No-ops without a real root (jsdom).
function onReady(u) {
  const lines = u.root?.querySelectorAll?.('.u-cursor-x, .u-cursor-y')
  lines?.forEach((el) => (el.style.borderColor = theme.value.crosshair))
  recomputeOverlays(u)
}

// Fold BaseChart's cursor + hooks onto the caller's opts right before construction. We keep the
// builder's drag config (non-rescaling brush) and turn ON the crosshair x-line + per-series
// cursor points (the builder leaves them off); all interactivity funnels through the hooks above.
function augment(opts) {
  const merge = (name, fn) => [...(opts.hooks?.[name] || []), fn]
  return {
    ...opts,
    cursor: {
      ...(opts.cursor || {}),
      x: true,
      y: false,
      points: { ...(opts.cursor?.points || {}), show: true },
    },
    hooks: {
      ...(opts.hooks || {}),
      ready: merge('ready', onReady),
      setCursor: merge('setCursor', onSetCursor),
      setSelect: merge('setSelect', onSetSelect),
      draw: merge('draw', recomputeOverlays),
      setScale: merge('setScale', recomputeOverlays),
      setSize: merge('setSize', recomputeOverlays),
    },
  }
}

// useUplot re-invokes this whenever its reactive inputs (props/theme) change; it must read them
// synchronously. We build via the caller, then augment.
function build(UplotCtor) {
  const { opts, data } = props.buildOptions(UplotCtor, theme.value)
  return { opts: augment(opts), data }
}

const { uplot, rebuild } = useUplot(container, build, { width, height: elHeight })

// Repaint on light↔dark: theme colours are baked into opts at construction, so a bump means a
// full rebuild (not just a redraw) — matches the frozen useUplot / useChartTheme contract.
watch(version, () => rebuild())

// Overlay props can change without a data/scale change; recompute against the live instance.
watch(() => [props.markers, props.bands, props.refLines], () => recomputeOverlays(), { deep: true })

// --- legend toggle ---------------------------------------------------------------------------
const shown = ref({}) // key -> bool (default shown)
watch(
  () => props.legendItems,
  (items) => {
    const next = {}
    for (const it of items || []) next[it.key] = shown.value[it.key] ?? true
    shown.value = next
  },
  { immediate: true, deep: true },
)
const isShown = (key) => shown.value[key] !== false

function toggleSeries(i, item) {
  const next = !isShown(item.key)
  shown.value = { ...shown.value, [item.key]: next }
  // uPlot series are 1-indexed (series[0] is the implicit x); guard for the jsdom no-op path.
  if (uplot.value && typeof uplot.value.setSeries === 'function') uplot.value.setSeries(i + 1, { show: next })
  emit('legend-toggle', { key: item.key, shown: next })
}

// Overlay style helpers (default to the plot box before the first draw / in jsdom).
const markerStyle = (i) => ({ left: (markerPx.value[i] ?? 0) + 'px', top: plot.value.top + 'px', height: plot.value.height + 'px' })
const bandStyle = (i) => {
  const b = bandPx.value[i] || { left: 0, width: 0 }
  return { left: b.left + 'px', width: b.width + 'px', top: plot.value.top + 'px', height: plot.value.height + 'px' }
}
const refLineStyle = (i) => ({ top: (refLinePx.value[i] ?? 0) + 'px', left: plot.value.left + 'px', width: plot.value.width + 'px' })

// Exposed for wrapper tests: uPlot no-ops in jsdom, so tests drive these hooks directly with a
// stubbed instance to exercise select emit / tooltip build instead of a real canvas cursor.
defineExpose({ onSetSelect, onSetCursor, recomputeOverlays, uplot })
</script>

<template>
  <div ref="container" class="relative w-full" :style="{ height: height + 'px' }">
    <!-- uPlot mounts its canvas into this container; overlays sit on top, positioned from data. -->

    <!-- empty / loading (render regardless of the renderer — uPlot no-ops in jsdom) -->
    <div
      v-if="loading"
      data-testid="chart-loading"
      class="absolute inset-0 z-10 flex items-center justify-center text-[12px] text-muted-foreground"
    >
      Loading…
    </div>
    <div
      v-else-if="showEmpty"
      data-testid="chart-empty"
      class="absolute inset-0 z-10 flex items-center justify-center text-[12px] text-muted-foreground"
    >
      No data in this window
    </div>

    <!-- data-positioned overlays (bands under markers/reflines) -->
    <div class="pointer-events-none absolute inset-0 overflow-hidden">
      <div
        v-for="(b, i) in bands"
        :key="'band' + i"
        data-testid="chart-band"
        class="absolute"
        :style="bandStyle(i)"
      >
        <div class="h-full w-full" :style="{ background: b.color, opacity: 0.13 }" />
        <span
          v-if="b.label"
          class="absolute left-1 top-1 rounded bg-popover/80 px-1 py-0.5 text-[9px] font-medium text-muted-foreground"
        >
          {{ b.label }}
        </span>
      </div>

      <div
        v-for="(m, i) in markers"
        :key="'marker' + i"
        data-testid="chart-marker"
        class="absolute"
        :style="markerStyle(i)"
      >
        <div class="h-full border-l border-dashed" :style="{ borderColor: m.color }" />
        <span
          class="absolute -top-0.5 left-1 whitespace-nowrap rounded px-1 py-0.5 text-[9px] font-semibold text-popover"
          :style="{ background: m.color }"
        >
          {{ m.label }}
        </span>
      </div>

      <div
        v-for="(r, i) in refLines"
        :key="'refline' + i"
        data-testid="chart-refline"
        class="absolute -translate-y-1/2"
        :style="refLineStyle(i)"
      >
        <div class="w-full border-t border-dashed" :style="{ borderColor: r.color }" />
        <span
          class="absolute right-0 -top-2 whitespace-nowrap rounded px-1 py-0.5 text-[9px] font-semibold text-popover"
          :style="{ background: r.color }"
        >
          {{ r.label }}
        </span>
      </div>
    </div>

    <!-- floating tooltip (follows the cursor; content built in the setCursor hook) -->
    <div
      v-if="tooltip && tooltip.show"
      data-testid="chart-tooltip"
      class="pointer-events-none absolute z-20 -translate-x-1/2 -translate-y-[calc(100%+12px)]"
      :style="{ left: tooltip.left + 'px', top: tooltip.top + 'px' }"
    >
      <ChartTooltipCard :title="tooltip.title" :rows="tooltip.rows" />
    </div>

    <!-- bottom-center toggle legend — a lone chip is noise (and duplicates some callers' own
         legend, e.g. ResponseTimeChart), so it only earns its place with 2+ series to distinguish. -->
    <div
      v-if="legendItems.length > 1"
      data-testid="chart-legend"
      class="absolute inset-x-0 bottom-0 z-10 flex flex-wrap items-center justify-center gap-x-3 gap-y-1 px-2"
    >
      <button
        v-for="(it, i) in legendItems"
        :key="it.key"
        type="button"
        data-testid="chart-legend-item"
        class="flex items-center gap-1.5 text-[11px] text-muted-foreground transition-opacity"
        :class="isShown(it.key) ? 'opacity-100' : 'opacity-40 line-through'"
        @click="toggleSeries(i, it)"
      >
        <span class="inline-block size-2 shrink-0 rounded-sm" :style="{ background: it.color }" />
        {{ it.label }}
      </button>
    </div>
  </div>
</template>

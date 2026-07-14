<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useResizeObserver } from '@vueuse/core'

// Code-editor-style vertical minimap for deep traces. The scaling pain in the waterfall is
// vertical (it always shows the full trace width), so this is a bird's-eye of the row list:
// each row is a 1px line whose x/width mirror its span bar. The canvas is VISUAL-ONLY — all
// interaction (the draggable viewport rectangle) lives in the DOM overlay so it stays testable.
const props = defineProps({
  // openRows nodes: each exposes id, offsetNs, durationNs, isError.
  rows: { type: Array, default: () => [] },
  traceDurationNs: { type: [BigInt, Number], default: 0 },
  scrollTop: { type: Number, default: 0 },
  viewportHeight: { type: Number, default: 0 },
  totalHeight: { type: Number, default: 0 },
  // Optional set of matched span ids → painted as bright ticks.
  matches: { type: Object, default: null },
})
const emit = defineEmits(['scroll-to'])

const rootEl = ref(null)
const canvasEl = ref(null)

function clamp(v, lo, hi) {
  return Math.max(lo, Math.min(hi, v))
}

// --- canvas draw (visual only; no-op without a 2D context, e.g. jsdom) ---
function draw() {
  const canvas = canvasEl.value
  if (!canvas) return
  const ctx = canvas.getContext('2d')
  if (!ctx) return // jsdom / headless: skip pixel work entirely
  const rect = canvas.getBoundingClientRect()
  const W = rect.width || canvas.clientWidth
  const H = rect.height || canvas.clientHeight
  if (!W || !H) return

  const dpr = window.devicePixelRatio || 1
  canvas.width = Math.round(W * dpr)
  canvas.height = Math.round(H * dpr)
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  ctx.clearRect(0, 0, W, H)

  // Resolve theme colours from CSS vars (HSL tuples) so light/dark both work.
  const styles = getComputedStyle(canvas)
  const varColor = (name) => `hsl(${styles.getPropertyValue(name).trim()})`
  const neutral = varColor('--muted-foreground')
  const errorColor = varColor('--sev-error')
  const bright = varColor('--foreground')

  const rows = props.rows
  const n = rows.length || 1
  const dur = Number(props.traceDurationNs) || 1
  const matches = props.matches

  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]
    const y = (i / n) * H
    const x = (Number(row.offsetNs) / dur) * W
    const w = Math.max(1, (Number(row.durationNs) / dur) * W)
    const isMatch = matches ? matches.has(row.id) : false
    if (row.isError) {
      ctx.globalAlpha = 1
      ctx.fillStyle = errorColor
    } else if (isMatch) {
      ctx.globalAlpha = 1
      ctx.fillStyle = bright
    } else {
      ctx.globalAlpha = 0.5
      ctx.fillStyle = neutral
    }
    ctx.fillRect(x, y, Math.min(w, W - x), 1)
  }
  ctx.globalAlpha = 1
}

useResizeObserver(canvasEl, () => draw())
watch(() => [props.rows, props.matches, props.traceDurationNs], () => draw(), { deep: false })
onMounted(draw)

// --- viewport rectangle (DOM overlay) ---
const viewportStyle = computed(() => {
  const total = props.totalHeight || 1
  const top = clamp((props.scrollTop / total) * 100, 0, 100)
  const height = clamp((props.viewportHeight / total) * 100, 0, 100)
  return { top: `${top}%`, height: `${height}%` }
})

// --- drag / click to scroll ---
// A plain click and a drag share one path: pointerdown seeks immediately, then window
// pointermove keeps seeking until pointerup. The target scrollTop centres the viewport on the
// pointer, clamped to a valid scroll range.
let dragging = false

function emitScrollTo(clientY) {
  const el = rootEl.value
  if (!el) return
  const rect = el.getBoundingClientRect()
  const H = el.offsetHeight || rect.height
  if (!H) return
  const y = clientY - rect.top
  const raw = (y / H) * (Number(props.totalHeight) || 0)
  const target = raw - (Number(props.viewportHeight) || 0) / 2
  const max = Math.max(0, (Number(props.totalHeight) || 0) - (Number(props.viewportHeight) || 0))
  emit('scroll-to', clamp(target, 0, max))
}

function onWindowPointerMove(e) {
  if (!dragging) return
  emitScrollTo(e.clientY)
}
function onWindowPointerUp() {
  dragging = false
  window.removeEventListener('pointermove', onWindowPointerMove)
  window.removeEventListener('pointerup', onWindowPointerUp)
}
function onPointerDown(e) {
  e.preventDefault()
  dragging = true
  emitScrollTo(e.clientY)
  window.addEventListener('pointermove', onWindowPointerMove)
  window.addEventListener('pointerup', onWindowPointerUp)
}

onBeforeUnmount(() => {
  window.removeEventListener('pointermove', onWindowPointerMove)
  window.removeEventListener('pointerup', onWindowPointerUp)
})
</script>

<template>
  <div
    ref="rootEl"
    data-testid="trace-minimap"
    class="relative w-[72px] shrink-0 cursor-pointer select-none border-l border-border bg-muted/20"
    role="scrollbar"
    aria-label="Trace minimap"
    @pointerdown="onPointerDown"
  >
    <canvas ref="canvasEl" class="block h-full w-full" />
    <div
      data-testid="trace-minimap-viewport"
      class="pointer-events-none absolute inset-x-0 rounded-sm border border-foreground/30 bg-foreground/10"
      :style="viewportStyle"
    />
  </div>
</template>

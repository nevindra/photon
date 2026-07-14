// Owns a single uPlot instance's lifecycle inside a Vue component: it lazy-loads the engine,
// creates the chart into a container element once it has a real size, keeps its data/size in
// sync, and tears it down on unmount. Rendering primitives (BaseChart) drive this; the pure
// option/data shaping lives in chartOptions.js — this file is only the imperative lifecycle.
//
// LAZY / CODE-SPLIT: uPlot (and its CSS) are pulled in with dynamic `import()` so the ~50 KB
// engine is its own chunk, off the initial-load critical path (see the charting design doc).
//
// jsdom-SAFE: if there's no window, or a <canvas> can't produce a 2D context (headless test
// env), instance creation is skipped entirely and every returned handle no-ops — mirrors the
// TraceMinimap canvas guard so wrapper tests run without a real renderer.
import { onMounted, onUnmounted, shallowRef, toValue, watch } from 'vue'

// True only where uPlot can actually paint — a real window AND a working canvas 2D context.
// jsdom has a window but returns null from getContext('2d'), so this is false there.
function canRender() {
  if (typeof window === 'undefined' || typeof document === 'undefined') return false
  try {
    const canvas = document.createElement('canvas')
    return typeof canvas.getContext === 'function' && !!canvas.getContext('2d')
  } catch {
    return false
  }
}

/**
 * Drive a uPlot instance from a Vue component.
 *
 * @param {import('vue').Ref<HTMLElement|null>} container  element uPlot mounts its canvas into.
 * @param {(uPlot: any) => { opts: object, data: any[] }} build  builds the uPlot opts + data.
 *   Called with the uPlot constructor (for `uPlot.paths.*`) once loaded, and re-invoked whenever
 *   its reactive inputs change (its return drives setData) or on an explicit rebuild().
 * @param {{ width: (number|import('vue').Ref<number>), height: (number|import('vue').Ref<number>) }} size
 *   reactive size, e.g. VueUse `useElementSize(container)` supplied by the caller.
 * @returns {{ uplot: import('vue').ShallowRef<any|null>, rebuild: () => void, redraw: () => void }}
 *   `uplot`   — the live instance (null before load / in jsdom).
 *   `rebuild` — destroy + recreate from a fresh build() (opts-structure / theme changes).
 *   `redraw`  — cheap repaint of the existing instance (u.redraw()).
 */
export function useUplot(container, build, size) {
  const uplot = shallowRef(null)
  let UplotCtor = null // resolved uPlot constructor once the dynamic import lands
  const loaded = shallowRef(false) // reactive trigger: flips true after the import resolves
  let disposed = false

  // A lightweight signature of the opts' SERIES set (count + each series' identity). uPlot bakes its
  // series at construction, so `setData()` can only refill EXISTING series — it can't add/remove or
  // re-identify them. Charts whose series are data-derived (metrics group-bys, severity histograms)
  // are created while their query is still pending (0 series) and later receive data with N series;
  // a pure setData would push N arrays into a 0-series instance and draw nothing. When this signature
  // changes we recreate the instance instead. `null` until the first build.
  let lastSig = null
  function seriesSig(opts) {
    // Per-series FILL presence is folded in: an area fill (or its removal) is baked into opts at
    // construction — uPlot can't toggle it via setData — so switching Line↔Area↔Stacked (which flips
    // `series[].fill` on/off) must recreate, not refill.
    const series = (opts?.series || []).map((s) => `${s.label ?? ''}|${s.stroke ?? ''}|${s.fill ? 'F' : ''}`).join('~')
    // The x window is baked into the scale's `range` fn at construction, so a window change (time
    // picker, live-tail advancing "now") must recreate too — fold its bounds into the signature.
    let win = ''
    try {
      const r = opts?.scales?.x?.range
      const b = typeof r === 'function' ? r() : r
      if (Array.isArray(b)) win = `${b[0]},${b[1]}`
    } catch {
      // a range fn that needs live uPlot args is not window-pinned; ignore it for the signature
    }
    // The y-scale distribution (linear vs base-10 log, `distr:3`) is likewise baked at construction,
    // so toggling the log-y axis must recreate the instance rather than only refill data.
    const ydistr = opts?.scales?.y?.distr ?? ''
    return `${series}#${win}#${ydistr}`
  }

  const width = () => Math.max(0, Math.round(toValue(size?.width) || 0))
  const height = () => Math.max(0, Math.round(toValue(size?.height) || 0))

  // Import the engine (and its stylesheet) once, but only where it can render — this keeps the
  // CSS import out of the jsdom path so tests never choke on it.
  async function load() {
    if (UplotCtor || !canRender()) return
    const mod = await import('uplot')
    UplotCtor = mod.default || mod
    try {
      await import('uplot/dist/uPlot.min.css')
    } catch {
      // CSS is cosmetic; a resolver that can't load it (some test setups) must not break the chart.
    }
  }

  // Create the instance if everything is ready: loaded ctor, live container, real size, renderer.
  // `result` lets the data watcher reuse the build() it already ran (avoids a double build).
  function create(result) {
    if (disposed || uplot.value || !UplotCtor) return
    const el = toValue(container)
    const w = width()
    const h = height()
    if (!el || !canRender() || !w || !h) return
    const { opts, data } = result || build(UplotCtor)
    uplot.value = new UplotCtor({ ...opts, width: w, height: h }, data, el)
    lastSig = seriesSig(opts) // record what we actually built so the data watcher compares correctly
  }

  function destroy() {
    if (!uplot.value) return
    try {
      uplot.value.destroy()
    } catch {
      // ignore — best-effort teardown
    }
    uplot.value = null
  }

  // Data (and opts) reactivity: re-run build() when its inputs — or `loaded` — change. On the
  // first change we create; thereafter we push new data with setData (cheap, no re-instantiation).
  watch(
    () => (loaded.value && UplotCtor ? build(UplotCtor) : null),
    (result) => {
      if (!result) return
      if (!uplot.value) {
        create(result) // first build (create() records lastSig)
        return
      }
      if (seriesSig(result.opts) !== lastSig) {
        // The series set changed (async data arriving after an empty create, or a re-group) — uPlot
        // can't grow/re-identify series in place, so recreate from the fresh opts + data.
        destroy()
        create(result)
        return
      }
      // Same series set AND same window — cheap data refill (a window change would have changed the
      // signature above and recreated with a freshly-baked range).
      uplot.value.setData(result.data)
    },
  )

  // Size reactivity: resize a live instance; if none yet, a first real size may be what unblocks
  // creation (the container had 0×0 at import time).
  watch(
    () => [width(), height()],
    ([w, h]) => {
      if (!w || !h) return
      if (uplot.value) uplot.value.setSize({ width: w, height: h })
      else create()
    },
  )

  onMounted(async () => {
    await load()
    if (disposed) return
    loaded.value = true // (re)triggers the data watcher → create() when the renderer is available
  })

  onUnmounted(() => {
    disposed = true
    destroy()
  })

  // Recreate on structural/theme changes the caller detects (uPlot bakes opts at construction).
  function rebuild() {
    destroy()
    create()
  }

  // Light repaint for changes that draw hooks pick up without a new instance.
  function redraw() {
    uplot.value?.redraw?.()
  }

  return { uplot, rebuild, redraw }
}

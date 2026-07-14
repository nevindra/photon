// frontend/src/components/charts/chartOptions.js
// Pure, framework-free option builders + data normalizers for the uPlot charting layer.
//
// This file MUST stay I/O-free: no Vue, no DOM, and no top-level `uplot` import. The loaded
// uPlot module is *passed in* to the builders (so the layer can lazy-load/code-split uPlot and
// so unit tests can hand in a tiny fake). Everything here is a table-testable transform — the
// primary unit-test surface for the whole charting system.
//
// Timestamps cross the component boundary as **millisecond Numbers**; uPlot works in **seconds**
// internally, so we divide by 1000 at the uPlot edge (`msToSec`). Series identity colors come
// from `seriesColor()` hex (already canvas-safe); axis/grid colors come from the `theme` object
// (resolved elsewhere by useChartTheme — here it's just a bag of color strings).
import { formatNumber } from '@/lib/core/format'
import { seriesColor } from '@/lib/core/seriesColor'

const pad = (n) => String(n).padStart(2, '0')

// ms Number → seconds Number (fractional preserved — uPlot's x scale is UNIX seconds).
export function msToSec(ms) {
  return Number(ms) / 1000
}

// Seconds (uPlot x value) → short "HH:MM" clock label for axis ticks.
function clockLabel(sec) {
  const d = new Date(sec * 1000)
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`
}

// hex ('#rgb' / '#rrggbb') → 'rgba(r, g, b, a)'; a bare 'hsl(h s% l%)' triplet (the shape
// useChartTheme's `solid()` hands back, e.g. the brand default below) → 'hsl(h s% l% / a)'.
// Anything else is returned unchanged (best effort so a caller-provided named/rgb(a)/hsl(...
// / a) color still flows through). Used for area fill + dimming.
function withAlpha(color, alpha) {
  const str = typeof color === 'string' ? color.trim() : ''
  const hsl = /^hsl\(\s*([\d.]+)\s+([\d.]+)%\s+([\d.]+)%\s*\)$/i.exec(str)
  if (hsl) {
    const [, h, s, l] = hsl
    return `hsl(${h} ${s}% ${l}% / ${alpha})`
  }
  let r
  let g
  let b
  if (/^#[0-9a-f]{6}$/i.test(str)) {
    r = parseInt(str.slice(1, 3), 16)
    g = parseInt(str.slice(3, 5), 16)
    b = parseInt(str.slice(5, 7), 16)
  } else if (/^#[0-9a-f]{3}$/i.test(str)) {
    r = parseInt(str[1] + str[1], 16)
    g = parseInt(str[2] + str[2], 16)
    b = parseInt(str[3] + str[3], 16)
  } else {
    return color
  }
  return `rgba(${r}, ${g}, ${b}, ${alpha})`
}

// Align a set of series onto one shared x-axis.
// `series`: [{ key, label, points: [{ t: <ms>, v: Number|null }], color? }]
// Returns { xs, ys }: `xs` = sorted unique ms timestamps across ALL series; `ys` = one array per
// series of values aligned to `xs` (null where that series has no point at that x). Nulls in the
// input are preserved (a gap and an explicit null both read as null downstream).
export function alignSeries(series) {
  const list = series || []
  const xset = new Set()
  const maps = list.map((s) => {
    const m = new Map()
    for (const p of s.points || []) {
      const t = Number(p.t)
      xset.add(t)
      m.set(t, p.v == null ? null : Number(p.v))
    }
    return m
  })
  const xs = [...xset].sort((a, b) => a - b)
  const ys = maps.map((m) => xs.map((t) => (m.has(t) ? m.get(t) : null)))
  return { xs, ys }
}

// Indices where a value is non-null but BOTH neighbors are null/absent — a 1-vertex "run" that a
// line can't draw (it has no segment), so BaseChart dots these. Out-of-bounds neighbors (the
// first/last index) count as absent.
export function isolatedPointIndices(values) {
  const v = values || []
  const out = []
  for (let i = 0; i < v.length; i++) {
    if (v[i] == null) continue
    const left = i > 0 ? v[i - 1] : null
    const right = i < v.length - 1 ? v[i + 1] : null
    if (left == null && right == null) out.push(i)
  }
  return out
}

// Pre-accumulate stacked-bar baselines (uPlot has no native stacking).
// `buckets`: [{ t: <ms>, segments: [{ key, color, value }] }]; `keys`: bottom→top draw order.
// Returns { xs, stacks } where `xs` = the buckets' ms timestamps and `stacks[key]` is the running
// cumulative TOP of that key across buckets (so drawing each key as bars from 0 to its cumulative
// top, tallest last, yields a stack). A missing/nullish segment counts as 0.
export function stackSegments(buckets, keys) {
  const list = buckets || []
  const order = keys || []
  const xs = list.map((b) => Number(b.t))
  const stacks = {}
  for (const k of order) stacks[k] = []
  list.forEach((b, i) => {
    const byKey = new Map((b.segments || []).map((s) => [s.key, s]))
    let running = 0
    for (const k of order) {
      const seg = byKey.get(k)
      const val = seg && seg.value != null ? Number(seg.value) : 0
      running += Number.isFinite(val) ? val : 0
      stacks[k][i] = running
    }
  })
  return { xs, stacks }
}

// Themed axes (x + y) shared by both builders. `formatValue` renders y ticks; x ticks default to
// clock labels unless `xValues` overrides the x-axis `values` fn (e.g. a duration axis). `xSplits`
// optionally overrides where x ticks/gridlines land (e.g. clean decades on a log axis). Grid lines
// are dashed and colored from the theme.
function themedAxes(theme, formatValue, xValues, xSplits) {
  const t = theme || {}
  const grid = { stroke: t.grid, dash: [3, 4], width: 1 }
  const ticks = { stroke: t.grid, width: 1 }
  return [
    {
      stroke: t.axis,
      grid,
      ticks,
      values: xValues ?? ((u, splits) => splits.map((sec) => clockLabel(sec))),
      ...(xSplits ? { splits: xSplits } : {}),
    },
    {
      stroke: t.axis,
      grid,
      ticks,
      values: (u, splits) => splits.map((v) => formatValue(v)),
    },
  ]
}

// A canvas linear-gradient fill (series color at ~0.26 alpha → transparent) for the area under
// the lead line. Returns a *function* uPlot calls at draw time with the live instance; it degrades
// to a flat translucent color when there's no canvas context (jsdom / no ctx), so it never throws.
function areaFill(color) {
  return (u) => {
    const ctx = u && u.ctx
    if (!ctx || typeof ctx.createLinearGradient !== 'function' || !u.bbox) {
      return withAlpha(color, 0.26)
    }
    const { top, height } = u.bbox
    const g = ctx.createLinearGradient(0, top, 0, top + height)
    g.addColorStop(0, withAlpha(color, 0.26))
    g.addColorStop(1, withAlpha(color, 0))
    return g
  }
}

// Build uPlot line/area { data, opts, tooltipData }.
// `uPlot` is the loaded module (passed in — this file never imports it). `series` is the public
// LineChart shape; `startMs`/`endMs` fix the x window; `formatValue` formats y ticks/tooltips;
// `area` enables the gradient fill (under the LEAD series when unstacked, under EVERY band when
// stacked); `stacked` accumulates the series into cumulative bands (a filled TOTAL); `theme`
// supplies axis colors; `highlightKey` dims every series whose key !== highlightKey.
//
// `tooltipData` is the RAW (de-stacked) per-series values, one array per y-series in the SAME order
// as `data`'s y-series — so BaseChart's tooltip shows each signal's own value, not its cumulative
// top. It is `null` for the non-stacked path (there the raw values ARE `data`, so BaseChart falls
// back to `u.data`).
export function buildLineOptions({
  uPlot,
  series,
  startMs,
  endMs,
  formatValue = formatNumber,
  area = false,
  stacked = false,
  theme,
  highlightKey = null,
  compact = false, // mini/sparkline mode: hide axes + gridlines and shrink padding to just the trace
  yLog = false,
}) {
  const list = series || []
  const { xs, ys } = alignSeries(list)
  const spline = uPlot.paths.spline()

  // `order` is the sequence of list indices in DRAW (== data/series/tooltipData) order.
  //  - Unstacked: natural order (series[0] first) — unchanged legacy behaviour.
  //  - Stacked: TOP-DOWN — the grand total first (drawn at the BACK) down to the bottom band last
  //    (drawn in FRONT). uPlot draws series in array order and every area fill drops to the zero
  //    baseline, so emitting the largest cumulative first lets each smaller band paint over the
  //    larger one behind it ("smaller cumulative sits in front of larger").
  let order
  let data
  let tooltipData = null

  if (stacked) {
    // Cumulative running sums (nulls → 0), same accumulation as stackSegments: cum[k] = ys[0..k].
    // cum[last] is the grand total (the topmost band).
    const n = list.length
    const cum = ys.map(() => xs.map(() => 0))
    for (let x = 0; x < xs.length; x++) {
      let running = 0
      for (let k = 0; k < n; k++) {
        const v = ys[k][x]
        running += v == null ? 0 : v
        cum[k][x] = running
      }
    }
    order = list.map((_, i) => i).reverse() // total (last) first → bottom (first) last
    data = [xs.map(msToSec), ...order.map((i) => cum[i])]
    // Raw de-stacked values, SAME order as data's y-series (nulls preserved so the tooltip skips a
    // signal with no sample at that x). BaseChart reads props.tooltipData[i] for data's y-series i.
    tooltipData = order.map((i) => ys[i])
  } else {
    order = list.map((_, i) => i)
    data = [xs.map(msToSec), ...ys]
  }

  const opts = {
    scales: {
      // A range FUNCTION (not a static array) reliably PINS the x window regardless of the data's own
      // extent. uPlot treats a static `[min,max]` array as a starting hint and still auto-fits to the
      // data — so a 24h window holding only a few recent minutes of points would collapse the axis to
      // those minutes. Returning a constant [min,max] ignores dataMin/dataMax and holds the real window.
      x: { time: true, range: () => [msToSec(startMs), msToSec(endMs)] },
    },
    // series[0] is uPlot's implicit x series; one entry per y-series follows (in `order`).
    series: [
      {},
      ...order.map((i, pos) => {
        const s = list[i]
        // A lone series has no one to be told apart from, so it reads as "the" metric — default
        // it to the brand cyan instead of the hashed multi-series identity colour, which only
        // earns its keep once there's more than one series on the chart. Explicit `s.color`
        // (severity tones, monochrome-by-design charts, etc.) always wins over either default.
        const base = s.color ?? (list.length === 1 ? theme?.brand : undefined) ?? seriesColor(s.key).stroke
        const dim = highlightKey != null && s.key !== highlightKey
        const out = {
          label: s.label ?? s.key,
          stroke: dim ? withAlpha(base, 0.3) : base,
          width: 2.2,
          cap: 'round',
          paths: spline,
          points: { show: false },
        }
        if (stacked) {
          // Fill every band with its OWN colour gradient when area; plain stacked lines otherwise.
          if (area) out.fill = areaFill(base)
        } else if (area && pos === 0) {
          // Unstacked: gradient under the lead/single series only (grouped multi-series = no mud).
          out.fill = areaFill(base)
        }
        return out
      }),
    ],
    axes: compact ? [{ show: false }, { show: false }] : themedAxes(theme, formatValue),
    legend: { show: false },
    cursor: { drag: { x: true, y: false, setScale: false }, points: { show: false } },
    padding: compact ? [8, 6, 2, 6] : [10, 12, 4, 6],
  }

  // Optional base-10 log y-scale (long-tailed metrics). Log can't plot <=0, so clamp the low bound
  // above zero; keep the top at the data max. A range FUNCTION pins it (a static array only hints).
  if (yLog) {
    opts.scales.y = {
      distr: 3,
      range: (u, dataMin, dataMax) => {
        const hi = dataMax != null && dataMax > 0 ? dataMax : 1
        const lo = dataMin != null && dataMin > 0 ? dataMin : hi / 1000
        return [Math.max(lo, 1e-9), hi]
      },
    }
  }

  return { data, opts, tooltipData }
}

// Collect the distinct segment keys in first-seen (bottom→top) order, remembering each key's
// color. Returns a Map<key, color> so insertion order == draw order.
function orderedSegmentKeys(buckets) {
  const seen = new Map()
  for (const b of buckets || []) {
    for (const seg of b.segments || []) {
      if (!seen.has(seg.key)) seen.set(seg.key, seg.color)
    }
  }
  return seen
}

// A rounded, airy bars path (bar ≈ 60% of slot). `radius` gives the rounded tops (supported by
// the pinned uPlot build).
function barsPaths(uPlot) {
  return uPlot.paths.bars({ size: [0.6, Infinity], gap: 2, radius: 0.25 })
}

// The x-scale for a value (non-time) axis, e.g. a duration histogram. `log` switches to uPlot's
// base-10 log distribution (`distr: 3`) — the right axis for a long-tailed latency histogram whose
// buckets are GEOMETRIC (each an equal ratio wider): on a log axis those exponentially-spaced
// buckets render evenly spaced, so the bars come out uniform width and a single slow outlier is one
// small bar at the right instead of an empty tail stretching the whole axis. Both modes pin the
// extent with a range FUNCTION — a static [min,max] array only HINTS uPlot, which then auto-fits to
// the data (see the buildLineOptions note). A log axis needs strictly-positive bounds, so it pads by
// half a geometric step on each side (bars centre on their x, so this keeps the first/last bar from
// clipping at the plot edge) and falls back to a positive [1,10] when empty.
function valueXScale(xs, log) {
  if (!log) {
    return { time: false, range: () => (xs.length ? [xs[0], xs[xs.length - 1]] : [0, 1]) }
  }
  return {
    time: false,
    distr: 3,
    range: () => {
      if (!xs.length) return [1, 10]
      const lo = xs[0]
      const hi = xs[xs.length - 1]
      // Geometric half-step padding: buckets are a fixed ratio apart, so xs[1]/xs[0] is that ratio;
      // a lone bucket falls back to a decade. Clamp the low edge above 0 (log can't plot 0/negatives).
      const step = xs.length > 1 && xs[0] > 0 ? xs[1] / xs[0] : 10
      const pad = Math.sqrt(step)
      return [Math.max(lo / pad, 1e-9), hi * pad]
    },
  }
}

// Clean decade tick positions for a log axis: the powers of ten (…1µs=1e3, 10µs=1e4, 1ms=1e6…) that
// fall within [min,max]. Passing uPlot explicit splits makes it draw exactly ONE tick per decade —
// round labels and NO 2×–9× minor-tick "stripes" (uPlot's default log axis subdivides every decade
// and draws a bare dash for each, which reads as noise). A narrow span (≤ 2 decades) also gets the
// 2× / 5× subdivisions so it isn't left with a single lonely label.
function logDecadeSplits(min, max) {
  if (!(min > 0) || !(max > min)) return [min, max]
  const loE = Math.floor(Math.log10(min))
  const hiE = Math.ceil(Math.log10(max))
  const mantissas = hiE - loE <= 2 ? [1, 2, 5] : [1]
  const out = []
  for (let e = loE; e <= hiE; e++) {
    const decade = Math.pow(10, e)
    for (const m of mantissas) {
      const v = m * decade
      if (v >= min && v <= max) out.push(v)
    }
  }
  return out.length ? out : [min, max]
}

// Build uPlot bar/stacked/histogram { data, opts, tooltipData }.
// `buckets` is the public BarChart shape. When `stacked`, segment values are pre-accumulated into
// cumulative baselines (via `stackSegments`) and each key is drawn as bars to its cumulative top;
// otherwise each key is drawn raw from 0. `tooltipData` holds the RAW per-key values (same order as
// `data`'s y-series) for stacked charts so a hovered segment reads its own value, not its cumulative
// top; it is `null` when unstacked (there the raw values ARE `data`, so BaseChart uses `u.data`).
//
// `xUnit` picks what `bucket.t` means: `'time'` (default) is a UNIX-ms timestamp — converted to
// seconds, `scales.x.time = true`, axis ticks are clock labels (today's behavior, unchanged).
// `'value'` is an arbitrary numeric axis (e.g. a duration histogram) — `bucket.t` is used AS-IS
// (no ms→sec conversion), `scales.x.time = false`, min/max come from the first/last x value, and
// axis ticks are rendered via `xFormat` (falls back to `String` when omitted).
export function buildBarOptions({
  uPlot,
  buckets,
  startMs,
  endMs,
  stacked = true,
  formatValue = formatNumber,
  theme,
  xUnit = 'time',
  xFormat,
  xLog = false,
}) {
  const list = buckets || []
  const keyColors = orderedSegmentKeys(list)
  const keys = [...keyColors.keys()] // natural bottom→top order
  const colorOf = (k) => keyColors.get(k) ?? seriesColor(k).stroke

  const rawXs = list.map((b) => Number(b.t))
  const xs = xUnit === 'value' ? rawXs : rawXs.map(msToSec)
  // Raw (pre-accumulation) per-key heights, one array per key aligned to xs, in natural order.
  const rawArrays = keys.map((k) =>
    list.map((b) => {
      const seg = (b.segments || []).find((s) => s.key === k)
      return seg && seg.value != null ? Number(seg.value) : 0
    }),
  )

  // DRAW order. uPlot paints each bar series in array order and every bar drops to the y=0 baseline
  // (overlapping, not auto-stacked), so a stack must be painted LARGEST cumulative FIRST — the grand
  // total at the back, each smaller cumulative in front of it. Emitting natural order instead would
  // draw the total last, on top, covering the whole bar in a single colour. Unstacked = natural.
  const idx = keys.map((_, i) => i)
  const order = stacked ? [...idx].reverse() : idx

  let yArrays
  let tooltipData = null
  if (stacked) {
    const { stacks } = stackSegments(list, keys) // cumulative tops per key (natural order)
    const cum = keys.map((k) => stacks[k])
    yArrays = order.map((i) => cum[i]) // total first → bottom band last
    // Raw de-stacked values, SAME order as data's y-series, so a hovered segment reads its own value.
    tooltipData = order.map((i) => rawArrays[i])
  } else {
    yArrays = order.map((i) => rawArrays[i])
  }
  const data = [xs, ...yArrays]

  const bars = barsPaths(uPlot)
  // A range FUNCTION pins the axis (a static array lets uPlot auto-fit to the data instead): time
  // mode holds the real window; value mode holds the bucket extent (linear or, when `xLog`, base-10
  // log — see valueXScale). `xLog` only applies in value mode (a time axis is always linear).
  const xScale =
    xUnit === 'value'
      ? valueXScale(xs, xLog)
      : { time: true, range: () => [msToSec(startMs), msToSec(endMs)] }
  // On a log axis the ticks land on clean decades (1µs, 10µs, 1ms…), but formatDuration renders those
  // as "10.0µs" / "1.00s" — strip the trailing-zero decimals so decade labels read crisply. Linear
  // value ticks keep the raw format (they can legitimately be fractional).
  const rawTick = (v) => (xFormat ? xFormat(v) : String(v))
  const tick = xUnit === 'value' && xLog ? (v) => rawTick(v).replace(/\.0+(?=\D)/, '') : rawTick
  const xValues = xUnit === 'value' ? (u, splits) => splits.map(tick) : undefined
  // Log axis: pin ticks/gridlines to clean decades (kills the noisy 2×–9× minor-tick dashes).
  const xSplits = xUnit === 'value' && xLog ? (u, i, sMin, sMax) => logDecadeSplits(sMin, sMax) : undefined

  const opts = {
    scales: {
      x: xScale,
      // Bars grow from a zero baseline, so the y-scale MUST include 0 — uPlot's default auto-range
      // starts near the data's min, which leaves bars drawn from off-canvas and stretching past the
      // top. Anchor at 0 with a little headroom; `dataMax` here is the tallest (cumulative) bar.
      y: { range: (u, dataMin, dataMax) => [0, dataMax != null && dataMax > 0 ? dataMax * 1.02 : 1] },
    },
    series: [
      {},
      ...order.map((i) => {
        const k = keys[i]
        const color = colorOf(k)
        return { label: k, stroke: color, fill: color, paths: bars, points: { show: false } }
      }),
    ],
    axes: themedAxes(theme, formatValue, xValues, xSplits),
    legend: { show: false },
    cursor: { drag: { x: true, y: false, setScale: false }, points: { show: false } },
    padding: [10, 8, 4, 6],
  }
  return { data, opts, tooltipData }
}

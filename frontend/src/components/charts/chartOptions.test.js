// frontend/src/components/charts/chartOptions.test.js
// Table tests for the pure charting core — no real uPlot, no canvas. The builders take the uPlot
// module as an argument, so we hand them a tiny fake whose path factories return sentinels.
import { describe, it, expect, vi } from 'vitest'
import {
  msToSec,
  alignSeries,
  isolatedPointIndices,
  stackSegments,
  buildLineOptions,
  buildBarOptions,
} from './chartOptions.js'
import { seriesColor } from '@/lib/core/seriesColor'

// Stand-in for the loaded uPlot module — only the path factories are exercised by the builders.
const fakeUplot = { paths: { spline: () => 'SPLINE', bars: () => 'BARS' } }

describe('msToSec', () => {
  it('converts ms → seconds (fractional preserved)', () => {
    expect(msToSec(1000)).toBe(1)
    expect(msToSec(1500)).toBe(1.5)
    expect(msToSec(0)).toBe(0)
  })
})

describe('alignSeries', () => {
  it('builds a sorted union of xs and aligns values, null-filling gaps', () => {
    const { xs, ys } = alignSeries([
      { key: 'a', points: [{ t: 30, v: 3 }, { t: 10, v: 1 }] },
      { key: 'b', points: [{ t: 20, v: 2 }, { t: 10, v: 9 }] },
    ])
    expect(xs).toEqual([10, 20, 30]) // union + sorted ascending
    expect(ys[0]).toEqual([1, null, 3]) // 'a' has no point at t=20
    expect(ys[1]).toEqual([9, 2, null]) // 'b' has no point at t=30
  })

  it('preserves explicit nulls in the input', () => {
    const { ys } = alignSeries([{ key: 'a', points: [{ t: 10, v: null }, { t: 20, v: 5 }] }])
    expect(ys[0]).toEqual([null, 5])
  })

  it('handles an empty series list', () => {
    expect(alignSeries([])).toEqual({ xs: [], ys: [] })
    expect(alignSeries(undefined)).toEqual({ xs: [], ys: [] })
  })
})

describe('isolatedPointIndices', () => {
  it('flags endpoints, interior isolates and skips real runs', () => {
    // idx: 0=5 (isolated, edge), 2=3 (isolated interior), 5=7 (isolated, edge)
    expect(isolatedPointIndices([5, null, 3, null, null, 7])).toEqual([0, 2, 5])
  })

  it('does not flag values that have a non-null neighbor', () => {
    expect(isolatedPointIndices([1, 2, null])).toEqual([]) // 1-2 form a drawable segment
    expect(isolatedPointIndices([null, 2, 3, null])).toEqual([])
  })

  it('flags a lone single point', () => {
    expect(isolatedPointIndices([9])).toEqual([0])
    expect(isolatedPointIndices([null, 9, null])).toEqual([1])
  })

  it('is safe for empty/absent input', () => {
    expect(isolatedPointIndices([])).toEqual([])
    expect(isolatedPointIndices(undefined)).toEqual([])
  })
})

describe('stackSegments', () => {
  it('accumulates cumulative tops through the key order', () => {
    const buckets = [
      { t: 100, segments: [{ key: 'ok', value: 2 }, { key: 'error', value: 3 }] },
      { t: 200, segments: [{ key: 'ok', value: 5 }, { key: 'error', value: 1 }] },
    ]
    const { xs, stacks } = stackSegments(buckets, ['ok', 'error'])
    expect(xs).toEqual([100, 200])
    expect(stacks.ok).toEqual([2, 5]) // bottom layer == its own value
    expect(stacks.error).toEqual([5, 6]) // top == running sum (ok + error)
  })

  it('treats a missing/nullish segment as 0', () => {
    const buckets = [
      { t: 1, segments: [{ key: 'ok', value: 4 }] }, // no 'error' segment
      { t: 2, segments: [{ key: 'ok', value: null }, { key: 'error', value: 7 }] },
    ]
    const { stacks } = stackSegments(buckets, ['ok', 'error'])
    expect(stacks.ok).toEqual([4, 0]) // null → 0
    expect(stacks.error).toEqual([4, 7]) // missing 'error' → same as 'ok' baseline
  })

  it('extracts xs and returns an entry per key', () => {
    const { xs, stacks } = stackSegments([{ t: 42, segments: [] }], ['a', 'b'])
    expect(xs).toEqual([42])
    expect(stacks.a).toEqual([0])
    expect(stacks.b).toEqual([0])
  })
})

describe('buildLineOptions', () => {
  const series = [
    { key: 'a', label: 'A', points: [{ t: 1000, v: 1 }, { t: 2000, v: 2 }] },
    { key: 'b', label: 'B', color: '#123456', points: [{ t: 2000, v: 9 }] },
  ]

  it('emits data[0] as xs in SECONDS with nulls preserved and one series entry per y + x', () => {
    const { data, opts } = buildLineOptions({
      uPlot: fakeUplot,
      series,
      startMs: 1000,
      endMs: 2000,
    })
    expect(data[0]).toEqual([1, 2]) // ms → sec
    expect(data[1]).toEqual([1, 2]) // series 'a' aligned
    expect(data[2]).toEqual([null, 9]) // series 'b' null-filled at t=1000
    expect(opts.series).toHaveLength(series.length + 1) // + implicit x series
  })

  it('uses provided color over seriesColor, spline paths, width 2.2, no points', () => {
    const { opts } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000 })
    expect(opts.series[1].stroke).toBe(seriesColor('a').stroke) // no color → hashed identity
    expect(opts.series[2].stroke).toBe('#123456') // provided color wins
    expect(opts.series[1].paths).toBe('SPLINE')
    expect(opts.series[1].width).toBe(2.2)
    expect(opts.series[1].points).toEqual({ show: false })
  })

  it('sets round caps on every line series', () => {
    const { opts } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000 })
    expect(opts.series[1].cap).toBe('round')
    expect(opts.series[2].cap).toBe('round')
  })

  it('adds a gradient fill fn on the LEAD series only when area=true', () => {
    const { opts } = buildLineOptions({
      uPlot: fakeUplot,
      series,
      startMs: 1000,
      endMs: 2000,
      area: true,
    })
    expect(typeof opts.series[1].fill).toBe('function')
    expect(opts.series[2].fill).toBeUndefined()
  })

  it('omits fills entirely when area=false', () => {
    const { opts } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000 })
    expect(opts.series[1].fill).toBeUndefined()
    expect(opts.series[2].fill).toBeUndefined()
  })

  it('dims non-highlighted series strokes', () => {
    const { opts } = buildLineOptions({
      uPlot: fakeUplot,
      series,
      startMs: 1000,
      endMs: 2000,
      highlightKey: 'a',
    })
    expect(opts.series[1].stroke).toBe(seriesColor('a').stroke) // highlighted → full color
    expect(opts.series[2].stroke).toMatch(/^rgba\(/) // dimmed → translucent
  })

  it('pins the x window (range fn) in SECONDS', () => {
    const { opts } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 4000 })
    // A range fn (not a static array) so uPlot holds the window instead of auto-fitting to data.
    expect(typeof opts.scales.x.range).toBe('function')
    expect(opts.scales.x.range()).toEqual([1, 4])
  })

  it('routes the y-axis values through formatValue', () => {
    const formatValue = vi.fn((v) => `<${v}>`)
    const { opts } = buildLineOptions({
      uPlot: fakeUplot,
      series,
      startMs: 1000,
      endMs: 2000,
      formatValue,
    })
    const rendered = opts.axes[1].values({}, [10, 20])
    expect(rendered).toEqual(['<10>', '<20>'])
    expect(formatValue).toHaveBeenCalledWith(10)
  })

  it('disables the built-in legend and sets a non-rescaling drag cursor', () => {
    const { opts } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000 })
    expect(opts.legend).toEqual({ show: false })
    expect(opts.cursor.drag).toEqual({ x: true, y: false, setScale: false })
  })

  it('unstacked path returns null tooltipData (data already holds raw values)', () => {
    const { tooltipData } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000 })
    expect(tooltipData).toBeNull()
  })
})

describe('buildLineOptions (stacked)', () => {
  const series = [
    { key: 'a', label: 'A', points: [{ t: 1000, v: 1 }, { t: 2000, v: 2 }] },
    { key: 'b', label: 'B', points: [{ t: 1000, v: 10 }, { t: 2000, v: 20 }] },
  ]

  it('accumulates into cumulative bands drawn top-down (total is the first/topmost y-series)', () => {
    const { data } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000, stacked: true })
    expect(data[0]).toEqual([1, 2]) // xs ms → sec
    expect(data[1]).toEqual([11, 22]) // grand total (a+b) — topmost band, drawn first (behind)
    expect(data[2]).toEqual([1, 2]) //  bottom band 'a' — drawn last (in front)
  })

  it('returns RAW de-stacked tooltipData in the SAME order as data y-series', () => {
    const { tooltipData } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000, stacked: true })
    expect(tooltipData[0]).toEqual([10, 20]) // 'b' raw — its band tops the stack (paired with data[1])
    expect(tooltipData[1]).toEqual([1, 2]) //  'a' raw (paired with data[2])
  })

  it('fills EVERY band with a gradient fn when stacked && area', () => {
    const { opts } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000, stacked: true, area: true })
    expect(typeof opts.series[1].fill).toBe('function')
    expect(typeof opts.series[2].fill).toBe('function')
  })

  it('draws stacked lines with no fill when area=false', () => {
    const { opts } = buildLineOptions({ uPlot: fakeUplot, series, startMs: 1000, endMs: 2000, stacked: true })
    expect(opts.series[1].fill).toBeUndefined()
    expect(opts.series[2].fill).toBeUndefined()
  })

  it('treats nulls as 0 for accumulation but preserves them in tooltipData', () => {
    const gappy = [
      { key: 'a', points: [{ t: 1000, v: null }, { t: 2000, v: 4 }] },
      { key: 'b', points: [{ t: 1000, v: 3 }, { t: 2000, v: 5 }] },
    ]
    const { data, tooltipData } = buildLineOptions({ uPlot: fakeUplot, series: gappy, startMs: 1000, endMs: 2000, stacked: true })
    expect(data[1]).toEqual([3, 9]) // total: (null→0)+3 ; 4+5
    expect(data[2]).toEqual([0, 4]) // 'a' cumulative: null→0, then 4
    expect(tooltipData[1]).toEqual([null, 4]) // 'a' RAW keeps the null so the tooltip skips it
  })
})

describe('buildBarOptions', () => {
  const buckets = [
    { t: 1000, segments: [{ key: 'ok', color: '#0a0', value: 2 }, { key: 'error', color: '#f00', value: 1 }] },
    { t: 2000, segments: [{ key: 'ok', color: '#0a0', value: 4 }, { key: 'error', color: '#f00', value: 3 }] },
  ]

  it('emits stacked (cumulative) data with xs in seconds, painted largest-first', () => {
    const { data } = buildBarOptions({ uPlot: fakeUplot, buckets, startMs: 1000, endMs: 2000 })
    expect(data[0]).toEqual([1, 2]) // xs ms → sec
    // Stacked bars are drawn total-first (back) → bottom band last (front): reversed vs natural.
    expect(data[1]).toEqual([3, 7]) // 'error' cumulative top (ok + error) — the total, drawn behind
    expect(data[2]).toEqual([2, 4]) // 'ok' cumulative — drawn in front
  })

  it('emits raw (non-cumulative) data when stacked=false', () => {
    const { data } = buildBarOptions({
      uPlot: fakeUplot,
      buckets,
      startMs: 1000,
      endMs: 2000,
      stacked: false,
    })
    expect(data[1]).toEqual([2, 4]) // 'ok' raw
    expect(data[2]).toEqual([1, 3]) // 'error' raw, not accumulated
  })

  it('returns RAW de-stacked tooltipData aligned to the drawn (reversed) y-series order', () => {
    const { data, tooltipData } = buildBarOptions({ uPlot: fakeUplot, buckets, startMs: 1000, endMs: 2000 })
    // Drawn order is reversed for stacking: y-series 1 = 'error' (cumulative total), 2 = 'ok'.
    expect(data[1]).toEqual([3, 7]) // 'error' CUMULATIVE top lives in data
    expect(tooltipData[0]).toEqual([1, 3]) // 'error' RAW (its own values, not the cumulative 3,7)
    expect(tooltipData[1]).toEqual([2, 4]) // 'ok' raw
  })

  it('omits tooltipData (null) when unstacked — data already holds raw values', () => {
    const { tooltipData } = buildBarOptions({ uPlot: fakeUplot, buckets, startMs: 1000, endMs: 2000, stacked: false })
    expect(tooltipData).toBeNull()
  })

  it('assigns segment colors and the bars path per key', () => {
    const { opts } = buildBarOptions({ uPlot: fakeUplot, buckets, startMs: 1000, endMs: 2000 })
    expect(opts.series).toHaveLength(3) // x + error + ok (stacked = reversed draw order)
    expect(opts.series[1].fill).toBe('#f00') // 'error' drawn first (behind)
    expect(opts.series[2].fill).toBe('#0a0') // 'ok' drawn in front
    expect(opts.series[1].paths).toBe('BARS')
  })

  it('falls back to seriesColor when a segment has no color', () => {
    const noColor = [{ t: 1000, segments: [{ key: 'lonely', value: 5 }] }]
    const { opts } = buildBarOptions({ uPlot: fakeUplot, buckets: noColor, startMs: 1000, endMs: 2000 })
    expect(opts.series[1].fill).toBe(seriesColor('lonely').stroke)
  })

  it('themes axes, disables the legend and sets a drag cursor', () => {
    const theme = { axis: '#aaa', grid: '#ddd' }
    const { opts } = buildBarOptions({
      uPlot: fakeUplot,
      buckets,
      startMs: 1000,
      endMs: 2000,
      theme,
    })
    expect(opts.axes[0].stroke).toBe('#aaa')
    expect(opts.axes[1].grid.stroke).toBe('#ddd')
    expect(opts.axes[1].grid.dash).toEqual([3, 4])
    expect(opts.legend).toEqual({ show: false })
    expect(opts.cursor.drag).toEqual({ x: true, y: false, setScale: false })
  })

  it('defaults to time mode: xs in seconds, scales.x.time=true, clock-labeled x-axis', () => {
    const { data, opts } = buildBarOptions({ uPlot: fakeUplot, buckets, startMs: 1000, endMs: 2000 })
    expect(data[0]).toEqual([1, 2]) // ms → sec
    expect(opts.scales.x.time).toBe(true)
    expect(opts.scales.x.range()).toEqual([1, 2]) // window pinned via range fn
  })

  it('xUnit: "value" leaves x data un-scaled, disables scales.x.time and formats ticks via xFormat', () => {
    const xFormat = vi.fn((v) => `${v}ns`)
    const { data, opts } = buildBarOptions({
      uPlot: fakeUplot,
      buckets,
      startMs: 1000,
      endMs: 2000,
      xUnit: 'value',
      xFormat,
    })
    expect(data[0]).toEqual([1000, 2000]) // NOT divided by 1000 — raw bucket.t values preserved
    expect(opts.scales.x.time).toBe(false)
    expect(opts.scales.x.range()).toEqual([1000, 2000]) // derived from first/last x value
    const rendered = opts.axes[0].values({}, [1000, 2000])
    expect(rendered).toEqual(['1000ns', '2000ns'])
    expect(xFormat).toHaveBeenCalledWith(1000)
  })

  it('xUnit: "value" falls back to String when xFormat is omitted', () => {
    const { opts } = buildBarOptions({
      uPlot: fakeUplot,
      buckets,
      startMs: 1000,
      endMs: 2000,
      xUnit: 'value',
    })
    expect(opts.axes[0].values({}, [1000, 2000])).toEqual(['1000', '2000'])
  })

  it('xUnit: "value" + xLog uses a base-10 log x-scale (distr:3) with a positive padded range', () => {
    const { opts } = buildBarOptions({
      uPlot: fakeUplot,
      buckets,
      startMs: 1000,
      endMs: 2000,
      xUnit: 'value',
      xLog: true,
    })
    expect(opts.scales.x.time).toBe(false)
    expect(opts.scales.x.distr).toBe(3) // uPlot log distribution — geometric buckets render evenly spaced
    const [lo, hi] = opts.scales.x.range()
    const pad = Math.sqrt(2000 / 1000) // geometric half-step derived from the two buckets' ratio
    expect(lo).toBeCloseTo(1000 / pad) // padded below the first bucket, still strictly > 0 for a log axis
    expect(hi).toBeCloseTo(2000 * pad) // padded above the last bucket
    expect(lo).toBeGreaterThan(0)
  })

  it('xUnit: "value" without xLog stays linear (no distr)', () => {
    const { opts } = buildBarOptions({ uPlot: fakeUplot, buckets, startMs: 1000, endMs: 2000, xUnit: 'value' })
    expect(opts.scales.x.distr).toBeUndefined()
  })

  it('log axis pins ticks to clean decades and strips trailing-zero duration labels', () => {
    // Stub xFormat mimics formatDuration's ".0"/".00" tails on exact decade values.
    const xFormat = (v) => ({ 10000: '10.0µs', 1000000: '1.0ms', 1000000000: '1.00s' })[v] ?? String(v)
    const { opts } = buildBarOptions({
      uPlot: fakeUplot,
      buckets,
      startMs: 1000,
      endMs: 2000,
      xUnit: 'value',
      xLog: true,
      xFormat,
    })
    // Explicit decade splits over 1µs..1s → one tick per power of ten (no 2×–9× minor-tick stripes).
    const splits = opts.axes[0].splits({}, 0, 1_000, 1_000_000_000)
    expect(splits).toEqual([1_000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000, 1_000_000_000])
    // Decade labels render crisply: the trailing-zero decimals are stripped.
    expect(opts.axes[0].values({}, [10_000, 1_000_000, 1_000_000_000])).toEqual(['10µs', '1ms', '1s'])
  })
})

const U = { paths: { spline: () => null, bars: () => null } }

describe('buildLineOptions yLog', () => {
  const series = [{ key: 'a', label: 'a', points: [{ t: 1000, v: 5 }, { t: 2000, v: 50 }] }]

  it('leaves the y-scale linear by default (no distr)', () => {
    const { opts } = buildLineOptions({ uPlot: U, series, startMs: 1000, endMs: 2000 })
    expect(opts.scales.y).toBeUndefined()
  })
  it('sets a base-10 log y-scale with a strictly positive range when yLog is on', () => {
    const { opts } = buildLineOptions({ uPlot: U, series, startMs: 1000, endMs: 2000, yLog: true })
    expect(opts.scales.y.distr).toBe(3)
    const [lo, hi] = opts.scales.y.range(null, 0, 50)
    expect(lo).toBeGreaterThan(0)
    expect(hi).toBeGreaterThanOrEqual(50)
  })
})

describe('buildLineOptions yRange', () => {
  it('yRange pins the y scale to the given bounds', () => {
    const { opts } = buildLineOptions({
      uPlot: fakeUplot,
      series: [{ key: 'a', points: [{ t: 0, v: 0.4 }, { t: 60_000, v: 0.6 }] }],
      startMs: 0,
      endMs: 60_000,
      yRange: [0, 100],
    })
    expect(opts.scales.y.range()).toEqual([0, 100])
  })

  it('y axis size grows to fit the widest tick label', () => {
    const { opts } = buildLineOptions({
      uPlot: fakeUplot,
      series: [{ key: 'a', points: [{ t: 0, v: 1 }] }],
      startMs: 0,
      endMs: 60_000,
      theme: {},
    })
    const yAxis = opts.axes[1]
    const fakeU = { ctx: { measureText: (s) => ({ width: s.length * 7 }) } }
    // "12,000 By/s" (11 chars * 7px = 77px) must not be clamped to the 50px default.
    expect(yAxis.size(fakeU, ['12,000 By/s'], 1)).toBeGreaterThan(77 / (globalThis.devicePixelRatio || 1))
    expect(yAxis.size(fakeU, null, 1)).toBe(50)
  })
})

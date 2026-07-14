// Viz registry + pure helpers for the metrics chart. No Vue reactivity, no DOM. Icon components
// are Lucide (cosmetic — if an import name ever changes, swap for any present icon).
import type { Component } from 'vue'
import { LineChart, AreaChart, Layers, BarChart3, Hash, Table } from 'lucide-vue-next'
import { seriesColor, seriesLabelKey } from '@/lib/core/seriesColor'

export interface VizDef { id: string; label: string; icon?: Component } // icon optional; consumers guard

export const ALL_VIZ: VizDef[] = [
  { id: 'line', label: 'Line', icon: LineChart },
  { id: 'area', label: 'Area', icon: AreaChart },
  { id: 'stacked', label: 'Stacked', icon: Layers },
  { id: 'bar', label: 'Bar', icon: BarChart3 },
  { id: 'stacked-bar', label: 'Stacked bar', icon: BarChart3 },
  { id: 'stat', label: 'Stat', icon: Hash },
  { id: 'table', label: 'Table', icon: Table },
]

const VIZ_IDS = new Set(ALL_VIZ.map((v) => v.id))
export const DEFAULT_VIZ = 'line'

// eslint-disable-next-line @typescript-eslint/no-unused-vars
export function defaultVizForType(_type: string): string {
  return DEFAULT_VIZ
}

// stat and plain bar need a single series (stat = one big number; bar has no side-by-side grouped
// layout, so multi-series bars would overlap — use stacked-bar instead). Everything else is always
// available.
export function availableViz(arg: { type?: string; seriesCount: number }): string[] {
  return ALL_VIZ.map((v) => v.id).filter((id) => (id === 'stat' || id === 'bar' ? arg.seriesCount <= 1 : true))
}

export function parseViz(str: string | null | undefined): string {
  return str && VIZ_IDS.has(str) ? str : DEFAULT_VIZ
}
export function serializeViz(id: string): string {
  return id && id !== DEFAULT_VIZ && VIZ_IDS.has(id) ? id : ''
}

export interface Series {
  labels: Record<string, string>
  points: { t: string | number; v: number | null }[]
}

// Hero = latest finite value of the (single) series; deltaPct = latest vs the window mean.
export function statSummary(series: Series[]): {
  hero: number | null
  deltaPct: number | null
  dir: 'up' | 'down' | 'flat'
} {
  const pts = series[0]?.points ?? []
  const vs = pts.map((p) => p.v).filter((v): v is number => v != null && Number.isFinite(v))
  if (!vs.length) return { hero: null, deltaPct: null, dir: 'flat' }
  const hero = vs[vs.length - 1]
  const mean = vs.reduce((a, b) => a + b, 0) / vs.length
  if (mean === 0) return { hero, deltaPct: null, dir: 'flat' }
  const deltaPct = ((hero - mean) / Math.abs(mean)) * 100
  const dir = deltaPct > 0.5 ? 'up' : deltaPct < -0.5 ? 'down' : 'flat'
  return { hero, deltaPct, dir }
}

export interface Bucket {
  t: number
  segments: { key: string; label: string; color: string; value: number | null }[]
}

// Metric series → BarChart buckets: one bucket per distinct timestamp (ns-string → ms), one
// segment per series. Mirrors MetricChart's `Number(p.t) / 1e6` time mapping.
export function seriesToBuckets(series: Series[]): Bucket[] {
  const byT = new Map<number, Bucket>()
  for (const s of series) {
    const key = seriesLabelKey(s.labels)
    const color = seriesColor(key).stroke
    for (const p of s.points) {
      const t = Number(p.t) / 1e6
      let b = byT.get(t)
      if (!b) {
        b = { t, segments: [] }
        byT.set(t, b)
      }
      b.segments.push({ key, label: key, color, value: p.v })
    }
  }
  return [...byT.values()].sort((a, b) => a.t - b.t)
}

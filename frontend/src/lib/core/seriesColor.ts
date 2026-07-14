// frontend/src/lib/seriesColor.js
// Series identity colors for the metrics chart. Mirrors serviceColor.js's stable hash AND hue
// order so a series grouped by `service` gets the same color as that service in the trace
// waterfall / service map. `stroke` is a real hex (for SVG polylines); `swatch` is the matching
// Tailwind bg class (for legend/tooltip chips). Class strings are literal for Tailwind purge safety.

interface PaletteColor {
  stroke: string
  swatch: string
}

interface SeriesColorResult extends PaletteColor {
  index: number
}

export const SERIES_PALETTE: PaletteColor[] = [
  { stroke: '#0ea5e9', swatch: 'bg-sky-500/70' },
  { stroke: '#8b5cf6', swatch: 'bg-violet-500/70' },
  { stroke: '#10b981', swatch: 'bg-emerald-500/70' },
  { stroke: '#f59e0b', swatch: 'bg-amber-500/70' },
  { stroke: '#f43f5e', swatch: 'bg-rose-500/70' },
  { stroke: '#06b6d4', swatch: 'bg-cyan-500/70' },
  { stroke: '#6366f1', swatch: 'bg-indigo-500/70' },
  { stroke: '#14b8a6', swatch: 'bg-teal-500/70' },
  { stroke: '#d946ef', swatch: 'bg-fuchsia-500/70' },
  { stroke: '#84cc16', swatch: 'bg-lime-500/70' },
]

export function seriesColor(key: string | null | undefined): SeriesColorResult {
  const s = key ?? ''
  let h = 0
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0
  const index = h % SERIES_PALETTE.length
  return { ...SERIES_PALETTE[index], index }
}

// The series-identity KEY (not the display label) that feeds seriesColor() above, plus the
// hover/highlight cross-link between MetricChart and MetricLegendTable. Shared by both — they
// used to keep byte-identical local copies (`labelKey`/`keyOf`) in sync only by comment.
export function seriesLabelKey(labels: Record<string, any> | null | undefined): string {
  const safeLabels = labels || {}
  const keys = Object.keys(safeLabels)
  if (!keys.length) return '(all)'
  return keys.map((k) => `${k}=${safeLabels[k]}`).join(', ')
}

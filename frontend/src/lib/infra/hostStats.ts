// Pure tile-derivation helpers for the /infra/:host glance layer: last-point extraction,
// worst-mountpoint pick, and the shared warn/error utilization thresholds. Kept free of Vue so
// the tile math is table-testable.

export interface SeriesLike {
  labels: Record<string, string | null>
  points: { t: string; v: number | null }[]
}

export function latestValue(s?: SeriesLike): number | null {
  if (!s) return null
  for (let i = s.points.length - 1; i >= 0; i--) {
    const v = s.points[i].v
    if (v != null && Number.isFinite(v)) return v
  }
  return null
}

export function latestTotal(list?: SeriesLike[]): number | null {
  const vals = (list ?? []).map(latestValue).filter((v): v is number => v != null)
  if (!vals.length) return null
  return vals.reduce((a, b) => a + b, 0)
}

export function worstSeries(
  list: SeriesLike[] | undefined,
  labelKey: string,
): { label: string; value: number } | null {
  let best: { label: string; value: number } | null = null
  for (const s of list ?? []) {
    const v = latestValue(s)
    if (v == null) continue
    if (!best || v > best.value) best = { label: s.labels[labelKey] ?? '', value: v }
  }
  return best
}

// Shared glance thresholds: ≥90% error, ≥80% warning, else no accent.
export function utilAccent(frac: number | null): 'error' | 'warning' | undefined {
  if (frac == null) return undefined
  if (frac >= 0.9) return 'error'
  if (frac >= 0.8) return 'warning'
  return undefined
}

// Worst of a set of utilization fractions through the shared utilAccent thresholds — used to derive
// a per-host status (fleet KPIs, host cards) without re-deriving the 80%/90% cutoffs.
export function hostStatus(
  utils: (number | null | undefined)[],
): 'error' | 'warning' | undefined {
  let worst: 'error' | 'warning' | undefined
  for (const u of utils) {
    if (u == null) continue
    const accent = utilAccent(u)
    if (accent === 'error') return 'error'
    if (accent === 'warning') worst = 'warning'
  }
  return worst
}

export function sparkValues(s?: SeriesLike): number[] {
  return (s?.points ?? []).map((p) => p.v).filter((v): v is number => v != null)
}

export function cpuSeriesForMode(
  list: SeriesLike[] | undefined,
  mode: 'total' | 'per-core',
): SeriesLike[] {
  const all = list ?? []
  return mode === 'total'
    ? all.filter((s) => s.labels.cpu === 'total')
    : all.filter((s) => s.labels.cpu !== 'total')
}

export function formatPct(frac: number | null): string {
  if (frac == null || !Number.isFinite(frac)) return '—'
  const pct = frac * 100
  return `${Math.abs(pct) < 10 && pct !== 0 ? pct.toFixed(1) : Math.round(pct)}%`
}

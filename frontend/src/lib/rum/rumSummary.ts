// Pure aggregation for the RUM executive summary (`/rum`). The view fans out one vitals / errors /
// pages query per registered app (see `rumQueries.js`), zips them into a `perApp` array of
// `{ app, vitals, errors, pages }`, and these helpers roll that up into fleet KPIs, a fleet-wide
// Core-Web-Vitals band, a ranked apps table, a cross-app "live issues" feed, and the slowest routes.
// No Vue, no I/O — table-testable. Rating cutoffs are NEVER hardcoded: every fleet rating is derived
// from the `good_max`/`poor_min` the API attached to each vitals row (same contract as the backend).

export type Rating = 'good' | 'needs' | 'poor'

export interface Dist {
  good: number
  needs: number
  poor: number
  total: number
}

export interface VitalRow {
  metric: string
  p75: number | null
  rating: Rating | null
  good_max: number | null
  poor_min: number | null
  dist: Dist
}

export interface ErrorIssue {
  fingerprint: string
  exception_type: string
  message: string
  count: number
  sessions: number
}

export interface PageRow {
  route: string
  pageviews: number
  lcp_p75: number | null
  inp_p75?: number | null
  cls_p75?: number | null
}

export interface AppData {
  app: string
  vitals: VitalRow[]
  errors: ErrorIssue[]
  pages: PageRow[]
}

export interface VitalCell {
  p75: number | null
  rating: Rating | null
}

export interface AppRow {
  app: string
  pageviews: number
  lcp: VitalCell | null
  inp: VitalCell | null
  cls: VitalCell | null
  errors: number
  sessions: number
  status: Rating | null
  _poor: number
  _needs: number
}

export interface FleetKpis {
  pageviews: number
  goodShare: number | null
  appsPassing: { passing: number; total: number }
  errors: number
  sessions: number
  slowestApp: { app: string; p75: number; rating: Rating | null } | null
}

export interface RouteRow {
  app: string
  route: string
  lcp_p75: number | null
  rating: Rating | null
}

export type Issue = ErrorIssue & { app: string }

export const CORE_VITALS = ['web_vitals.lcp', 'web_vitals.inp', 'web_vitals.cls']

export const VITAL_LABELS: Record<string, string> = {
  'web_vitals.lcp': 'LCP',
  'web_vitals.inp': 'INP',
  'web_vitals.cls': 'CLS',
  'web_vitals.fcp': 'FCP',
  'web_vitals.ttfb': 'TTFB',
}

export const VITAL_FULL: Record<string, string> = {
  'web_vitals.lcp': 'Largest Contentful Paint',
  'web_vitals.inp': 'Interaction to Next Paint',
  'web_vitals.cls': 'Cumulative Layout Shift',
  'web_vitals.fcp': 'First Contentful Paint',
  'web_vitals.ttfb': 'Time to First Byte',
}

// CLS is unit-less (2dp); time metrics read as seconds >=1s else whole ms (mirrors WebVitalScorecard).
export function formatVital(metric: string, value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return '—'
  if (metric === 'web_vitals.cls') return value.toFixed(2)
  if (value >= 1000) return (value / 1000).toFixed(1) + 's'
  return Math.round(value) + 'ms'
}

// Rating from the API-supplied thresholds — mirrors `vital_summary_json` in `photon-api/src/rum.rs`.
export function ratingFor(
  p75: number | null | undefined,
  goodMax: number | null | undefined,
  poorMin: number | null | undefined,
): Rating | null {
  if (p75 == null || !Number.isFinite(p75) || goodMax == null || poorMin == null) return null
  if (p75 <= goodMax) return 'good'
  if (p75 <= poorMin) return 'needs'
  return 'poor'
}

const RANK: Record<Rating, number> = { good: 0, needs: 1, poor: 2 }

// Worst (most severe) of a set of ratings; null when none are present.
export function worstRating(ratings: (Rating | null)[]): Rating | null {
  const present = ratings.filter((r): r is Rating => r != null)
  if (!present.length) return null
  return present.reduce((w, r) => (RANK[r] > RANK[w] ? r : w))
}

const vitalOf = (vitals: VitalRow[] | undefined, metric: string): VitalRow | null =>
  (vitals || []).find((v) => v.metric === metric) || null

const sum = <T>(arr: T[] | undefined, pick: (x: T) => number): number =>
  (arr || []).reduce((s, x) => s + (pick(x) || 0), 0)

// The three core-vital ratings for one app (null where the app reported no samples for that vital).
export function appCoreRatings(a: AppData): (Rating | null)[] {
  return CORE_VITALS.map((m) => vitalOf(a.vitals, m)?.rating ?? null)
}

// An app's overall health = the worst of its present core-vital ratings (null when it has none).
export function appWorstRating(a: AppData): Rating | null {
  return worstRating(appCoreRatings(a))
}

// Fleet-wide summary of ONE vital across all apps: summed good/needs/poor distribution (exact) and a
// pageview-weighted mean p75 (an approximation — a true percentile can't be re-derived from per-app
// p75s; with a single app it equals that app's p75). Returns null when no app reported the vital.
export function fleetVital(perApp: AppData[], metric: string): VitalRow | null {
  const rows = (perApp || []).map((a) => vitalOf(a.vitals, metric)).filter((r): r is VitalRow => r != null)
  if (!rows.length) return null
  let good = 0
  let needs = 0
  let poor = 0
  let wnum = 0
  let wden = 0
  let simpleSum = 0
  let simpleN = 0
  let goodMax: number | null = null
  let poorMin: number | null = null
  for (const r of rows) {
    const d = r.dist || { good: 0, needs: 0, poor: 0, total: 0 }
    good += d.good || 0
    needs += d.needs || 0
    poor += d.poor || 0
    if (r.p75 != null && Number.isFinite(r.p75)) {
      const w = d.total || 0
      wnum += r.p75 * w
      wden += w
      simpleSum += r.p75
      simpleN += 1
    }
    if (r.good_max != null) goodMax = r.good_max
    if (r.poor_min != null) poorMin = r.poor_min
  }
  const p75 = wden > 0 ? wnum / wden : simpleN ? simpleSum / simpleN : null
  return {
    metric,
    p75,
    rating: ratingFor(p75, goodMax, poorMin),
    good_max: goodMax,
    poor_min: poorMin,
    dist: { good, needs, poor, total: good + needs + poor },
  }
}

// The core-vital fleet band, LCP → INP → CLS, omitting any vital no app reported.
export function fleetVitals(perApp: AppData[]): VitalRow[] {
  return CORE_VITALS.map((m) => fleetVital(perApp, m)).filter((v): v is VitalRow => v != null)
}

const appPageviews = (a: AppData): number => sum(a.pages, (p) => p.pageviews)

// Headline fleet numbers for the KPI strip.
export function fleetKpis(perApp: AppData[]): FleetKpis {
  const apps = perApp || []
  // Share of vital measurements rated "good" across the three core vitals (0..1, or null if no data).
  let goodN = 0
  let totalN = 0
  for (const a of apps) {
    for (const m of CORE_VITALS) {
      const v = vitalOf(a.vitals, m)
      if (v) {
        goodN += v.dist?.good || 0
        totalN += v.dist?.total || 0
      }
    }
  }
  // Slowest app by LCP p75.
  let slowestApp: FleetKpis['slowestApp'] = null
  for (const a of apps) {
    const lcp = vitalOf(a.vitals, 'web_vitals.lcp')
    if (lcp && lcp.p75 != null && Number.isFinite(lcp.p75) && (!slowestApp || lcp.p75 > slowestApp.p75)) {
      slowestApp = { app: a.app, p75: lcp.p75, rating: lcp.rating }
    }
  }
  return {
    pageviews: sum(apps, appPageviews),
    goodShare: totalN ? goodN / totalN : null,
    appsPassing: { passing: apps.filter((a) => appWorstRating(a) === 'good').length, total: apps.length },
    // `sessions` sums each issue's distinct-session count across issues+apps — a rough affected-session
    // rollup (a session in two issues counts twice), fine for a headline figure.
    errors: sum(apps, (a) => sum(a.errors, (e) => e.count)),
    sessions: sum(apps, (a) => sum(a.errors, (e) => e.sessions)),
    slowestApp,
  }
}

// One row per app for the ranked table. Sorted worst-first: more poor vitals, then more
// needs-improvement, then more errors, then more traffic.
export function rankApps(perApp: AppData[]): AppRow[] {
  const rows: AppRow[] = (perApp || []).map((a) => {
    const ratings = appCoreRatings(a)
    const cell = (m: string): VitalCell | null => {
      const v = vitalOf(a.vitals, m)
      return v ? { p75: v.p75, rating: v.rating } : null
    }
    return {
      app: a.app,
      pageviews: appPageviews(a),
      lcp: cell('web_vitals.lcp'),
      inp: cell('web_vitals.inp'),
      cls: cell('web_vitals.cls'),
      errors: sum(a.errors, (e) => e.count),
      sessions: sum(a.errors, (e) => e.sessions),
      status: appWorstRating(a),
      _poor: ratings.filter((r) => r === 'poor').length,
      _needs: ratings.filter((r) => r === 'needs').length,
    }
  })
  return rows.sort(
    (x, y) => y._poor - x._poor || y._needs - x._needs || y.errors - x.errors || y.pageviews - x.pageviews,
  )
}

// Cross-app "live issues" feed: every error issue tagged with its app, most frequent first.
export function topIssues(perApp: AppData[], limit = 6): Issue[] {
  const all: Issue[] = []
  for (const a of perApp || []) {
    for (const e of a.errors || []) all.push({ app: a.app, ...e })
  }
  return all.sort((x, y) => (y.count || 0) - (x.count || 0)).slice(0, limit)
}

// Cross-app slowest routes by LCP p75. Each route's rating uses its own app's LCP thresholds
// (carried on that app's vitals row) so we still never hardcode cutoffs; null when unavailable.
export function slowestRoutes(perApp: AppData[], limit = 5): RouteRow[] {
  const all: RouteRow[] = []
  for (const a of perApp || []) {
    const lcp = vitalOf(a.vitals, 'web_vitals.lcp')
    for (const p of a.pages || []) {
      all.push({
        app: a.app,
        route: p.route,
        lcp_p75: p.lcp_p75,
        rating: lcp ? ratingFor(p.lcp_p75, lcp.good_max, lcp.poor_min) : null,
      })
    }
  }
  return all
    .filter((r) => r.lcp_p75 != null && Number.isFinite(r.lcp_p75))
    .sort((x, y) => (y.lcp_p75 as number) - (x.lcp_p75 as number))
    .slice(0, limit)
}

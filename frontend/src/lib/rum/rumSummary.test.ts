import { describe, it, expect } from 'vitest'
import {
  ratingFor,
  worstRating,
  fleetVital,
  fleetVitals,
  fleetKpis,
  appWorstRating,
  rankApps,
  topIssues,
  slowestRoutes,
  type AppData,
} from '@/lib/rum/rumSummary'

// LCP thresholds 2500/4000, INP 200/500, CLS 0.1/0.25 — as the API attaches them per row.
const vital = (metric: string, p75: number, dist: { good: number; needs: number; poor: number }, t: [number, number]) => ({
  metric,
  p75,
  rating: ratingFor(p75, t[0], t[1]),
  good_max: t[0],
  poor_min: t[1],
  dist: { ...dist, total: dist.good + dist.needs + dist.poor },
})

// storefront: LCP poor, INP good, CLS needs — the unhealthy, high-traffic app.
// docs: all three good, zero errors — the healthy app.
const perApp: AppData[] = [
  {
    app: 'storefront',
    vitals: [
      vital('web_vitals.lcp', 4300, { good: 40, needs: 30, poor: 30 }, [2500, 4000]),
      vital('web_vitals.inp', 180, { good: 90, needs: 7, poor: 3 }, [200, 500]),
      vital('web_vitals.cls', 0.18, { good: 60, needs: 30, poor: 10 }, [0.1, 0.25]),
    ],
    errors: [
      { fingerprint: 'a', exception_type: 'TypeError', message: 'x', count: 100, sessions: 40 },
      { fingerprint: 'b', exception_type: 'RangeError', message: 'y', count: 30, sessions: 12 },
    ],
    pages: [
      { route: '/checkout', pageviews: 5000, lcp_p75: 4900, inp_p75: 210, cls_p75: 0.2 },
      { route: '/home', pageviews: 8000, lcp_p75: 1800, inp_p75: 120, cls_p75: 0.02 },
    ],
  },
  {
    app: 'docs',
    vitals: [
      vital('web_vitals.lcp', 1900, { good: 95, needs: 4, poor: 1 }, [2500, 4000]),
      vital('web_vitals.inp', 120, { good: 98, needs: 1, poor: 1 }, [200, 500]),
      vital('web_vitals.cls', 0.02, { good: 99, needs: 1, poor: 0 }, [0.1, 0.25]),
    ],
    errors: [],
    pages: [{ route: '/guide', pageviews: 2000, lcp_p75: 2100, inp_p75: 130, cls_p75: 0.03 }],
  },
]

describe('ratingFor / worstRating', () => {
  it('rates against the supplied thresholds', () => {
    expect(ratingFor(2400, 2500, 4000)).toBe('good')
    expect(ratingFor(3000, 2500, 4000)).toBe('needs')
    expect(ratingFor(4300, 2500, 4000)).toBe('poor')
    expect(ratingFor(null, 2500, 4000)).toBeNull()
  })
  it('picks the most severe rating and ignores nulls', () => {
    expect(worstRating(['good', 'needs', null])).toBe('needs')
    expect(worstRating(['good', 'poor', 'needs'])).toBe('poor')
    expect(worstRating([null, null])).toBeNull()
  })
})

describe('fleetVital', () => {
  it('sums the distribution exactly and pageview-weights the p75', () => {
    const f = fleetVital(perApp, 'web_vitals.lcp')!
    // 30+1 poor, 30+4 needs, 40+95 good.
    expect(f.dist).toEqual({ good: 135, needs: 34, poor: 31, total: 200 })
    // weighted mean = (4300*100 + 1900*100) / 200 = 3100.
    expect(f.p75).toBe(3100)
    expect(f.rating).toBe('needs')
  })
  it('returns null when no app reported the vital', () => {
    expect(fleetVital(perApp, 'web_vitals.fcp')).toBeNull()
  })
  it('equals the single app p75 exactly with one app', () => {
    expect(fleetVital([perApp[1]], 'web_vitals.lcp')!.p75).toBe(1900)
  })
})

describe('fleetVitals', () => {
  it('returns the three core vitals in LCP→INP→CLS order', () => {
    expect(fleetVitals(perApp).map((v) => v.metric)).toEqual([
      'web_vitals.lcp',
      'web_vitals.inp',
      'web_vitals.cls',
    ])
  })
})

describe('fleetKpis', () => {
  it('rolls up pageviews, passing apps, errors, sessions, and the slowest app', () => {
    const k = fleetKpis(perApp)
    expect(k.pageviews).toBe(5000 + 8000 + 2000)
    expect(k.appsPassing).toEqual({ passing: 1, total: 2 }) // only docs is all-good
    expect(k.errors).toBe(130)
    expect(k.sessions).toBe(52)
    expect(k.slowestApp).toEqual({ app: 'storefront', p75: 4300, rating: 'poor' })
    expect(k.goodShare).toBeGreaterThan(0)
    expect(k.goodShare!).toBeLessThanOrEqual(1)
  })
})

describe('appWorstRating', () => {
  it('is the worst core vital rating for the app', () => {
    expect(appWorstRating(perApp[0])).toBe('poor')
    expect(appWorstRating(perApp[1])).toBe('good')
  })
})

describe('rankApps', () => {
  it('orders the unhealthiest app first', () => {
    const rows = rankApps(perApp)
    expect(rows.map((r) => r.app)).toEqual(['storefront', 'docs'])
    expect(rows[0].status).toBe('poor')
    expect(rows[0].errors).toBe(130)
    expect(rows[0].pageviews).toBe(13000)
    expect(rows[0].lcp).toEqual({ p75: 4300, rating: 'poor' })
  })
})

describe('topIssues', () => {
  it('flattens issues across apps, tags the app, and sorts by count', () => {
    const issues = topIssues(perApp)
    expect(issues).toHaveLength(2)
    expect(issues[0]).toMatchObject({ app: 'storefront', count: 100 })
    expect(issues[1].count).toBe(30)
  })
  it('respects the limit', () => {
    expect(topIssues(perApp, 1)).toHaveLength(1)
  })
})

describe('slowestRoutes', () => {
  it('sorts routes by LCP p75 desc and rates each via its app thresholds', () => {
    const routes = slowestRoutes(perApp, 3)
    expect(routes.map((r) => r.route)).toEqual(['/checkout', '/guide', '/home'])
    expect(routes[0]).toMatchObject({ app: 'storefront', lcp_p75: 4900, rating: 'poor' })
    expect(routes[1].rating).toBe('good') // /guide 2100 <= 2500
  })
})

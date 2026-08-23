// The fleet merge. These tests exist because the merge is where the index can lie: a stale Site
// contributes zeros, and zeros render as "no errors, no latency, all good" at the exact moment the
// page knows nothing about that Site (ADR-0004).
import { describe, it, expect } from 'vitest'
import {
  mergeFleet,
  rowId,
  staleSites,
  attentionRows,
  levelCounts,
  fleetTotals,
  compareRows,
  siteStats,
  type FleetRow,
} from './servicesOverview'
import type { OverviewRow, OverviewSite, ServicesOverview } from '@/lib/core/api'

function row(over: Partial<OverviewRow> = {}): OverviewRow {
  return {
    service: 'api',
    tenant: null,
    environment: null,
    operation: null,
    count: 1000,
    rate: 10,
    span_count: 1400,
    basis: 'entry',
    error_count: 0,
    error_rate: 0,
    http_4xx: 0,
    http_5xx: 0,
    satisfied: 1000,
    tolerating: 0,
    frustrated: 0,
    apdex: 1,
    p50: '1000000',
    p90: '2000000',
    p99: '3000000',
    spark: [],
    error_spark: [],
    ...over,
  }
}

function site(over: Partial<OverviewSite> = {}): OverviewSite {
  return {
    name: 'jkt',
    status: 'up',
    last_seen_ms: 0,
    mode: 'summary',
    ui_url: null,
    ...over,
  }
}

function overview(rows: OverviewRow[], sites: OverviewSite[] = []): ServicesOverview {
  return { source: 'raw', window: { start: '0', end: '1', buckets: 30 }, sites, rows }
}

describe('rowId', () => {
  it('separates the service from the environment with a character neither can contain', () => {
    // `a` + env `b-c` must not collide with service `a-b` + env `c`.
    expect(rowId('a', 'b-c')).not.toBe(rowId('a-b', 'c'))
  })

  it('treats a missing environment as its own identity, not as a wildcard', () => {
    expect(rowId('api', null)).not.toBe(rowId('api', 'prod'))
  })
})

describe('staleSites', () => {
  it('counts anything that is not up — stale and down both mean "we have no recent data"', () => {
    const s = staleSites([site({ name: 'a' }), site({ name: 'b', status: 'stale' }), site({ name: 'c', status: 'down' })])
    expect([...s].sort()).toEqual(['b', 'c'])
  })
})

describe('mergeFleet', () => {
  it('merges one service across Sites into a single row', () => {
    const out = mergeFleet(
      overview([row({ tenant: 'jkt' }), row({ tenant: 'sgp' })], [site({ name: 'jkt' }), site({ name: 'sgp' })]),
    )
    expect(out).toHaveLength(1)
    expect(out[0].sites).toHaveLength(2)
    expect(out[0].count).toBe(2000)
  })

  it('keeps two environments of the same service apart', () => {
    const out = mergeFleet(overview([row({ environment: 'prod' }), row({ environment: 'staging' })]))
    expect(out).toHaveLength(2)
    expect(out.map((r) => r.environment).sort()).toEqual(['prod', 'staging'])
  })

  it('reports Spread as unhealthy-out-of-reporting', () => {
    const out = mergeFleet(
      overview(
        [
          row({ tenant: 'jkt', error_rate: 0.2, error_count: 200 }),
          row({ tenant: 'sgp' }),
          row({ tenant: 'fra' }),
        ],
        [site({ name: 'jkt' }), site({ name: 'sgp' }), site({ name: 'fra' })],
      ),
    )
    expect(out[0].spread).toEqual({ unhealthy: 1, total: 3, stale: 0 })
  })

  it('excludes a stale Site from Spread rather than counting it healthy', () => {
    const out = mergeFleet(
      overview(
        [row({ tenant: 'jkt', error_rate: 0.2, error_count: 200 }), row({ tenant: 'sgp' })],
        [site({ name: 'jkt' }), site({ name: 'sgp', status: 'stale' })],
      ),
    )
    // "unhealthy in 1 of 2" would claim we know sgp is fine. We do not.
    expect(out[0].spread).toEqual({ unhealthy: 1, total: 1, stale: 1 })
  })

  it('marks a stale Site’s slice Unknown, never OK', () => {
    const out = mergeFleet(
      overview([row({ tenant: 'sgp', count: 0, error_count: 0, apdex: null })], [site({ name: 'sgp', status: 'stale' })]),
    )
    const slice = out[0].sites[0]
    expect(slice.siteStale).toBe(true)
    expect(slice.level).toBe('unknown')
    expect(out[0].level).toBe('unknown')
  })

  it('keeps a stale Site out of the numeric totals', () => {
    const out = mergeFleet(
      overview(
        [row({ tenant: 'jkt', count: 500 }), row({ tenant: 'sgp', count: 0 })],
        [site({ name: 'jkt' }), site({ name: 'sgp', status: 'stale' })],
      ),
    )
    // 500, not 250: averaging a blind Site's zeros in halves the fleet's real traffic.
    expect(out[0].count).toBe(500)
  })

  it('takes the worst p99 across Sites, never the mean', () => {
    const out = mergeFleet(
      overview([row({ tenant: 'jkt', p99: '1000000' }), row({ tenant: 'sgp', p99: '9000000' })]),
    )
    // A mean of two p99s is not a percentile of anything.
    expect(out[0].p99).toBe(9_000_000)
  })

  it('recomputes Apdex from the bands, not by averaging per-Site scores', () => {
    const out = mergeFleet(
      overview([
        row({ tenant: 'jkt', satisfied: 100, tolerating: 0, frustrated: 0, apdex: 1 }),
        row({ tenant: 'sgp', satisfied: 0, tolerating: 0, frustrated: 900, apdex: 0 }),
      ]),
    )
    // 100 satisfied of 1000 sampled = 0.1, not the (1 + 0)/2 = 0.5 an average would give.
    expect(out[0].apdex).toBeCloseTo(0.1, 5)
  })

  it('sums sparklines bucket by bucket', () => {
    const out = mergeFleet(
      overview([
        row({ tenant: 'jkt', spark: [[1000, 5], [2000, 7]] }),
        row({ tenant: 'sgp', spark: [[1000, 3], [3000, 1]] }),
      ]),
    )
    expect(out[0].spark).toEqual([[1000, 8], [2000, 7], [3000, 1]])
  })

  it('badges a row span-based when any Site fell off entry spans', () => {
    const clean = mergeFleet(overview([row({ tenant: 'jkt' })]))
    expect(clean[0].spanBased).toBe(false)
    const fallback = mergeFleet(overview([row({ tenant: 'jkt' }), row({ tenant: 'sgp', basis: 'all_spans' })]))
    expect(fallback[0].spanBased).toBe(true)
  })

  it('calls a service that reported last window and nothing now Down', () => {
    const out = mergeFleet(overview([row({ count: 0, error_count: 0, apdex: null })]), {
      prev: [row({ count: 5000, rate: 50 })],
    })
    expect(out[0].level).toBe('down')
  })

  it('leaves a service that never reported Unknown', () => {
    const out = mergeFleet(overview([row({ count: 0, error_count: 0, apdex: null })]))
    expect(out[0].level).toBe('unknown')
  })

  it('routes the previous window per Site, not per service', () => {
    // jkt went silent; sgp is still serving. Merging the two prev rows into one would hide it.
    const out = mergeFleet(
      overview([row({ tenant: 'jkt', count: 0, error_count: 0, apdex: null }), row({ tenant: 'sgp' })]),
      { prev: [row({ tenant: 'jkt', count: 5000, rate: 50 }), row({ tenant: 'sgp', count: 5000 })] },
    )
    const jkt = out[0].sites.find((s) => s.tenant === 'jkt')
    expect(jkt?.level).toBe('down')
    expect(out[0].level).toBe('down')
  })

  it('raises a row on a triggered incident and carries the badge count', () => {
    const inc = {
      id: 1, rule_id: 'r', series_key: '', severity: 'critical' as const,
      state: 'triggered', started_at: 0, resolved_at: null, value: 0, summary: 'p99 over budget',
    }
    const out = mergeFleet(overview([row()]), { incidentsByService: { api: [inc as never] } })
    expect(out[0].level).toBe('critical')
    expect(out[0].incidents).toHaveLength(1)
  })

  it('orders the per-Site slices worst first', () => {
    const out = mergeFleet(
      overview([row({ tenant: 'ok' }), row({ tenant: 'bad', error_rate: 0.5, error_count: 500 })]),
    )
    expect(out[0].sites[0].tenant).toBe('bad')
  })
})

describe('ordering and tallies', () => {
  const rows = (): FleetRow[] =>
    mergeFleet(
      overview([
        row({ service: 'ok-svc' }),
        row({ service: 'warn-svc', error_rate: 0.02, error_count: 20 }),
        row({ service: 'crit-svc', error_rate: 0.5, error_count: 500 }),
      ]),
    )

  it('sorts worst first', () => {
    expect(rows().map((r) => r.service)).toEqual(['crit-svc', 'warn-svc', 'ok-svc'])
  })

  it('breaks a level tie on error rate, then on name', () => {
    const a = { level: 'ok', error_rate: 0.001, service: 'b' } as FleetRow
    const b = { level: 'ok', error_rate: 0.009, service: 'a' } as FleetRow
    expect(compareRows(a, b)).toBeGreaterThan(0)
    const c = { level: 'ok', error_rate: 0, service: 'a' } as FleetRow
    const d = { level: 'ok', error_rate: 0, service: 'b' } as FleetRow
    expect(compareRows(c, d)).toBeLessThan(0)
  })

  it('lists only rows that want attention, capped', () => {
    expect(attentionRows(rows()).map((r) => r.service)).toEqual(['crit-svc', 'warn-svc'])
    expect(attentionRows(rows(), 1)).toHaveLength(1)
  })

  it('tallies every level, including the empty ones', () => {
    expect(levelCounts(rows())).toEqual({ down: 0, critical: 1, warning: 1, ok: 1, unknown: 0 })
  })

  it('computes fleet totals over the rows the table shows', () => {
    const t = fleetTotals(rows())
    expect(t.services).toBe(3)
    expect(t.requests).toBe(3000)
    expect(t.errorRate).toBeCloseTo(520 / 3000, 5)
  })
})

describe('siteStats', () => {
  it('renders nothing on a Site install — there is no registry to band', () => {
    const o = overview([row()])
    expect(siteStats(o, mergeFleet(o))).toEqual([])
  })

  it('leads with an All-sites card that counts merged services, not slices', () => {
    const o = overview(
      [row({ tenant: 'jkt' }), row({ tenant: 'sgp' })],
      [site({ name: 'jkt' }), site({ name: 'sgp' })],
    )
    const cards = siteStats(o, mergeFleet(o))
    expect(cards[0].key).toBeNull()
    // One service the fleet runs in two places — saying "2" would contradict the table below.
    expect(cards[0].services).toBe(1)
    expect(cards[0].requests).toBe(2000)
    expect(cards.map((c) => c.name)).toEqual(['All sites', 'jkt', 'sgp'])
  })

  it('orders the Site cards worst first', () => {
    const o = overview(
      [row({ tenant: 'jkt' }), row({ tenant: 'sgp', error_rate: 0.5, error_count: 500 })],
      [site({ name: 'jkt' }), site({ name: 'sgp' })],
    )
    expect(siteStats(o, mergeFleet(o)).map((c) => c.name)).toEqual(['All sites', 'sgp', 'jkt'])
  })

  it('carries the registry facts the rows cannot', () => {
    const o = overview([row({ tenant: 'jkt' })], [site({ name: 'jkt', mode: 'full', last_seen_ms: 42, ui_url: 'https://x' })])
    const jkt = siteStats(o, mergeFleet(o))[1]
    expect(jkt).toMatchObject({ mode: 'full', lastSeenMs: 42, uiUrl: 'https://x', stale: false })
  })

  it('marks a stale Site stale and Unknown rather than OK', () => {
    const o = overview([row({ tenant: 'sgp', count: 0, apdex: null })], [site({ name: 'sgp', status: 'stale' })])
    const sgp = siteStats(o, mergeFleet(o))[1]
    expect(sgp.stale).toBe(true)
    expect(sgp.level).toBe('unknown')
  })

  it('gives a registered Site with no services a card anyway', () => {
    // No row means no traffic — but the Site is registered, and hiding it would read as "there is
    // no such Site" rather than "that Site is serving nothing".
    const o = overview([], [site({ name: 'fra' })])
    const cards = siteStats(o, mergeFleet(o))
    expect(cards.map((c) => c.name)).toEqual(['All sites', 'fra'])
    expect(cards[1].services).toBe(0)
  })
})

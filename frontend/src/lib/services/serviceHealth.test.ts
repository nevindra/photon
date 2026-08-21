// Table tests for the single Attention Level model. Pure — no Vue, no network.
//
// The rules are ordered and the order is the point, so there is one test per rule plus the two that
// pin the interactions (a stale Site beating everything, an incident that may only raise). `idle`
// has its own test because retiring it is the whole reason this module changed shape.
import { describe, it, expect } from 'vitest'
import {
  attentionServices,
  byWorstFirst,
  healthCounts,
  healthReasons,
  LEVELS,
  serviceHealth,
  serviceLevel,
  SILENCE_MIN_PREV_COUNT,
  STATUS_META,
  type AttentionLevel,
  type ServiceHealthRow,
} from '@/lib/services/serviceHealth'
import type { AlertIncident, AlertSeverity } from '@/lib/core/api'

const incident = (severity: AlertSeverity, summary = 'error_rate 8.2% > 5%'): AlertIncident => ({
  id: 1,
  rule_id: 'r1',
  series_key: '',
  started_at: 0,
  ended_at: null,
  peak_value: 0,
  severity,
  summary,
})

const OK: ServiceHealthRow = { count: 500, rate: 5, error_rate: 0, apdex: 0.99 }
const CRITICAL: ServiceHealthRow = { count: 500, rate: 5, error_rate: 0.082, apdex: 0.62 }

describe('serviceLevel', () => {
  it('stale_site_beats_every_other_rule', () => {
    // Healthy-looking numbers from a Site that stopped pushing are not health — they are silence
    // wearing health's clothes.
    const blind = serviceLevel(OK, { siteStale: true })
    expect(blind.level).toBe('unknown')
    expect(blind.reasons[0].label).toBe('No data')

    // Not Down either, even though the previous window had traffic: the row is missing because the
    // Site went quiet, which says nothing about the service. And not raised by an incident: the
    // alert engine evaluated the same absent data, so its state is exactly as stale as the row.
    expect(
      serviceLevel(
        { count: 0 },
        { siteStale: true, prev: { count: 5000, rate: 500 }, incidents: [incident('critical')] },
      ).level,
    ).toBe('unknown')
  })

  it('silence_is_down_not_unknown', () => {
    const { level, reasons } = serviceLevel({ count: 0 }, { prev: { count: 3400, rate: 340 } })
    expect(level).toBe('down')
    expect(reasons[0]).toEqual({ label: 'No spans', value: '340/s', note: 'was' })

    // A missing row is the same fact as a zero row — a Site that went quiet sends neither.
    expect(serviceLevel({}, { prev: { count: 3400, rate: 340 } }).level).toBe('down')
  })

  it('never_reported_is_unknown_not_down', () => {
    expect(serviceLevel({ count: 0 }).level).toBe('unknown')
    expect(serviceLevel({ count: 0 }, { prev: { count: SILENCE_MIN_PREV_COUNT - 1 } }).level).toBe('unknown')
    expect(serviceLevel({ count: 0 }).reasons[0]).toEqual({ label: 'No traffic', value: '—' })
  })

  it('incident_raises_but_never_lowers_the_level', () => {
    const warn = [incident('warning', 'p99 above 1s')]

    // Already Critical: a Warning-mapped incident leaves it alone.
    expect(serviceLevel(CRITICAL, { incidents: warn }).level).toBe('critical')

    // The same incident on a clean row raises it, and says who asked to be told, first.
    const raised = serviceLevel(OK, { incidents: warn })
    expect(raised.level).toBe('warning')
    expect(raised.reasons[0]).toEqual({ label: 'p99 above 1s', value: '', note: 'warning' })

    // `info` maps to Warning too — somebody asked to be told, but it never outranks a real outage.
    expect(serviceLevel(OK, { incidents: [incident('info')] }).level).toBe('warning')
    expect(serviceLevel(OK, { incidents: [incident('critical')] }).level).toBe('critical')

    // Silence outranks every incident: Down is rank 0 and nothing raises past it.
    expect(serviceLevel({ count: 0 }, { prev: { count: 3400, rate: 340 }, incidents: warn }).level).toBe('down')

    // Unknown is the absence of a judgment, so a firing incident IS a judgment and replaces it.
    expect(serviceLevel({ count: 0 }, { incidents: warn }).level).toBe('warning')
  })

  it('error_rate_and_apdex_take_the_worse_of_the_two', () => {
    // Either one on its own is enough.
    expect(serviceLevel({ count: 10, error_rate: 0.05, apdex: 0.99 }).level).toBe('critical')
    expect(serviceLevel({ count: 10, error_rate: 0, apdex: 0.69 }).level).toBe('critical')
    expect(serviceLevel({ count: 10, error_rate: 0.01, apdex: 0.99 }).level).toBe('warning')
    expect(serviceLevel({ count: 10, error_rate: 0, apdex: 0.84 }).level).toBe('warning')
    expect(serviceLevel({ count: 10, error_rate: 0.001, apdex: 0.98 }).level).toBe('ok')

    // Worse wins, and each clause keeps its own word rather than being flattened to the row's.
    const mixed = serviceLevel({ count: 10, error_rate: 0.02, apdex: 0.62 })
    expect(mixed.level).toBe('critical')
    expect(mixed.reasons).toEqual([
      { label: 'Error rate', value: '2.0%', note: 'warning' },
      { label: 'Apdex', value: '0.62', note: 'critical' },
    ])

    // A null Apdex is not a judgment — fall back to the error rate alone.
    expect(serviceLevel({ count: 10, error_rate: 0.001, apdex: null }).level).toBe('ok')
    expect(serviceLevel({ count: 10, error_rate: 0.2, apdex: null }).level).toBe('critical')
  })

  it('idle_is_gone', () => {
    // Type level: `idle` is not an Attention Level at all any more. If it ever comes back, this
    // line compiles and `bun run type-check` fails on the unused @ts-expect-error.
    // @ts-expect-error `idle` is retired — Silence is Down, never-reported is Unknown.
    const retired: AttentionLevel = 'idle'
    void retired

    expect(LEVELS).toEqual(['down', 'critical', 'warning', 'ok', 'unknown'])
    expect(Object.keys(STATUS_META)).not.toContain('idle')
    expect(Object.keys(healthCounts([]))).not.toContain('idle')

    // And the fact `idle` used to hide: a service falling from 5,000 req/s to nothing is Down, not
    // a calm neutral reading.
    expect(serviceLevel({ count: 0 }, { prev: { count: 5000, rate: 500 } }).level).toBe('down')
  })
})

describe('healthReasons', () => {
  it('names the crossed conditions', () => {
    expect(healthReasons({ error_rate: 0.082, apdex: 0.71 })).toEqual(['Error rate 8.2%', 'Apdex 0.71'])
  })
  it('is empty when clean', () => {
    expect(healthReasons({ error_rate: 0.001, apdex: 0.99 })).toEqual([])
  })
})

describe('serviceHealth', () => {
  it('carries the level, its display metadata, and the reason line as text', () => {
    const h = serviceHealth(CRITICAL)
    expect(h.level).toBe('critical')
    expect(h.meta).toBe(STATUS_META.critical)
    expect(h.text).toEqual(['Error rate 8.2%', 'Apdex 0.62'])
  })
  it('drops the em-dash placeholder from the text form', () => {
    expect(serviceHealth({ count: 0 }).text).toEqual(['No traffic'])
  })
})

describe('healthCounts', () => {
  it('tallies by level', () => {
    const rows = [
      { count: 0 },
      { count: 5, error_rate: 0.06, apdex: 0.9 },
      { count: 5, error_rate: 0.02, apdex: 0.9 },
      { count: 5, error_rate: 0, apdex: 0.99 },
    ]
    expect(healthCounts(rows)).toEqual({ down: 0, critical: 1, warning: 1, ok: 1, unknown: 1 })
  })
})

describe('byWorstFirst / attentionServices', () => {
  const rows = [
    { service: 'ok', count: 100, error_rate: 0, apdex: 0.99 },
    { service: 'crit', count: 100, error_rate: 0.2, apdex: 0.5 },
    { service: 'warn', count: 100, error_rate: 0.02, apdex: 0.9 },
    { service: 'quiet', count: 0 },
  ]
  it('orders critical → warning → ok → unknown', () => {
    expect(byWorstFirst(rows).map((r) => r.service)).toEqual(['crit', 'warn', 'ok', 'quiet'])
  })
  it('picks only the levels that need attention, worst-first, capped', () => {
    expect(attentionServices(rows, 3).map((r) => r.service)).toEqual(['crit', 'warn'])
    expect(attentionServices(rows, 1).map((r) => r.service)).toEqual(['crit'])
  })
})

describe('STATUS_META', () => {
  it('has literal class strings for every level', () => {
    for (const level of LEVELS) {
      expect(typeof STATUS_META[level].dot).toBe('string')
      expect(typeof STATUS_META[level].label).toBe('string')
      expect(typeof STATUS_META[level].rail).toBe('string')
    }
  })
  it('spends the one solid pill on Down, so it cannot read as "a bit slow"', () => {
    expect(STATUS_META.down.solid).toBe(true)
    expect(STATUS_META.critical.solid).toBe(false)
    // Down and Critical share the error tone; only the weight separates them.
    expect(STATUS_META.down.tone).toBe('error')
    expect(STATUS_META.critical.tone).toBe('error')
  })
})

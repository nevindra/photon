// Table tests for the Attention Board's domain model. Everything here is pure — no Vue, no
// network — which is the whole reason the judging lives in `attention.ts` and not in the view.
import { describe, it, expect } from 'vitest'
import {
  applyIncidents,
  attentionList,
  boardWorlds,
  compactAge,
  compareSubjects,
  hostSubject,
  HOST_SILENCE_MS,
  monitorSubject,
  needsAttention,
  parseSeriesKey,
  resolveIncident,
  rollup,
  rumAppSubject,
  serviceSubjects,
  SILENCE_MIN_PREV_COUNT,
  siteSubject,
  vanishedHostSubject,
  vitalSamples,
  vitalText,
  worstLevel,
  type Subject,
} from './attention'
import type { AlertIncident, AlertRule, InfraHost, Monitor, RumVitalRow, TenantSummary } from '@/lib/core/api'

const NOW = 1_700_000_000_000

const host = (over: Partial<InfraHost> = {}): InfraHost => ({
  host: 'web-01',
  cpuUtil: 0.2,
  memUtil: 0.3,
  diskUtil: 0.4,
  diskUtilAvg: 0.4,
  diskGroups: 1,
  gpuUtil: null,
  gpuUtilAvg: null,
  gpuGroups: 0,
  lastSeenNs: String(BigInt(NOW) * 1_000_000n),
  hasGpu: false,
  ...over,
})

const monitor = (over: Partial<Monitor> = {}): Monitor => ({
  id: 'm1',
  name: 'checkout probe',
  type: 'http',
  target: 'https://shop.example/checkout',
  interval_secs: 60,
  timeout_secs: 10,
  retries: 3,
  enabled: true,
  ...over,
})

const rule = (over: Partial<AlertRule> = {}): AlertRule => ({
  id: 'r1',
  name: 'Checkout error rate',
  description: null,
  enabled: true,
  signal: 'traces',
  condition: { signal: 'traces', service: 'payments-api', kind: 'error_rate', window_secs: 300, cmp: 'gt', threshold: 0.05 },
  for_secs: 60,
  interval_secs: 60,
  severity: 'critical',
  channel_ids: [],
  created_at: 0,
  updated_at: 0,
  ...over,
})

const incident = (over: Partial<AlertIncident> = {}): AlertIncident => ({
  id: 1,
  rule_id: 'r1',
  series_key: '',
  started_at: Math.floor(NOW / 1000) - 600,
  ended_at: null,
  peak_value: 0.08,
  severity: 'critical',
  summary: 'error_rate 8.2% > 5%',
  ...over,
})

const site = (over: Partial<TenantSummary> = {}): TenantSummary => ({
  name: 'eu-west',
  mode: 'full',
  status: 'up',
  last_seen_ms: NOW - 30_000,
  ingest_rows_per_sec: 120,
  open_incidents: 0,
  hot_bytes: 0,
  ui_url: null,
  spark: [],
  ...over,
})

describe('attention levels', () => {
  it('ranks down worst and unknown last', () => {
    expect(worstLevel('ok', 'warning', 'critical')).toBe('critical')
    expect(worstLevel('unknown', 'ok')).toBe('ok')
    expect(worstLevel('warning', 'down')).toBe('down')
    expect(worstLevel()).toBe('unknown')
  })

  it('lists only down/critical/warning', () => {
    expect(needsAttention('down')).toBe(true)
    expect(needsAttention('critical')).toBe(true)
    expect(needsAttention('warning')).toBe(true)
    expect(needsAttention('ok')).toBe(false)
    expect(needsAttention('unknown')).toBe(false)
  })

  it('adds the Sites world only when a registry exists', () => {
    expect(boardWorlds().map((w) => w.key)).toEqual(['frontend', 'backend', 'infra', 'ops'])
    expect(boardWorlds({ sites: true }).map((w) => w.key)).toContain('sites')
  })

  // Uptime has no tenant dimension, so under a lens its monitors are always Central's own.
  it('drops the Ops world when a tenant lens is active', () => {
    expect(boardWorlds({ ops: false }).map((w) => w.key)).toEqual(['frontend', 'backend', 'infra'])
  })
})

describe('serviceSubjects', () => {
  it('takes serviceHealth’s judgment as-is, one clause per crossed threshold', () => {
    const [critical] = serviceSubjects([{ service: 'payments-api', count: 500, error_rate: 0.082, apdex: 0.62 }])
    expect(critical.level).toBe('critical')
    expect(critical.reasons[0]).toEqual({ label: 'Error rate', value: '8.2%', note: 'critical' })

    // serviceHealth speaks the shared vocabulary now, so the note is "warning" — the old
    // "degraded" band word is retired along with `idle`.
    const [warning] = serviceSubjects([{ service: 'auth-api', count: 500, error_rate: 0.002, apdex: 0.81 }])
    expect(warning.level).toBe('warning')
    expect(warning.reasons[0]).toEqual({ label: 'Apdex', value: '0.81', note: 'warning' })

    const [healthy] = serviceSubjects([{ service: 'search', count: 500, error_rate: 0, apdex: 0.99 }])
    expect(healthy.level).toBe('ok')
    expect(healthy.reasons).toEqual([])
  })

  // The hole this closes: zero traffic used to be a level of its own (`idle`), so a service falling
  // from thousands of req/s to nothing disappeared exactly when it mattered. It is Down now, and
  // the rule lives in serviceHealth.ts where every page reads it.
  it('calls a service that reported and then stopped Down, not idle', () => {
    const subjects = serviceSubjects([], [{ service: 'ledger-worker', count: 3400, rate: 340 }])
    expect(subjects).toHaveLength(1)
    expect(subjects[0].level).toBe('down')
    expect(subjects[0].reasons[0]).toEqual({ label: 'No spans', value: '340/s', note: 'was' })
  })

  it('calls a service that never reported Unknown, not Down', () => {
    const subjects = serviceSubjects(
      [{ service: 'batch-job', count: 0 }],
      [{ service: 'batch-job', count: SILENCE_MIN_PREV_COUNT - 1 }],
    )
    expect(subjects[0].level).toBe('unknown')
  })

  it('merges the same service across Sites into one row carrying a Spread', () => {
    const [subject] = serviceSubjects([
      { service: 'payments-api', tenant: 'eu-west', count: 100, error_rate: 0.082 },
      { service: 'payments-api', tenant: 'us-east', count: 100, error_rate: 0.08 },
      { service: 'payments-api', tenant: 'ap-south', count: 100, error_rate: 0 },
    ])
    expect(subject.spread).toEqual({ bad: 2, total: 3 })
    expect(subject.sites.sort()).toEqual(['eu-west', 'us-east'])
    // The merged row speaks for the worst Site — that is the one somebody has to look at.
    expect(subject.level).toBe('critical')
  })

  it('leaves an untenanted (Central-local) service without a Spread', () => {
    const [subject] = serviceSubjects([{ service: 'photon-itself', count: 10, error_rate: 0 }])
    expect(subject.spread).toBeNull()
    expect(subject.local).toBe(true)
  })
})

describe('rumAppSubject', () => {
  const vitals = (rating: string): RumVitalRow[] => [
    { metric: 'web_vitals.lcp', p75: 4800, rating, good_max: 2500, poor_min: 4000, dist: { good: 1, needs: 2, poor: 7, total: 10 } },
  ]

  // A page that cries wolf about latency stops being read.
  it('never raises a slow Web Vital above Warning', () => {
    const subject = rumAppSubject('storefront', vitals('poor'))
    expect(subject.level).toBe('warning')
    expect(subject.reasons[0]).toEqual({ label: 'LCP p75', value: '4.80s', note: 'poor' })
  })

  it('is OK when every vital is rated good', () => {
    expect(rumAppSubject('storefront', vitals('good')).level).toBe('ok')
  })

  it('separates an app that stopped reporting from one that never did', () => {
    const empty: RumVitalRow[] = []
    expect(rumAppSubject('storefront', empty).level).toBe('unknown')
    expect(rumAppSubject('storefront', empty, vitals('good')).level).toBe('down')
  })

  it('counts samples from the distribution, falling back to the parts when total is absent', () => {
    expect(vitalSamples([{ ...vitals('good')[0], dist: { good: 3, needs: 1, poor: 1 } }])).toBe(5)
  })

  it('formats CLS without a unit and every other vital as a duration', () => {
    expect(vitalText('web_vitals.cls', 0.184)).toBe('0.18')
    expect(vitalText('web_vitals.inp', 210)).toBe('210ms')
    expect(vitalText('web_vitals.lcp', 2800)).toBe('2.80s')
  })
})

describe('hostSubject', () => {
  it('is Down once it stops sampling, even while still inside the window', () => {
    const stale = host({ lastSeenNs: String(BigInt(NOW - HOST_SILENCE_MS - 60_000) * 1_000_000n) })
    const subject = hostSubject(stale, NOW)
    expect(subject.level).toBe('down')
    expect(subject.reasons[0].note).toBe('silent')
    expect(subject.sinceMs).toBe(NOW - HOST_SILENCE_MS - 60_000)
  })

  it('maps hostStats thresholds: >=90% is Critical, >=80% Warning', () => {
    expect(hostSubject(host({ cpuUtil: 0.95 }), NOW).level).toBe('critical')
    expect(hostSubject(host({ memUtil: 0.85 }), NOW).level).toBe('warning')
    expect(hostSubject(host(), NOW).level).toBe('ok')
  })

  it('reports a host that vanished from the window as Down', () => {
    expect(vanishedHostSubject('web-03').level).toBe('down')
  })
})

describe('monitorSubject', () => {
  it('takes the probe state as the judgment, and a paused monitor as no judgment', () => {
    expect(monitorSubject(monitor({ last_state: 'down' })).level).toBe('down')
    expect(monitorSubject(monitor({ last_state: 'up' })).level).toBe('ok')
    expect(monitorSubject(monitor({ enabled: false, last_state: 'up' })).level).toBe('unknown')
    expect(monitorSubject(monitor({})).level).toBe('unknown')
  })
})

describe('siteSubject', () => {
  it('uses the tenants API staleness bands verbatim', () => {
    expect(siteSubject(site({ status: 'up' }), NOW).level).toBe('ok')
    expect(siteSubject(site({ status: 'stale' }), NOW).level).toBe('warning')
    expect(siteSubject(site({ status: 'down' }), NOW).level).toBe('down')
  })

  it('carries the incidents open at the Site into the reason line', () => {
    const subject = siteSubject(site({ status: 'down', open_incidents: 3 }), NOW)
    expect(subject.reasons.map((r) => r.label)).toContain('Open incidents there')
  })
})

describe('incidents', () => {
  it('parses a canonical series key, tolerating values that contain =', () => {
    expect(parseSeriesKey('host.name=web-03,service.name=api')).toEqual({ 'host.name': 'web-03', 'service.name': 'api' })
    expect(parseSeriesKey('q=a=b')).toEqual({ q: 'a=b' })
    expect(parseSeriesKey('')).toEqual({})
    expect(parseSeriesKey(null)).toEqual({})
  })

  it('resolves each signal to the Subject it names', () => {
    expect(resolveIncident(incident(), rule())).toEqual({ kind: 'service', name: 'payments-api' })
    expect(
      resolveIncident(
        incident(),
        rule({ signal: 'rum', condition: { signal: 'rum', app_id: 'storefront', kind: 'vital_lcp_p75', window_secs: 300, cmp: 'gt', threshold: 4000 } }),
      ),
    ).toEqual({ kind: 'rumApp', name: 'storefront' })
    // The uptime bridge synthesises its rule id, so no rule lookup is needed at all.
    expect(resolveIncident(incident({ rule_id: 'uptime:m1' }))).toEqual({ kind: 'monitor', name: 'm1' })
  })

  it('resolves a grouped metrics rule through the series key', () => {
    const metrics = rule({
      signal: 'metrics',
      condition: { signal: 'metrics', metric_name: 'system.cpu.utilization', group_by: ['host.name'], agg: 'avg', window_secs: 300, cmp: 'gt', threshold: 0.9 },
    })
    expect(resolveIncident(incident({ series_key: 'host.name=gpu-01' }), metrics)).toEqual({ kind: 'host', name: 'gpu-01' })
    // No group_by → the rule is genuinely about the fleet. That is an Unattributed Incident.
    expect(resolveIncident(incident({ series_key: '' }), metrics)).toBeNull()
  })

  it('raises a Subject level, counts the badge, and takes the incident start as the age', () => {
    const subjects = serviceSubjects([{ service: 'payments-api', count: 500, error_rate: 0, apdex: 0.99 }])
    expect(subjects[0].level).toBe('ok')
    const { subjects: after, unattributed, orphaned } = applyIncidents(subjects, [incident()], [rule()])
    expect(after[0].level).toBe('critical')
    expect(after[0].incidents).toBe(1)
    expect(after[0].sinceMs).toBe(incident().started_at * 1000)
    expect(unattributed).toEqual([])
    expect(orphaned).toEqual([])
  })

  // Both buckets are returned, never dropped — but they are different claims. "About the fleet"
  // is always true; "about a place missing from this board" can be an artifact of a tenant lens.
  it('separates a fleet-level incident from one whose place is off the board', () => {
    const fleet = rule({
      id: 'r2', signal: 'metrics',
      condition: { signal: 'metrics', metric_name: 'system.cpu.utilization', agg: 'avg', window_secs: 300, cmp: 'gt', threshold: 0.9 },
    })
    const { unattributed, orphaned } = applyIncidents(
      [],
      [incident(), incident({ id: 2, rule_id: 'r2' })],
      [rule(), fleet],
    )
    expect(unattributed.map((u) => u.rule?.id)).toEqual(['r2'])
    expect(orphaned.map((o) => o.rule?.id)).toEqual(['r1'])
  })

  it('resolves an uptime incident to the monitor and shows its display name', () => {
    const subjects = [monitorSubject(monitor({ last_state: 'up' }))]
    const { subjects: after } = applyIncidents(subjects, [incident({ rule_id: 'uptime:m1', severity: 'critical' })], [], { m1: 'checkout probe' })
    expect(after[0].incidents).toBe(1)
    expect(after[0].name).toBe('checkout probe')
  })

  it('never lets a warning or info incident outrank a real outage', () => {
    const subjects = serviceSubjects([], [{ service: 'ledger-worker', count: 3400, rate: 340 }])
    applyIncidents(subjects, [incident({ severity: 'info' })], [rule({ condition: { signal: 'traces', service: 'ledger-worker', kind: 'error_rate', window_secs: 300, cmp: 'gt', threshold: 0.05 } })])
    expect(subjects[0].level).toBe('down')
    expect(subjects[0].incidents).toBe(1)
  })
})

describe('rollup and ordering', () => {
  const s = (over: Partial<Subject>): Subject => ({
    id: 'service:x',
    kind: 'service',
    world: 'backend',
    name: 'x',
    sites: [],
    local: false,
    level: 'ok',
    reasons: [],
    sinceMs: null,
    incidents: 0,
    spread: null,
    dest: '/services/x',
    ...over,
  })

  it('tallies every level per world and reports the worst', () => {
    const [frontend, backend] = rollup([
      s({ id: 'a', world: 'backend', level: 'down' }),
      s({ id: 'b', world: 'backend', level: 'ok' }),
      s({ id: 'c', world: 'frontend', level: 'warning' }),
    ], boardWorlds())
    expect(frontend.total).toBe(1)
    expect(frontend.worst).toBe('warning')
    expect(backend.tallies).toEqual({ down: 1, critical: 0, warning: 0, ok: 1, unknown: 0 })
    expect(backend.worst).toBe('down')
  })

  it('lists worst first, then most incidents, then oldest', () => {
    const list = attentionList([
      s({ id: 'warn', level: 'warning' }),
      s({ id: 'down', level: 'down' }),
      s({ id: 'ok', level: 'ok' }),
      s({ id: 'crit-old', level: 'critical', sinceMs: 100 }),
      s({ id: 'crit-new', level: 'critical', sinceMs: 900 }),
      s({ id: 'crit-paged', level: 'critical', sinceMs: 900, incidents: 2 }),
    ])
    expect(list.map((x) => x.id)).toEqual(['down', 'crit-paged', 'crit-old', 'crit-new', 'warn'])
  })

  it('filters to one world without reordering', () => {
    const list = attentionList([s({ id: 'a', level: 'down' }), s({ id: 'b', world: 'infra', level: 'critical' })], 'infra')
    expect(list.map((x) => x.id)).toEqual(['b'])
  })

  it('breaks a total tie by name so the order is stable across polls', () => {
    expect(compareSubjects(s({ name: 'a' }), s({ name: 'b' }))).toBeLessThan(0)
  })
})

describe('compactAge', () => {
  it('gives one unit, never two', () => {
    expect(compactAge(42_000)).toBe('42s')
    expect(compactAge(18 * 60_000)).toBe('18m')
    expect(compactAge(3 * 3_600_000)).toBe('3h')
    expect(compactAge(5 * 86_400_000)).toBe('5d')
    expect(compactAge(null)).toBe('—')
  })
})

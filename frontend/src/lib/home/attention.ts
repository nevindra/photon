// The Attention Board's domain model, pure and table-tested (no Vue, no I/O). `/home` judges every
// watched thing into ONE ranked Attention Level so a Host and a RUM app can sit in the same ordered
// list — see docs/adr/0001-landing-page-is-an-attention-board.md and CONTEXT.md for the vocabulary.
//
// This module adds NO fourth opinion about what "bad" means. Services come through
// `lib/services/serviceHealth.ts`, hosts through `lib/infra/hostStats.ts`, Web Vitals through the
// `rating` the API already computed, Sites through the tenants API's own up/stale/down. All this
// file does is map those native judgments onto the shared axis, keep the native word in the reason
// line, and rank the result.
//
// The axis itself lives in `lib/services/serviceHealth.ts` — the Attention Level, its ranking, its
// display metadata and the incident-severity mapping are all defined once, there, and re-exported
// here so the board's own vocabulary keeps its home-shaped names. Services used to be judged twice,
// once per module, which is how one service could read "Critical" here and "Degraded" on
// `/services` in the same second.
import {
  INCIDENT_LEVEL,
  LEVEL_RANK,
  LEVELS,
  needsAttention,
  serviceLevel,
  SILENCE_MIN_PREV_COUNT,
  STATUS_META,
  worstLevel,
  type AttentionLevel,
  type Reason,
  type StatusMeta,
} from '@/lib/services/serviceHealth'
import { hostStatus, formatPct } from '@/lib/infra/hostStats'
import type {
  AlertIncident,
  AlertRule,
  InfraHost,
  Monitor,
  RumVitalRow,
  TenantSummary,
} from '@/lib/core/api'

// --- Attention Level ---------------------------------------------------------------------------

// Down = not answering. Critical = answering badly, user-visible. Warning = degraded, not yet
// breaking. OK = inside every threshold. Unknown is NOT a level — it is the absence of a judgment
// (no traffic, paused, never reported), which is why it ranks last and never colours a row.
export {
  INCIDENT_LEVEL,
  LEVEL_RANK,
  LEVELS,
  needsAttention,
  SILENCE_MIN_PREV_COUNT,
  worstLevel,
  type AttentionLevel,
  type Reason,
}

// The board's name for the one display table. `StatusMeta` already carries the rail, the dot, the
// StatusPill tone and the solid flag — a second table keyed by the same five words would be the
// duplication this change exists to remove.
export const LEVEL_META = STATUS_META
export type LevelMeta = StatusMeta

// --- Worlds & Subjects -------------------------------------------------------------------------

export type WorldKey = 'frontend' | 'backend' | 'infra' | 'ops' | 'sites'
export type SubjectKind = 'service' | 'rumApp' | 'host' | 'monitor' | 'site'

export interface WorldMeta {
  key: WorldKey
  name: string
  /** Plural noun for the roll-up card's total, e.g. "24 services". */
  noun: string
  /** Where "open this world" drills to. */
  dest: string
}

export const WORLDS: WorldMeta[] = [
  { key: 'frontend', name: 'Frontend', noun: 'RUM apps', dest: '/rum' },
  { key: 'backend', name: 'Backend', noun: 'services', dest: '/services' },
  { key: 'infra', name: 'Infra', noun: 'hosts', dest: '/infra' },
  { key: 'ops', name: 'Ops', noun: 'monitors', dest: '/uptime' },
]

export const SITES_WORLD: WorldMeta = { key: 'sites', name: 'Sites', noun: 'sites', dest: '/tenants' }

/**
 * Which Worlds this install can show.
 *
 * `sites` only exists on Central, once a tenant registry exists. `ops` drops out under an active
 * tenant lens: uptime is a self-contained SQLite vertical with NO tenant dimension, so its monitors
 * are always Central's own. Showing them beside `eu-west`'s services under an "eu-west" filter
 * would claim they belong to eu-west, which is a lie the page must not tell.
 */
export function boardWorlds({ sites = false, ops = true }: { sites?: boolean; ops?: boolean } = {}): WorldMeta[] {
  const base = ops ? WORLDS : WORLDS.filter((w) => w.key !== 'ops')
  return sites ? [...base, SITES_WORLD] : base
}

export const KIND_LABEL: Record<SubjectKind, string> = {
  service: 'Service',
  rumApp: 'RUM app',
  host: 'Host',
  monitor: 'Monitor',
  site: 'Site',
}

/** On Central: how many Sites a Service is unhealthy in, out of how many run it. */
export interface Spread {
  bad: number
  total: number
}

export interface Subject {
  /** Stable across polls: `kind:name`. Subjects merged across Sites keep the merged name. */
  id: string
  kind: SubjectKind
  world: WorldKey
  name: string
  /** Sites this Subject is unhealthy in. Empty on a Site install and for Central's own Subjects. */
  sites: string[]
  /** Central's own (untenanted) Subject, on an install that has Sites registered. */
  local: boolean
  level: AttentionLevel
  reasons: Reason[]
  /** When the condition has been true since, when we know it. Null when there is no start time. */
  sinceMs: number | null
  /** Triggered incidents that resolved to this Subject. */
  incidents: number
  spread: Spread | null
  /** Route path only — callers pass it through `correlate()` so the time + tenant ride along. */
  dest: string
}

// --- Silence -----------------------------------------------------------------------------------

// A Subject that reported inside the Watch Range and then stopped is Down: the stopping is the
// news. One that never reported is Unknown — Photon cannot tell a dead machine from an uninstalled
// agent, so it reports which one it observed rather than guessing.
//
// Detection compares against the PREVIOUS equal-length range, so a Subject that has been gone
// longer than two Watch Ranges is absent from both and is not reported at all. That is the honest
// limit of a bounded lookback; widen the Watch Range to see further back.
//
// The threshold (`SILENCE_MIN_PREV_COUNT`, re-exported above) and the service-side rule live in
// `serviceHealth.ts`; hosts get the finer `lastSeenNs` signal below.

/** A host reporting every 15s is Down after 5 minutes of quiet — 20 missed samples. */
export const HOST_SILENCE_MS = 5 * 60_000

// --- Services ----------------------------------------------------------------------------------

// A type alias, NOT an interface: only aliases get TypeScript's implicit index signature, which is
// what makes a `RedRow` off the wire assignable to serviceHealth's `ServiceHealthRow`.
export type ServiceRedRow = {
  service: string
  /** The Site this row came from. Populated for `group=service` only — that is what makes
   *  cross-Site merging free here and unavailable for RUM apps (see the ADR). */
  tenant?: string | null
  count?: number
  rate?: number
  error_rate?: number
  apdex?: number | null
  p50?: string | number | null
  p90?: string | number | null
  p99?: string | number | null
}

/**
 * RED rows → Backend Subjects. Rows arrive already split per Site (`RedRow.tenant` is populated for
 * `group=service`), so same-named rows are merged into ONE Subject carrying a Spread: the same
 * software deployed many times. Hosts and Monitors never merge — see the ADR.
 */
export function serviceSubjects(rows: ServiceRedRow[], prevRows: ServiceRedRow[] = []): Subject[] {
  const prevByKey = new Map<string, ServiceRedRow>()
  for (const r of prevRows) prevByKey.set(`${r.service}\u0000${r.tenant ?? ''}`, r)

  // Every (service, site) pair seen in EITHER range — a site that went silent has no current row.
  const pairs = new Map<string, { service: string; site: string | null; row?: ServiceRedRow }>()
  for (const r of rows) pairs.set(`${r.service}\u0000${r.tenant ?? ''}`, { service: r.service, site: r.tenant ?? null, row: r })
  for (const r of prevRows) {
    const k = `${r.service}\u0000${r.tenant ?? ''}`
    if (!pairs.has(k)) pairs.set(k, { service: r.service, site: r.tenant ?? null })
  }

  const merged = new Map<string, Subject>()
  for (const [key, { service, site, row }] of pairs) {
    // One judgment, made in one place. A missing row is the same fact as a zero row (a Site that
    // went quiet sends neither), so both go through `serviceLevel` with the previous range.
    const { level, reasons } = serviceLevel(row ?? {}, { prev: prevByKey.get(key) })

    const existing = merged.get(service)
    const unhealthy = needsAttention(level)
    if (!existing) {
      merged.set(service, {
        id: `service:${service}`,
        kind: 'service',
        world: 'backend',
        name: service,
        sites: unhealthy && site ? [site] : [],
        local: site == null,
        level,
        reasons,
        sinceMs: null,
        incidents: 0,
        spread: site ? { bad: unhealthy ? 1 : 0, total: 1 } : null,
        dest: `/services/${encodeURIComponent(service)}`,
      })
      continue
    }
    if (site) {
      existing.spread = { bad: (existing.spread?.bad ?? 0) + (unhealthy ? 1 : 0), total: (existing.spread?.total ?? 0) + 1 }
      if (unhealthy) existing.sites.push(site)
    } else {
      existing.local = true
    }
    // The merged row speaks for the worst Site — that is the one someone has to look at.
    if (LEVEL_RANK[level] < LEVEL_RANK[existing.level]) {
      existing.level = level
      existing.reasons = reasons
    }
  }
  return [...merged.values()]
}

// --- RUM apps ----------------------------------------------------------------------------------

/** Total Web-Vital samples behind a vitals row set — the traffic proxy Silence needs. */
export function vitalSamples(vitals: RumVitalRow[] | undefined): number {
  let n = 0
  for (const v of vitals ?? []) n += v.dist?.total ?? (v.dist?.good ?? 0) + (v.dist?.needs ?? 0) + (v.dist?.poor ?? 0)
  return n
}

const VITAL_LABEL: Record<string, string> = {
  'web_vitals.lcp': 'LCP',
  'web_vitals.inp': 'INP',
  'web_vitals.cls': 'CLS',
  'web_vitals.fcp': 'FCP',
  'web_vitals.ttfb': 'TTFB',
}

export function vitalName(metric: string): string {
  return VITAL_LABEL[metric] ?? metric
}

/** CLS is unit-less; every other Web Vital is milliseconds. */
export function vitalText(metric: string, ms: number | null | undefined): string {
  if (ms == null) return '—'
  if (metric === 'web_vitals.cls') return ms.toFixed(2)
  return ms >= 1000 ? (ms / 1000).toFixed(2) + 's' : Math.round(ms) + 'ms'
}

/**
 * One registered RUM app → a Frontend Subject. A poor Web Vital is **Warning, never Critical**: a
 * landing page that cries wolf about latency stops being read. Critical only ever arrives from a
 * triggered incident (applied later by `applyIncidents`).
 */
export function rumAppSubject(app: string, vitals: RumVitalRow[] | undefined, prevVitals?: RumVitalRow[]): Subject {
  const samples = vitalSamples(vitals)
  const dest = `/rum/${encodeURIComponent(app)}`
  const base = {
    id: `rumApp:${app}`,
    kind: 'rumApp' as const,
    world: 'frontend' as const,
    name: app,
    sites: [],
    local: false,
    sinceMs: null,
    incidents: 0,
    spread: null,
    dest,
  }
  if (!samples) {
    const stopped = vitalSamples(prevVitals) >= SILENCE_MIN_PREV_COUNT
    return {
      ...base,
      level: stopped ? 'down' : 'unknown',
      reasons: stopped
        ? [{ label: 'No pageviews', value: String(vitalSamples(prevVitals)), note: 'was' }]
        : [{ label: 'No RUM data', value: '—' }],
    }
  }
  // The API rates each vital itself ('good' | 'needs-improvement' | 'poor') — never re-derive the
  // cutoffs client-side, just colour whatever rating came back.
  const bad = (vitals ?? []).filter((v) => v.rating?.startsWith('poor') || v.rating?.startsWith('needs'))
  if (!bad.length) return { ...base, level: 'ok', reasons: [] }
  return {
    ...base,
    level: 'warning',
    reasons: bad.slice(0, 3).map((v) => ({
      label: `${vitalName(v.metric)} p75`,
      value: vitalText(v.metric, v.p75),
      note: v.rating?.startsWith('poor') ? 'poor' : 'needs improvement',
    })),
  }
}

// --- Hosts -------------------------------------------------------------------------------------

const HOST_LEVEL: Record<string, AttentionLevel> = { error: 'critical', warning: 'warning' }

/**
 * One reporting host → an Infra Subject. Silence has a finer signal here than anywhere else:
 * `lastSeenNs` is on the row, so a host that is still inside the Watch Range but stopped sampling
 * five minutes ago is Down without waiting for it to fall out of the window.
 */
export function hostSubject(h: InfraHost, nowMs: number): Subject {
  const base = {
    id: `host:${h.host}`,
    kind: 'host' as const,
    world: 'infra' as const,
    name: h.host,
    sites: [],
    local: false,
    incidents: 0,
    spread: null,
    dest: `/infra/${encodeURIComponent(h.host)}`,
  }
  const lastSeenMs = h.lastSeenNs ? Number(BigInt(h.lastSeenNs) / 1_000_000n) : null
  if (lastSeenMs != null && nowMs - lastSeenMs > HOST_SILENCE_MS) {
    return {
      ...base,
      level: 'down',
      sinceMs: lastSeenMs,
      reasons: [{ label: 'Last reported', value: compactAge(nowMs - lastSeenMs), note: 'silent' }],
    }
  }
  const status = hostStatus([h.cpuUtil, h.memUtil, h.diskUtil, h.gpuUtil])
  const level = status ? HOST_LEVEL[status] : 'ok'
  const reasons: Reason[] = []
  if (status) {
    const parts: [string, number | null][] = [
      ['CPU', h.cpuUtil],
      ['Memory', h.memUtil],
      ['Disk', h.diskUtil],
      ['GPU', h.gpuUtil],
    ]
    for (const [label, util] of parts) {
      if (util != null && hostStatus([util])) reasons.push({ label, value: formatPct(util), note: status })
    }
  }
  return { ...base, level, sinceMs: null, reasons }
}

/** A host present in the previous range and gone from the current one. */
export function vanishedHostSubject(host: string): Subject {
  return {
    id: `host:${host}`,
    kind: 'host',
    world: 'infra',
    name: host,
    sites: [],
    local: false,
    level: 'down',
    reasons: [{ label: 'Stopped reporting', value: '—', note: 'silent' }],
    sinceMs: null,
    incidents: 0,
    spread: null,
    dest: `/infra/${encodeURIComponent(host)}`,
  }
}

// --- Monitors ----------------------------------------------------------------------------------

/**
 * One uptime monitor → an Ops Subject. A monitor is the one Subject kind whose native state IS the
 * judgment: it is a synthetic probe, so "down" means down, not "probably degraded".
 */
export function monitorSubject(m: Monitor): Subject {
  const state = m.last_state ?? ''
  const level: AttentionLevel = !m.enabled ? 'unknown' : state === 'down' ? 'down' : state === 'up' ? 'ok' : 'unknown'
  const reasons: Reason[] = []
  if (level === 'down') {
    reasons.push({ label: m.type ? m.type.toUpperCase() : 'Probe', value: m.target ?? '', note: 'down' })
  } else if (level === 'unknown') {
    reasons.push({ label: m.enabled ? 'No check yet' : 'Paused', value: '—' })
  }
  return {
    id: `monitor:${m.id}`,
    kind: 'monitor',
    world: 'ops',
    name: m.name,
    sites: [],
    local: false,
    level,
    reasons,
    sinceMs: m.last_check_at ? m.last_check_at * 1000 : null,
    incidents: 0,
    spread: null,
    dest: `/uptime?monitor=${encodeURIComponent(m.id)}`,
  }
}

// --- Sites -------------------------------------------------------------------------------------

const SITE_LEVEL: Record<TenantSummary['status'], AttentionLevel> = {
  down: 'down',
  stale: 'warning',
  up: 'ok',
}

/**
 * One registered Site → a Sites Subject (Central only). The staleness bands are the tenants API's
 * own (`up` < 120s, `stale` < 600s, `down` beyond) — the board does not re-derive them.
 */
export function siteSubject(t: TenantSummary, nowMs: number): Subject {
  const level = SITE_LEVEL[t.status] ?? 'unknown'
  const reasons: Reason[] = []
  if (level !== 'ok') {
    reasons.push({ label: 'Last push', value: t.last_seen_ms ? compactAge(nowMs - t.last_seen_ms) + ' ago' : 'never', note: t.status })
  }
  if (t.open_incidents) {
    reasons.push({ label: 'Open incidents there', value: String(t.open_incidents) })
  }
  return {
    id: `site:${t.name}`,
    kind: 'site',
    world: 'sites',
    name: t.name,
    sites: [],
    local: false,
    level,
    reasons,
    sinceMs: t.last_seen_ms || null,
    incidents: 0,
    spread: null,
    dest: '/tenants',
  }
}

// --- Incidents ---------------------------------------------------------------------------------

/** `k=v,k=v` (canonically sorted, `""` = no labels) → a map. Values may themselves contain `=`. */
export function parseSeriesKey(key: string | null | undefined): Record<string, string> {
  const out: Record<string, string> = {}
  for (const part of (key ?? '').split(',')) {
    if (!part) continue
    const eq = part.indexOf('=')
    if (eq <= 0) continue
    out[part.slice(0, eq)] = part.slice(eq + 1)
  }
  return out
}

/** photon-server's uptime bridge synthesises `uptime:<monitor.id>` as the rule id. */
export const UPTIME_RULE_PREFIX = 'uptime:'

export interface IncidentTarget {
  kind: SubjectKind
  /** For monitors this is the monitor ID, not its display name. */
  name: string
}

/**
 * Which Subject a triggered incident is about. Returns null for an **Unattributed Incident** — a
 * rule genuinely about the fleet (metrics with no `group_by`) or one whose place is not recoverable.
 * Those are shown in their own banner, never hidden.
 */
export function resolveIncident(incident: AlertIncident, rule?: AlertRule): IncidentTarget | null {
  if (incident.rule_id.startsWith(UPTIME_RULE_PREFIX)) {
    return { kind: 'monitor', name: incident.rule_id.slice(UPTIME_RULE_PREFIX.length) }
  }
  if (!rule) return null
  const labels = parseSeriesKey(incident.series_key)
  switch (rule.condition.signal) {
    case 'traces':
      return rule.condition.service ? { kind: 'service', name: rule.condition.service } : null
    case 'rum':
      return rule.condition.app_id ? { kind: 'rumApp', name: rule.condition.app_id } : null
    case 'metrics':
    case 'logs': {
      if (labels['host.name']) return { kind: 'host', name: labels['host.name'] }
      if (labels['service.name']) return { kind: 'service', name: labels['service.name'] }
      return null
    }
    default:
      return null
  }
}

export interface IncidentRef {
  incident: AlertIncident
  rule?: AlertRule
}

export interface IncidentApplication {
  subjects: Subject[]
  /** Resolved to no Subject AT ALL — a rule genuinely about the fleet, or a place we cannot
   *  recover. These are Unattributed Incidents and are always shown. */
  unattributed: IncidentRef[]
  /** Resolved to a real place that is not on this board — a service with no spans in the Watch
   *  Range, or another Site's subject when a tenant lens is active. Kept separate because the
   *  second case is an artifact of the lens, not a fact about the fleet. */
  orphaned: IncidentRef[]
}

/**
 * Fold triggered incidents onto the Subjects they name: raise the level if the incident is worse,
 * count the badge, and take the earliest `started_at` as the row's age. Anything that does not land
 * on a Subject is returned rather than dropped — `unattributed` (about the fleet) or `orphaned`
 * (about a real place that is not on this board) — so the page never goes quiet about something
 * that fired.
 */
export function applyIncidents(
  subjects: Subject[],
  incidents: AlertIncident[],
  rules: AlertRule[],
  monitorNameById: Record<string, string> = {},
): IncidentApplication {
  const ruleById = new Map(rules.map((r) => [r.id, r]))
  const byId = new Map(subjects.map((s) => [s.id, s]))
  const unattributed: IncidentRef[] = []
  const orphaned: IncidentRef[] = []

  for (const inc of incidents) {
    const rule = ruleById.get(inc.rule_id)
    const target = resolveIncident(inc, rule)
    const subject = target ? byId.get(`${target.kind}:${target.name}`) : undefined
    if (!subject) {
      ;(target ? orphaned : unattributed).push({ incident: inc, rule })
      continue
    }
    subject.incidents += 1
    const level = INCIDENT_LEVEL[inc.severity] ?? 'warning'
    if (LEVEL_RANK[level] < LEVEL_RANK[subject.level]) {
      subject.level = level
      subject.reasons = [{ label: inc.summary || rule?.name || 'Alert', value: '', note: inc.severity }, ...subject.reasons]
    }
    const startedMs = inc.started_at * 1000
    if (subject.sinceMs == null || startedMs < subject.sinceMs) subject.sinceMs = startedMs
    if (target?.kind === 'monitor' && monitorNameById[target.name]) subject.name = monitorNameById[target.name]
  }
  return { subjects, unattributed, orphaned }
}

// --- Roll-up & ordering ------------------------------------------------------------------------

export interface WorldRollup {
  world: WorldMeta
  total: number
  tallies: Record<AttentionLevel, number>
  worst: AttentionLevel
}

export function rollup(subjects: Subject[], worlds: WorldMeta[] = WORLDS): WorldRollup[] {
  return worlds.map((world) => {
    const tallies: Record<AttentionLevel, number> = { down: 0, critical: 0, warning: 0, ok: 0, unknown: 0 }
    let total = 0
    for (const s of subjects) {
      if (s.world !== world.key) continue
      tallies[s.level] += 1
      total += 1
    }
    return { world, total, tallies, worst: worstLevel(...LEVELS.filter((l) => tallies[l] > 0)) }
  })
}

/** Worst first; then most incidents; then oldest; then by name so ties are stable across polls. */
export function compareSubjects(a: Subject, b: Subject): number {
  const r = LEVEL_RANK[a.level] - LEVEL_RANK[b.level]
  if (r) return r
  if (a.incidents !== b.incidents) return b.incidents - a.incidents
  const sa = a.sinceMs ?? Infinity
  const sb = b.sinceMs ?? Infinity
  if (sa !== sb) return sa - sb
  return a.name.localeCompare(b.name)
}

export function attentionList(subjects: Subject[], world: WorldKey | null = null): Subject[] {
  return subjects
    .filter((s) => needsAttention(s.level) && (!world || s.world === world))
    .sort(compareSubjects)
}

// --- Formatting ---------------------------------------------------------------------------------

/** A duration as one glanceable unit: "42s", "18m", "3h", "2d". Never two units. */
export function compactAge(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return '—'
  const s = Math.max(0, Math.round(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.round(s / 60)
  if (m < 60) return `${m}m`
  const h = Math.round(m / 60)
  if (h < 48) return `${h}h`
  return `${Math.round(h / 24)}d`
}

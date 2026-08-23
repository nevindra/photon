// The single Attention Level model. Pure and table-tested — no Vue, no I/O.
//
// One vocabulary for the whole app. This module judges a RED-shaped row ({ count, error_rate,
// apdex }) into ONE of five ranked levels, and `/home`'s Attention Board (`lib/home/attention.ts`)
// imports that judgment instead of re-deriving its own. Two vocabularies for the same subject is
// how `payments-api` came to read "Critical" on one page and "Degraded" on another in the same
// second — see docs/adr/0004-the-services-index-is-a-fleet-board.md.
//
// `idle` is retired. It was one word covering two different facts: a service that reported 5,000
// req/s and stopped (Down — the stopping IS the news) and one that never reported at all (Unknown —
// Photon cannot tell a dead service from an undeployed one). Both used to render as calm neutral
// grey, which hid the outage at the exact moment it mattered.
//
// Thresholds stay exported constants so a future per-service SLO/settings feature can override them
// without touching call sites.
import type { AlertIncident, AlertSeverity } from '@/lib/core/api'

// Error-rate thresholds (fraction 0..1).
export const ERROR_CRITICAL = 0.05
export const ERROR_DEGRADED = 0.01
// Apdex thresholds (score 0..1; lower is worse). Apdex already encodes the latency SLO relative to
// the per-service Apdex threshold T, so it is our latency-health signal — no separate p99 rule.
export const APDEX_CRITICAL = 0.7
export const APDEX_DEGRADED = 0.85

/** Below this many events in the previous window, "it stopped" is noise, not news. */
export const SILENCE_MIN_PREV_COUNT = 10

// --- The axis --------------------------------------------------------------------------------

// Down = not answering. Critical = answering badly, user-visible. Warning = degraded, not yet
// breaking. OK = inside every threshold. Unknown is NOT a level — it is the absence of a judgment
// (no traffic, blind, never reported), which is why it ranks last and never colours a row.
export type AttentionLevel = 'down' | 'critical' | 'warning' | 'ok' | 'unknown'

export const LEVELS: AttentionLevel[] = ['down', 'critical', 'warning', 'ok', 'unknown']

export const LEVEL_RANK: Record<AttentionLevel, number> = {
  down: 0,
  critical: 1,
  warning: 2,
  ok: 3,
  unknown: 4,
}

/** Worst (lowest-ranked) of the given levels. `unknown` loses to every real judgment. */
export function worstLevel(...levels: (AttentionLevel | null | undefined)[]): AttentionLevel {
  let worst: AttentionLevel = 'unknown'
  for (const l of levels) {
    if (!l) continue
    if (LEVEL_RANK[l] < LEVEL_RANK[worst]) worst = l
  }
  return worst
}

/** The three levels a board or a strip leads with. OK and Unknown are counted, never listed. */
export function needsAttention(level: AttentionLevel): boolean {
  return level === 'down' || level === 'critical' || level === 'warning'
}

// One clause of a reason line. Rendered as "Error rate **8.2%** (critical)" — the number is what
// the reader scans for, the note is the word that clause earned on its own.
export interface Reason {
  label: string
  value: string
  note?: string
}

/** One clause as a display string ("Error rate 8.2%"), for banners and table tooltips. */
export function reasonText(r: Reason): string {
  return r.value && r.value !== '—' ? `${r.label} ${r.value}` : r.label
}

// A triggered warning/info incident is still something a human asked to be told about, so it
// reaches the list — as Warning, so it can never outrank a real outage.
export const INCIDENT_LEVEL: Record<AlertSeverity, AttentionLevel> = {
  critical: 'critical',
  warning: 'warning',
  info: 'warning',
}

// --- Display metadata ---------------------------------------------------------------------------

export interface StatusMeta {
  label: string
  /** StatusPill tone. Down reuses the error tone but renders SOLID — see `solid`. */
  tone: 'error' | 'warning' | 'success' | 'neutral'
  /** Down is the only filled pill on a page, so "not answering" never reads as "a bit slow". */
  solid: boolean
  /** Literal class strings — Tailwind's content scanner cannot see computed names. */
  rail: string
  dot: string
  text: string
  soft: string
}

// level → display metadata. Down and Critical share `--sev-error` and are told apart by weight (a
// solid fill vs a soft tint), Warning is `--sev-warn`, OK is `--success` (the positive counterpart
// to severity), Unknown is muted because the absence of a judgment must not look like one.
// `lib/home/attention.ts` re-exports this table as `LEVEL_META`; there is only one of it.
export const STATUS_META: Record<AttentionLevel, StatusMeta> = {
  down: { label: 'Down', tone: 'error', solid: true, rail: 'bg-sev-error', dot: 'bg-sev-error', text: 'text-sev-error', soft: 'bg-sev-error-soft' },
  critical: { label: 'Critical', tone: 'error', solid: false, rail: 'bg-sev-error', dot: 'bg-sev-error', text: 'text-sev-error', soft: 'bg-sev-error-soft' },
  warning: { label: 'Warning', tone: 'warning', solid: false, rail: 'bg-sev-warn', dot: 'bg-sev-warn', text: 'text-sev-warn', soft: 'bg-sev-warn-soft' },
  ok: { label: 'OK', tone: 'success', solid: false, rail: 'bg-success', dot: 'bg-success', text: 'text-success', soft: 'bg-success-soft' },
  unknown: { label: 'Unknown', tone: 'neutral', solid: false, rail: 'bg-muted-foreground/40', dot: 'bg-muted-foreground/60', text: 'text-muted-foreground', soft: 'bg-muted' },
}

// --- The row ------------------------------------------------------------------------------------

// A RED-shaped row as produced by the services/APM query layer. Rows in the wild carry additional
// fields (service name, error_count, p50/p90/p99 latency strings, etc.) that these pure functions
// don't need — the index signature keeps those legal without widening the fields we do read to
// `any`.
export interface ServiceHealthRow {
  count?: number
  error_rate?: number
  apdex?: number | null
  rate?: number
  [key: string]: unknown
}

/** Error rate and Apdex are judged separately; the row takes the worse of the two. */
function errorLevel(er: number): AttentionLevel {
  if (er >= ERROR_CRITICAL) return 'critical'
  if (er >= ERROR_DEGRADED) return 'warning'
  return 'ok'
}

/** `null` when there is no Apdex to judge on — not a verdict, so the error rate decides alone. */
function apdexLevel(ap: number | null | undefined): AttentionLevel | null {
  if (ap == null) return null
  if (ap < APDEX_CRITICAL) return 'critical'
  if (ap < APDEX_DEGRADED) return 'warning'
  return 'ok'
}

// The crossed conditions, each keeping the word it earned on its own. A row that is Critical on
// errors and Warning on Apdex says exactly that instead of tagging both with the row's verdict.
function bandReasons(row: ServiceHealthRow): Reason[] {
  const out: Reason[] = []
  const er = row.error_rate ?? 0
  const el = errorLevel(er)
  if (el !== 'ok') out.push({ label: 'Error rate', value: `${(er * 100).toFixed(1)}%`, note: el })
  const ap = row.apdex
  const al = apdexLevel(ap)
  if (ap != null && al && al !== 'ok') out.push({ label: 'Apdex', value: ap.toFixed(2), note: al })
  return out
}

/** A rate as the reason line renders it: "340/s". */
function rateText(rate: number | undefined): string {
  const n = rate ?? 0
  return (n >= 100 ? Math.round(n).toLocaleString('en-US') : n.toFixed(1)) + '/s'
}

export interface ServiceLevelContext {
  /** The previous equal-length window. Present = Silence can be told from never-reported. */
  prev?: ServiceHealthRow
  /** The Site this row came from stopped reporting: the numbers are blind, not healthy. */
  siteStale?: boolean
  /** Triggered incidents already resolved to this subject (see attention.ts's resolveIncident). */
  incidents?: AlertIncident[]
}

export interface ServiceLevelResult {
  level: AttentionLevel
  reasons: Reason[]
}

/**
 * Judge one RED row. Rules, in precedence order:
 *
 * 1. A stale Site is Unknown, whatever the row says.
 * 2. Traffic in `prev` and none now is Down (**Silence**).
 * 3. No traffic at either end is Unknown.
 * 4. A firing incident may RAISE the level, never lower it.
 * 5. Otherwise error rate or Apdex, whichever is worse.
 *
 * Rules 4 and 5 run in the opposite order to the list because "raise" needs something to raise
 * from: the bands are computed first, then the incident is folded on top and only wins if it is
 * worse.
 */
export function serviceLevel(row: ServiceHealthRow = {}, ctx: ServiceLevelContext = {}): ServiceLevelResult {
  // 1. A Site that stopped pushing makes every number under it blind, not good: read the row and
  //    you see zero errors on zero traffic, which renders as calm at the one moment the page knows
  //    nothing. Incidents do not rescue it either — the alert engine evaluates the same absent
  //    data, so its state is exactly as stale as the row. This is why it short-circuits.
  if (ctx.siteStale) {
    return { level: 'unknown', reasons: [{ label: 'No data', value: '—', note: 'site stale' }] }
  }

  let level: AttentionLevel
  let reasons: Reason[]

  if ((row.count ?? 0) > 0) {
    // 5. Whichever of error rate and Apdex is worse decides the row.
    level = worstLevel('ok', errorLevel(row.error_rate ?? 0), apdexLevel(row.apdex))
    reasons = bandReasons(row)
  } else if ((ctx.prev?.count ?? 0) >= SILENCE_MIN_PREV_COUNT) {
    // 2. Silence. It reported, then stopped — the stopping is itself the news, so this is Down and
    //    not a neutral "nothing to see here".
    level = 'down'
    reasons = [{ label: 'No spans', value: rateText(ctx.prev?.rate), note: 'was' }]
  } else {
    // 3. Nothing at either end. Photon cannot tell a dead service from one that was never deployed,
    //    so it reports what it observed rather than guessing.
    level = 'unknown'
    reasons = [{ label: 'No traffic', value: '—' }]
  }

  // 4. Somebody asked to be told about this, so a firing incident outranks a clean-looking window —
  //    but a Warning-mapped incident never talks a Critical row down, and nothing outranks Down.
  for (const inc of ctx.incidents ?? []) {
    const incLevel = INCIDENT_LEVEL[inc.severity] ?? 'warning'
    if (LEVEL_RANK[incLevel] >= LEVEL_RANK[level]) continue
    level = incLevel
    reasons = [{ label: inc.summary || 'Alert', value: '', note: inc.severity }, ...reasons]
  }

  return { level, reasons }
}

/**
 * The crossed threshold conditions as plain strings ("Error rate 8.2%"). Kept as its own export
 * because the banner and the table tooltip want the sentence, not the structure.
 */
export function healthReasons(row: ServiceHealthRow = {}): string[] {
  return bandReasons(row).map(reasonText)
}

export interface ServiceHealthResult {
  level: AttentionLevel
  meta: StatusMeta
  reasons: Reason[]
  /** The same clauses as display strings, for banners and tooltips. */
  text: string[]
}

export function serviceHealth(row: ServiceHealthRow = {}, ctx: ServiceLevelContext = {}): ServiceHealthResult {
  const { level, reasons } = serviceLevel(row, ctx)
  return { level, meta: STATUS_META[level], reasons, text: reasons.map(reasonText) }
}

/** Tally rows by level for the fleet summary chips. */
export function healthCounts(rows: ServiceHealthRow[] = []): Record<AttentionLevel, number> {
  const c: Record<AttentionLevel, number> = { down: 0, critical: 0, warning: 0, ok: 0, unknown: 0 }
  for (const r of rows) c[serviceLevel(r).level]++
  return c
}

// Worst-first comparator: level rank, then error_rate desc, then apdex asc (null = least risky,
// sorts last within a tie), then rate desc.
export function compareWorst(a: ServiceHealthRow, b: ServiceHealthRow): number {
  const ra = LEVEL_RANK[serviceLevel(a).level]
  const rb = LEVEL_RANK[serviceLevel(b).level]
  if (ra !== rb) return ra - rb
  const ea = a.error_rate ?? 0
  const eb = b.error_rate ?? 0
  if (ea !== eb) return eb - ea
  const aa = a.apdex == null ? Infinity : a.apdex
  const ab = b.apdex == null ? Infinity : b.apdex
  if (aa !== ab) return aa - ab
  return (b.rate ?? 0) - (a.rate ?? 0)
}

export function byWorstFirst(rows: ServiceHealthRow[] = []): ServiceHealthRow[] {
  return [...rows].sort(compareWorst)
}

/** The worst services for the "needs attention" strip, worst-first, capped at `max`. */
export function attentionServices(rows: ServiceHealthRow[] = [], max: number = 3): ServiceHealthRow[] {
  return byWorstFirst(rows.filter((r) => needsAttention(serviceLevel(r).level))).slice(0, max)
}

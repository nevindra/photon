// Composite service-health model. Pure and table-tested. Classifies a RED-shaped row
// ({ count, error_rate, apdex }) into a status + human-readable reasons, shared by the Services
// list (health column, fleet counts, attention strip) and the Service-detail banner. Colour is
// reserved for severity (see STATUS_META). Thresholds are exported constants so a future
// per-service SLO/settings feature can override them without touching call sites.

// Error-rate thresholds (fraction 0..1).
export const ERROR_CRITICAL = 0.05
export const ERROR_DEGRADED = 0.01
// Apdex thresholds (score 0..1; lower is worse). Apdex already encodes the latency SLO relative to
// the per-service Apdex threshold T, so it is our latency-health signal — no separate p99 rule.
export const APDEX_CRITICAL = 0.7
export const APDEX_DEGRADED = 0.85

export type ServiceStatus = 'critical' | 'degraded' | 'healthy' | 'idle'

export interface StatusMeta {
  label: string
  rank: number
  dot: string
  text: string
  soft: string
}

// status → display metadata. `rank` drives worst-first ordering. Class strings are LITERAL so
// Tailwind's content scanner keeps them. `dot`/`text`/`soft` reuse the app's severity palette;
// "healthy" uses the --success token ("good") — the positive counterpart to severity.
export const STATUS_META: Record<ServiceStatus, StatusMeta> = {
  critical: { label: 'Critical', rank: 0, dot: 'bg-sev-error', text: 'text-sev-error', soft: 'bg-sev-error-soft' },
  degraded: { label: 'Degraded', rank: 1, dot: 'bg-sev-warn', text: 'text-sev-warn', soft: 'bg-sev-warn-soft' },
  healthy: { label: 'Healthy', rank: 2, dot: 'bg-success', text: 'text-success', soft: 'bg-success-soft' },
  idle: { label: 'Idle', rank: 3, dot: 'bg-muted-foreground/50', text: 'text-muted-foreground', soft: 'bg-muted' },
}

// A RED-shaped row as produced by the services/APM query layer. Rows in the wild carry additional
// fields (service name, rate, error_count, p50/p90/p99 latency strings, etc.) that these pure
// functions don't need — the index signature keeps those legal without widening the fields we do
// read to `any`.
export interface ServiceHealthRow {
  count?: number
  error_rate?: number
  apdex?: number | null
  rate?: number
  [key: string]: unknown
}

// Classify one RED row. No traffic → idle (neutral, never coloured). Otherwise error rate OR apdex
// (whichever is worse) decides the band.
export function serviceStatus({ count, error_rate, apdex }: ServiceHealthRow = {}): ServiceStatus {
  if (!count) return 'idle'
  const er = error_rate ?? 0
  const ap = apdex
  if (er >= ERROR_CRITICAL || (ap != null && ap < APDEX_CRITICAL)) return 'critical'
  if (er >= ERROR_DEGRADED || (ap != null && ap < APDEX_DEGRADED)) return 'degraded'
  return 'healthy'
}

// Short human-readable reasons for the crossed conditions — used verbatim in the detail banner and
// the table health-cell tooltip. Empty when healthy/idle.
export function healthReasons({ error_rate, apdex }: ServiceHealthRow = {}): string[] {
  const out: string[] = []
  const er = error_rate ?? 0
  if (er >= ERROR_DEGRADED) out.push(`Error rate ${(er * 100).toFixed(1)}%`)
  if (apdex != null && apdex < APDEX_DEGRADED) out.push(`Apdex ${apdex.toFixed(2)}`)
  return out
}

export interface ServiceHealthResult {
  status: ServiceStatus
  meta: StatusMeta
  reasons: string[]
}

export function serviceHealth(row: ServiceHealthRow = {}): ServiceHealthResult {
  const status = serviceStatus(row)
  return { status, meta: STATUS_META[status], reasons: healthReasons(row) }
}

// Tally rows by status for the fleet summary chips.
export function healthCounts(rows: ServiceHealthRow[] = []): Record<ServiceStatus, number> {
  const c: Record<ServiceStatus, number> = { critical: 0, degraded: 0, healthy: 0, idle: 0 }
  for (const r of rows) c[serviceStatus(r)]++
  return c
}

// Worst-first comparator: status rank, then error_rate desc, then apdex asc (null = least risky,
// sorts last within a tie), then rate desc.
export function compareWorst(a: ServiceHealthRow, b: ServiceHealthRow): number {
  const ra = STATUS_META[serviceStatus(a)].rank
  const rb = STATUS_META[serviceStatus(b)].rank
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

// The worst non-healthy services for the "needs attention" strip, worst-first, capped at `max`.
export function attentionServices(rows: ServiceHealthRow[] = [], max: number = 3): ServiceHealthRow[] {
  return byWorstFirst(rows.filter((r) => {
    const s = serviceStatus(r)
    return s === 'critical' || s === 'degraded'
  })).slice(0, max)
}

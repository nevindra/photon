// The Services index's data layer: one request (`GET /api/services/overview`), then the fleet
// merge that turns per-Site rows into the board ADR-0004 describes.
//
// The merge is the interesting part. Rows arrive one per `(service, Site, environment)`, because
// that is what the backend can honestly produce. The board wants one row per
// `(service, environment)` carrying **Spread** — how many Sites this service is unhealthy in —
// because that is the question you cannot ask from inside a single Site: `payments-api` unhealthy
// in three Sites out of three points at the code; unhealthy in one out of three points at that
// Site.
//
// Two rules from CONTEXT.md that this file must not quietly break:
//
//   1. **Only Services merge across Sites.** They and RUM apps are the only Subjects that are one
//      piece of software deployed many times. A Host or a Monitor in two Sites is two things.
//   2. **A stale Site's rows are Unknown, never OK.** A Site that stopped reporting has no recent
//      data; rendering that as `0/s` and green is the failure ADR-0004 exists to prevent. The
//      merge marks them, and it excludes them from Spread's denominator — "unhealthy in 1 of 3"
//      is a lie when one of the three has not been heard from.
import { computed, toValue, type MaybeRefOrGetter } from 'vue'
import { useQuery, keepPreviousData } from '@tanstack/vue-query'
import { api, type OverviewRow, type OverviewSite, type ServicesOverview, type SparkPoint } from '@/lib/core/api'
import { contextQueryTerm, withContext } from '@/lib/core/context'
import {
  serviceLevel,
  worstLevel,
  needsAttention,
  LEVEL_RANK,
  type AttentionLevel,
  type Reason,
} from '@/lib/services/serviceHealth'
import type { AlertIncident } from '@/lib/core/api'
import { useIncidents, useRules } from '@/lib/alertsQueries'
import { resolveIncident } from '@/lib/home/attention'

export const servicesOverviewQueryKey = (
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number>,
) => [
  'services',
  'overview',
  toValue(query),
  String(toValue(startNs)),
  String(toValue(endNs)),
  toValue(buckets),
]

// Polled like the monitors dashboard: the index has no search-bar/drawer pause logic to gate on.
export function useServicesOverview(
  query: MaybeRefOrGetter<string>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
  buckets: MaybeRefOrGetter<number> = 30,
) {
  return useQuery({
    queryKey: computed(() => [
      ...servicesOverviewQueryKey(query, startNs, endNs, buckets),
      contextQueryTerm(),
    ]),
    queryFn: ({ signal }) =>
      api.servicesOverview(
        withContext(toValue(query)),
        toValue(startNs),
        toValue(endNs),
        toValue(buckets),
        { signal },
      ),
    refetchInterval: 15_000,
    // Keep the last page on screen while a new window or Site is fetched. Without this the Site
    // band vanishes the instant you click a card in it — the band is derived from this query, so a
    // pending state blanks the very control you just used.
    placeholderData: keepPreviousData,
  })
}

/**
 * Triggered incidents grouped by the service they name.
 *
 * Attribution runs through `attention.ts`'s `resolveIncident` rather than a second copy of the
 * rules — a rule's Subject depends on the RULE (its signal, its `service`, its group-by labels),
 * not on the incident alone, and two implementations of that is how `/home` and `/services` came
 * to disagree about which service was on fire.
 */
export function useIncidentsByService() {
  const incidents = useIncidents(() => ({ status: 'triggered' as const }))
  const rules = useRules()
  return computed<Record<string, AlertIncident[]>>(() => {
    const byId = new Map((rules.data.value ?? []).map((r) => [r.id, r]))
    const out: Record<string, AlertIncident[]> = {}
    for (const inc of incidents.data.value ?? []) {
      const target = resolveIncident(inc, byId.get(inc.rule_id))
      if (target?.kind !== 'service') continue
      ;(out[target.name] ??= []).push(inc)
    }
    return out
  })
}

/** How many Sites a service is unhealthy in, out of how many are actually reporting. */
export interface Spread {
  unhealthy: number
  /** Reporting Sites only. A stale Site is not counted — we do not know its state. */
  total: number
  /** Sites excluded from `total` because they stopped reporting. */
  stale: number
}

/** One Site's contribution to a merged row. */
export interface SiteSlice extends OverviewRow {
  level: AttentionLevel
  reasons: Reason[]
  /** The Site stopped reporting, so this slice's numbers are absent rather than zero. */
  siteStale: boolean
}

/** One row of the board: a service in an environment, across every Site it runs in. */
export interface FleetRow {
  service: string
  environment: string | null
  /** Stable identity for `:key` and for the expansion toggle. */
  id: string
  level: AttentionLevel
  reasons: Reason[]
  spread: Spread
  /** Per-Site child rows, worst first. Length 1 on a Site install. */
  sites: SiteSlice[]
  // Fleet totals over the reporting Sites.
  count: number
  span_count: number
  error_count: number
  error_rate: number
  /** Summed across Sites. Beside `error_count`, never instead of it: a 4xx is the caller's bad
   *  request, a 5xx is this service's own fault, and one number for both hides which. */
  http_4xx: number
  http_5xx: number
  /** Worst percentile across Sites, in nanoseconds — the number a person acts on. */
  p50: number
  p90: number
  p99: number
  apdex: number | null
  satisfied: number
  tolerating: number
  frustrated: number
  /** Summed per bucket across Sites. */
  spark: SparkPoint[]
  error_spark: SparkPoint[]
  /** True when ANY contributing Site reported a non-entry basis — earns a `span-based` badge. */
  spanBased: boolean
  incidents: AlertIncident[]
}

const num = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0)
const ns = (v: unknown): number => {
  // Percentiles cross the wire as decimal-nanosecond STRINGS (JS-safe); anything else is 0.
  const n = typeof v === 'string' ? Number(v) : typeof v === 'number' ? v : NaN
  return Number.isFinite(n) ? n : 0
}

/**
 * The rollup's remainder bucket (`red_rollup.rs`'s `OTHER_OPERATION`).
 *
 * It is a SUM over everything outside a service's busiest operations, not an endpoint. The UI must
 * render it as such: never linkable, never presented as something a person can drill into.
 */
export const OTHER_OPERATION = '__other__'

/** `(service, environment)` — the board's row identity. */
export function rowId(service: string, environment: string | null | undefined): string {
  return `${service}\u0000${environment ?? ''}`
}

/** Site names that are not currently reporting, from the `sites` band. */
export function staleSites(sites: OverviewSite[] = []): Set<string> {
  return new Set(sites.filter((s) => s.status !== 'up').map((s) => s.name))
}

/** Sum two `[ms, value]` series bucket-by-bucket, aligning on the timestamp. */
function mergeSpark(series: SparkPoint[][]): SparkPoint[] {
  const byTs = new Map<number, number>()
  for (const s of series) {
    for (const [t, v] of s ?? []) byTs.set(t, (byTs.get(t) ?? 0) + num(v))
  }
  return [...byTs.entries()].sort((a, b) => a[0] - b[0])
}

export interface MergeContext {
  /** Previous-window rows, keyed the same way, for the silence check. */
  prev?: OverviewRow[]
  /**
   * Triggered incidents already resolved to a service name. Pre-resolved by the caller rather than
   * resolved here: mapping an incident to its Subject needs the alert RULE as well as the incident
   * (`attention.ts`'s `resolveIncident`), and duplicating that logic is exactly how two pages start
   * disagreeing about which service is on fire.
   */
  incidentsByService?: Record<string, AlertIncident[]>
}

/**
 * Per-Site rows → the board's rows.
 *
 * `prev` powers the Silence rule: a service that reported thousands of requests last window and
 * nothing this one is **Down**, not idle. `serviceLevel` owns that judgment; this function only
 * routes the right previous row to it.
 */
export function mergeFleet(
  overview: ServicesOverview | null | undefined,
  ctx: MergeContext = {},
): FleetRow[] {
  const rows = overview?.rows ?? []
  const stale = staleSites(overview?.sites ?? [])
  const prevByKey = new Map<string, OverviewRow>()
  for (const p of ctx.prev ?? []) {
    prevByKey.set(`${rowId(p.service, p.environment)}\u0000${p.tenant ?? ''}`, p)
  }

  const groups = new Map<string, OverviewRow[]>()
  for (const r of rows) {
    const id = rowId(r.service, r.environment)
    const bucket = groups.get(id)
    if (bucket) bucket.push(r)
    else groups.set(id, [r])
  }

  const out: FleetRow[] = []
  for (const [id, group] of groups) {
    const first = group[0]
    const incidents = ctx.incidentsByService?.[first.service] ?? []

    const slices: SiteSlice[] = group.map((r) => {
      const siteStale = !!r.tenant && stale.has(r.tenant)
      const prev = prevByKey.get(`${id}\u0000${r.tenant ?? ''}`)
      const { level, reasons } = serviceLevel(
        {
          service: r.service,
          count: r.count,
          rate: r.rate,
          error_rate: r.error_rate,
          apdex: r.apdex ?? null,
        },
        {
          prev: prev && { count: prev.count, rate: prev.rate, error_rate: prev.error_rate },
          siteStale,
          incidents,
        },
      )
      return { ...r, level, reasons, siteStale }
    })
    slices.sort((a, b) => LEVEL_RANK[a.level] - LEVEL_RANK[b.level])

    // Only reporting Sites contribute numbers. A stale Site's zeros are absence of data, and
    // averaging them in would drag a healthy fleet's error rate and Apdex toward nothing.
    const live = slices.filter((s) => !s.siteStale)
    const basis = live.length ? live : slices
    const count = basis.reduce((a, s) => a + num(s.count), 0)
    const errors = basis.reduce((a, s) => a + num(s.error_count), 0)
    const bands = {
      satisfied: basis.reduce((a, s) => a + num(s.satisfied), 0),
      tolerating: basis.reduce((a, s) => a + num(s.tolerating), 0),
      frustrated: basis.reduce((a, s) => a + num(s.frustrated), 0),
    }
    const banded = bands.satisfied + bands.tolerating + bands.frustrated

    out.push({
      service: first.service,
      environment: first.environment ?? null,
      id,
      level: worstLevel(...slices.map((s) => s.level)),
      // The worst Site's reasons: showing every Site's would bury the one that matters.
      reasons: slices[0]?.reasons ?? [],
      spread: {
        unhealthy: live.filter((s) => needsAttention(s.level)).length,
        total: live.length,
        stale: slices.length - live.length,
      },
      sites: slices,
      count,
      span_count: basis.reduce((a, s) => a + num(s.span_count), 0),
      error_count: errors,
      error_rate: count > 0 ? errors / count : 0,
      http_4xx: basis.reduce((a, s) => a + num(s.http_4xx), 0),
      http_5xx: basis.reduce((a, s) => a + num(s.http_5xx), 0),
      // Worst, not averaged: an average of two Sites' p99s is not a percentile of anything.
      p50: basis.reduce((a, s) => Math.max(a, ns(s.p50)), 0),
      p90: basis.reduce((a, s) => Math.max(a, ns(s.p90)), 0),
      p99: basis.reduce((a, s) => Math.max(a, ns(s.p99)), 0),
      apdex: banded > 0 ? (bands.satisfied + bands.tolerating / 2) / banded : null,
      ...bands,
      spark: mergeSpark(basis.map((s) => s.spark ?? [])),
      error_spark: mergeSpark(basis.map((s) => s.error_spark ?? [])),
      spanBased: basis.some((s) => s.basis && s.basis !== 'entry'),
      incidents,
    })
  }

  out.sort(compareRows)
  return out
}

// --- The Site band -----------------------------------------------------------------------------

/**
 * One card in the Site band. Every number is derived from the SAME merged rows the table renders,
 * so the band and the table can never disagree inside one response — the backend sends only the
 * registry facts (liveness, mode, UI url) that no row could carry.
 */
export interface SiteStat {
  /** The tenant id to filter by. `null` on the All-sites card. */
  key: string | null
  name: string
  /** `null` on the All-sites card: "every Site at once" has no single liveness. */
  status: 'up' | 'stale' | 'down' | null
  mode: string | null
  lastSeenMs: number | null
  uiUrl: string | null
  /** The Site stopped reporting; its numbers below are absence, not zero. */
  stale: boolean
  services: number
  requests: number
  rate: number
  errors: number
  errorRate: number
  /** Services in this Site that want a human's attention. */
  attention: number
  /** Worst Attention Level anywhere in this Site — the card's own colour. */
  level: AttentionLevel
  spark: SparkPoint[]
}

function statFrom(
  key: string | null,
  name: string,
  slices: SiteSlice[],
  site?: OverviewSite,
): SiteStat {
  const requests = slices.reduce((a, s) => a + num(s.count), 0)
  const errors = slices.reduce((a, s) => a + num(s.error_count), 0)
  const stale = !!site && site.status !== 'up'
  return {
    key,
    name,
    status: site?.status ?? null,
    mode: site?.mode ?? null,
    lastSeenMs: site?.last_seen_ms ?? null,
    uiUrl: site?.ui_url ?? null,
    stale,
    // A stale Site's rows are Unknown, so counting them as services "running here" would be a
    // claim we cannot back. The count stays — it is what we last saw — but the level does not.
    services: slices.length,
    requests,
    rate: slices.reduce((a, s) => a + num(s.rate), 0),
    errors,
    errorRate: requests > 0 ? errors / requests : 0,
    attention: slices.filter((s) => needsAttention(s.level)).length,
    level: worstLevel(...slices.map((s) => s.level)),
    spark: mergeSpark(slices.map((s) => s.spark ?? [])),
  }
}

/**
 * The band: an All-sites card, then one card per registered Site, worst first.
 *
 * Empty on a Site install — `overview.sites` is the tenant registry, and a Site install has none.
 * The band then simply does not render, which is what makes one page work in both deployments.
 */
export function siteStats(
  overview: ServicesOverview | null | undefined,
  rows: FleetRow[] = [],
): SiteStat[] {
  const sites = overview?.sites ?? []
  if (!sites.length) return []

  const all = rows.flatMap((r) => r.sites)
  const byTenant = new Map<string, SiteSlice[]>()
  for (const s of all) {
    if (!s.tenant) continue
    const bucket = byTenant.get(s.tenant)
    if (bucket) bucket.push(s)
    else byTenant.set(s.tenant, [s])
  }

  const cards = sites.map((site) => statFrom(site.name, site.name, byTenant.get(site.name) ?? [], site))
  cards.sort((a, b) => {
    const byLevel = LEVEL_RANK[a.level] - LEVEL_RANK[b.level]
    if (byLevel !== 0) return byLevel
    return a.name.localeCompare(b.name)
  })

  // The All card counts MERGED services, not slices: `payments-api` in three Sites is one service
  // the fleet runs, and saying "3 services" there would contradict the table right below it.
  const allCard = statFrom(null, 'All sites', all)
  allCard.services = rows.length
  allCard.attention = rows.filter((r) => needsAttention(r.level)).length
  allCard.level = worstLevel(...rows.map((r) => r.level))
  return [allCard, ...cards]
}

/** Worst first: by Attention Level, then by error rate, then by name for a stable order. */
export function compareRows(a: FleetRow, b: FleetRow): number {
  const byLevel = LEVEL_RANK[a.level] - LEVEL_RANK[b.level]
  if (byLevel !== 0) return byLevel
  if (b.error_rate !== a.error_rate) return b.error_rate - a.error_rate
  return a.service.localeCompare(b.service)
}

/**
 * Rows that want a human's attention, worst first.
 *
 * Sorts rather than trusting the caller: `mergeFleet` already returns worst-first, but a strip that
 * silently depends on that would put the wrong eight rows on screen the first time somebody hands
 * it a filtered or re-sorted list — and a cap that keeps the wrong rows is worse than no cap.
 */
export function attentionRows(rows: FleetRow[] = [], max = 8): FleetRow[] {
  return rows
    .filter((r) => needsAttention(r.level))
    .sort(compareRows)
    .slice(0, max)
}

/** Level counts across the board, for the KPI band. */
export function levelCounts(rows: FleetRow[] = []): Record<AttentionLevel, number> {
  const out = { down: 0, critical: 0, warning: 0, ok: 0, unknown: 0 } as Record<
    AttentionLevel,
    number
  >
  for (const r of rows) out[r.level] += 1
  return out
}

/** Fleet-wide totals for the KPI band, over the same rows the table shows. */
export interface FleetTotals {
  requests: number
  errorRate: number
  p99: number
  services: number
  spanBased: number
}

export function fleetTotals(rows: FleetRow[] = []): FleetTotals {
  const requests = rows.reduce((a, r) => a + r.count, 0)
  const errors = rows.reduce((a, r) => a + r.error_count, 0)
  return {
    requests,
    errorRate: requests > 0 ? errors / requests : 0,
    // Worst across services, for the same reason it is worst across Sites.
    p99: rows.reduce((a, r) => Math.max(a, r.p99), 0),
    services: rows.length,
    spanBased: rows.filter((r) => r.spanBased).length,
  }
}

// The Attention Board's data layer: fan out every signal over the Watch Range, judge each watched
// thing into a Subject (pure logic in `attention.ts`), and derive the vital-signs strip. Nothing
// here decides what "bad" means — it only wires the existing per-signal models together.
//
// Every query is also fetched over the PREVIOUS equal-length range. That second copy pays for two
// things at once: the vital-signs deltas, and Silence detection (a Subject that reported and then
// stopped is Down — see the ADR).
import { computed, type ComputedRef } from 'vue'
import { useServicesList } from '@/lib/services/servicesQueries'
import { useRumApps, useRumAppsVitals } from '@/lib/rum/rumQueries'
import { useInfraHosts } from '@/lib/infra/infraQueries'
import { useMonitors } from '@/lib/uptime/uptimeQueries'
import { useTenantsSummary } from '@/lib/tenants/tenantsQueries'
import { useRules, useIncidents as useAlertIncidents } from '@/lib/alertsQueries'
import { tenant as activeTenant } from '@/lib/core/context'
import { formatCompact, formatDuration } from '@/lib/core/format'
import { formatPct } from '@/lib/infra/hostStats'
import type { AlertIncident, AlertRule, InfraHost, RumVitalRow, TenantSummary } from '@/lib/core/api'
import {
  applyIncidents,
  attentionList,
  boardWorlds,
  compactAge,
  hostSubject,
  monitorSubject,
  rollup,
  rumAppSubject,
  serviceSubjects,
  siteSubject,
  vanishedHostSubject,
  vitalName,
  vitalSamples,
  vitalText,
  type ServiceRedRow,
  type Subject,
  type WorldKey,
  type WorldRollup,
} from '@/lib/home/attention'
import {
  prevWatchEndNs,
  prevWatchStartNs,
  watchEndNs,
  watchNowMs,
  watchStartNs,
} from '@/lib/home/watchRange'

// --- vital signs ----------------------------------------------------------------------------------

export interface Vital {
  label: string
  value: string
  /** Absent when there is nothing to compare against (monitors carry no window). */
  delta?: string
  tone: 'bad' | 'warn' | 'flat'
}

// A move smaller than this is noise on a 15-minute window; past this it is worth a colour.
const DELTA_FLAT = 0.05
const DELTA_BAD = 0.25

function toneFor(change: number, worseWhen: 'up' | 'down'): 'bad' | 'warn' | 'flat' {
  const magnitude = Math.abs(change)
  if (magnitude < DELTA_FLAT) return 'flat'
  const worse = worseWhen === 'up' ? change > 0 : change < 0
  if (!worse) return 'flat'
  return magnitude >= DELTA_BAD ? 'bad' : 'warn'
}

const arrow = (change: number): string => (Math.abs(change) < DELTA_FLAT ? '→' : change > 0 ? '↑' : '↓')

/** A tile whose delta is a relative change: "↓ 61%". */
function ratioVital(label: string, value: string, cur: number, prev: number, worseWhen: 'up' | 'down'): Vital {
  if (!prev) return { label, value, tone: 'flat' }
  const change = (cur - prev) / prev
  return { label, value, delta: `${arrow(change)} ${Math.abs(Math.round(change * 100))}%`, tone: toneFor(change, worseWhen) }
}

/** A tile whose delta is in percentage points: "↑ 1.2pp". Error rates move in pp, not in percent. */
function pointVital(label: string, value: string, curFrac: number, prevFrac: number, worseWhen: 'up' | 'down'): Vital {
  const diff = (curFrac - prevFrac) * 100
  const change = Math.abs(diff) < 0.05 ? 0 : diff / 100
  return {
    label,
    value,
    delta: `${arrow(change)} ${Math.abs(diff).toFixed(1)}pp`,
    tone: toneFor(change, worseWhen),
  }
}

/** A tile whose delta is an absolute count: "↓ 1". */
function countVital(label: string, value: string, cur: number, prev: number, worseWhen: 'up' | 'down'): Vital {
  const diff = cur - prev
  const change = diff === 0 ? 0 : diff / Math.max(1, Math.abs(prev))
  return { label, value, delta: `${diff > 0 ? '↑' : diff < 0 ? '↓' : '→'} ${Math.abs(diff)}`, tone: toneFor(change, worseWhen) }
}

const plain = (label: string, value: string): Vital => ({ label, value, tone: 'flat' })

function rateText(rate: number): string {
  return (rate >= 100 ? formatCompact(Math.round(rate)) : rate.toFixed(1)) + '/s'
}

// --- fleet aggregates ------------------------------------------------------------------------------

interface RedTotals {
  rate: number
  count: number
  errorRate: number
  worstP99: number
}

function redTotals(rows: ServiceRedRow[]): RedTotals {
  let rate = 0
  let count = 0
  let weightedErrors = 0
  let worstP99 = 0
  for (const r of rows) {
    rate += r.rate ?? 0
    count += r.count ?? 0
    weightedErrors += (r.rate ?? 0) * (r.error_rate ?? 0)
    worstP99 = Math.max(worstP99, Number(r.p99 ?? 0))
  }
  return { rate, count, errorRate: rate ? weightedErrors / rate : 0, worstP99 }
}

function mean(values: (number | null | undefined)[]): number | null {
  const nums = values.filter((v): v is number => v != null && Number.isFinite(v))
  if (!nums.length) return null
  return nums.reduce((a, b) => a + b, 0) / nums.length
}

/** Worst p75 across every app for one Web Vital, with the metric name so the tile can label it. */
function worstVital(vitalsByApp: RumVitalRow[][], metric: string): number | null {
  let worst: number | null = null
  for (const vitals of vitalsByApp) {
    const row = vitals.find((v) => v.metric === metric)
    if (row?.p75 == null) continue
    worst = worst == null ? row.p75 : Math.max(worst, row.p75)
  }
  return worst
}

// --- the board ---------------------------------------------------------------------------------

export interface AttentionBoard {
  subjects: ComputedRef<Subject[]>
  rollups: ComputedRef<WorldRollup[]>
  attention: (world: ComputedRef<WorldKey | null>) => ComputedRef<Subject[]>
  unattributed: ComputedRef<{ incident: AlertIncident; rule?: AlertRule }[]>
  vitals: (world: ComputedRef<WorldKey | null>) => ComputedRef<Vital[]>
  totalWatched: ComputedRef<number>
  /** Whether a row should say WHERE it lives. True only when the board actually spans more than
   *  one place: Central, with no tenant lens narrowing it to a single Site. */
  showSites: ComputedRef<boolean>
  loading: ComputedRef<boolean>
}

export function useAttentionBoard(): AttentionBoard {
  // Backend — RED grouped by service. Rows arrive split per Site (`tenant`), which is what makes
  // cross-Site merging + Spread free here and unavailable for RUM apps (see the ADR).
  const services = useServicesList(() => '', watchStartNs, watchEndNs)
  const servicesPrev = useServicesList(() => '', prevWatchStartNs, prevWatchEndNs)

  // Frontend — the registry, then one vitals query per app over each range.
  const rumAppsQuery = useRumApps()
  const appNames = computed<string[]>(() => (rumAppsQuery.data.value?.apps ?? []).map((a) => a.name))
  const vitalsQueries = useRumAppsVitals(appNames, watchStartNs, watchEndNs)
  const vitalsPrevQueries = useRumAppsVitals(appNames, prevWatchStartNs, prevWatchEndNs)

  // Infra / Ops / Sites.
  const hosts = useInfraHosts(watchStartNs, watchEndNs)
  const hostsPrev = useInfraHosts(prevWatchStartNs, prevWatchEndNs)
  const monitorsQuery = useMonitors()
  const tenantsQuery = useTenantsSummary()

  // Alerts — the overlay that turns a threshold judgment into "somebody asked to be told".
  const rulesQuery = useRules()
  const incidentsQuery = useAlertIncidents(() => ({ status: 'triggered' as const, limit: 200 }))

  const rows = computed<ServiceRedRow[]>(() => services.data.value ?? [])
  const prevRows = computed<ServiceRedRow[]>(() => servicesPrev.data.value ?? [])
  const vitalsByApp = computed<RumVitalRow[][]>(() => vitalsQueries.value.map((q) => q.data?.vitals ?? []))
  const prevVitalsByApp = computed<RumVitalRow[][]>(() => vitalsPrevQueries.value.map((q) => q.data?.vitals ?? []))
  const hostRows = computed<InfraHost[]>(() => hosts.data.value?.hosts ?? [])
  const prevHostRows = computed<InfraHost[]>(() => hostsPrev.data.value?.hosts ?? [])
  const monitors = computed(() => monitorsQuery.data.value ?? [])
  const allSites = computed<TenantSummary[]>(() => tenantsQuery.data.value ?? [])
  // A tenant filter narrows the Sites world to the one Site being inspected, exactly like every
  // other view narrows to it.
  const sites = computed<TenantSummary[]>(() =>
    activeTenant.value ? allSites.value.filter((t) => t.name === activeTenant.value) : allSites.value,
  )
  const hasSites = computed(() => allSites.value.length > 0)
  // A tenant filter turns the board into a read-only lens over ONE Site's mirrored data. Services,
  // RUM apps and hosts all carry the tenant dimension and narrow with it; uptime monitors and the
  // alert engine do not — they are Central's own, local verticals. Under a lens the Ops world is
  // therefore dropped rather than mislabelled as the Site's.
  const lensed = computed(() => !!activeTenant.value)

  const worlds = computed(() => boardWorlds({ sites: hasSites.value, ops: !lensed.value }))

  const built = computed(() => {
    const now = watchNowMs.value
    const subjects: Subject[] = [
      ...serviceSubjects(rows.value, prevRows.value),
      ...appNames.value.map((app, i) => rumAppSubject(app, vitalsByApp.value[i], prevVitalsByApp.value[i])),
      ...hostRows.value.map((h) => hostSubject(h, now)),
      ...(lensed.value ? [] : monitors.value.map((m) => monitorSubject(m))),
    ]
    // A host that reported in the previous range and is gone from this one stopped reporting.
    const present = new Set(hostRows.value.map((h) => h.host))
    for (const h of prevHostRows.value) {
      if (!present.has(h.host)) subjects.push(vanishedHostSubject(h.host))
    }
    if (hasSites.value) subjects.push(...sites.value.map((t) => siteSubject(t, now)))

    const monitorNameById: Record<string, string> = {}
    for (const m of monitors.value) monitorNameById[m.id] = m.name

    return applyIncidents(subjects, incidentsQuery.data.value ?? [], rulesQuery.data.value ?? [], monitorNameById)
  })

  const subjects = computed(() => built.value.subjects)
  // Genuinely fleet-level incidents always show. An incident that named a real place missing from
  // the board shows too — unless a tenant lens is what made it missing, in which case it belongs to
  // another Site and saying otherwise here would be noise.
  const unattributed = computed(() =>
    lensed.value ? built.value.unattributed : [...built.value.unattributed, ...built.value.orphaned],
  )
  const rollups = computed(() => rollup(subjects.value, worlds.value))
  const totalWatched = computed(() => rollups.value.reduce((n, r) => n + r.total, 0))

  // Under a lens every row on the board belongs to the one Site named in the breadcrumb, so a
  // per-row Site badge is at best redundant and at worst wrong: `RedRow.tenant` is only stamped on
  // rows that carry the attribute, so an unstamped row would be labelled "central" while sitting
  // under a "divtik" filter. The board says where it is once, in the crumb.
  const showSites = computed(() => hasSites.value && !lensed.value)

  const attention = (world: ComputedRef<WorldKey | null>) =>
    computed(() => attentionList(subjects.value, world.value))

  const vitals = (world: ComputedRef<WorldKey | null>) =>
    computed<Vital[]>(() => {
      const cur = redTotals(rows.value)
      const prev = redTotals(prevRows.value)
      const pageviews = vitalsByApp.value.reduce((n, v) => n + vitalSamples(v), 0)
      const prevPageviews = prevVitalsByApp.value.reduce((n, v) => n + vitalSamples(v), 0)

      switch (world.value) {
        case 'backend':
          return [
            ratioVital('Traffic', rateText(cur.rate), cur.rate, prev.rate, 'down'),
            pointVital('Error rate', formatPct(cur.errorRate), cur.errorRate, prev.errorRate, 'up'),
            ratioVital('p99 latency', formatDuration(cur.worstP99), cur.worstP99, prev.worstP99, 'up'),
            ratioVital('Spans', formatCompact(cur.count), cur.count, prev.count, 'down'),
          ]
        case 'frontend': {
          const lcp = worstVital(vitalsByApp.value, 'web_vitals.lcp')
          const prevLcp = worstVital(prevVitalsByApp.value, 'web_vitals.lcp')
          const inp = worstVital(vitalsByApp.value, 'web_vitals.inp')
          const reporting = vitalsByApp.value.filter((v) => vitalSamples(v) > 0).length
          return [
            ratioVital('Vital samples', formatCompact(pageviews), pageviews, prevPageviews, 'down'),
            ratioVital(
              `${vitalName('web_vitals.lcp')} p75`,
              vitalText('web_vitals.lcp', lcp),
              lcp ?? 0,
              prevLcp ?? 0,
              'up',
            ),
            plain(`${vitalName('web_vitals.inp')} p75`, vitalText('web_vitals.inp', inp)),
            plain('Apps reporting', `${reporting}/${appNames.value.length}`),
          ]
        }
        case 'infra': {
          const cpu = mean(hostRows.value.map((h) => h.cpuUtil))
          const prevCpu = mean(prevHostRows.value.map((h) => h.cpuUtil))
          const memory = mean(hostRows.value.map((h) => h.memUtil))
          const prevMemory = mean(prevHostRows.value.map((h) => h.memUtil))
          const fullestDisk = hostRows.value.reduce<number | null>(
            (m, h) => (h.diskUtil == null ? m : m == null ? h.diskUtil : Math.max(m, h.diskUtil)),
            null,
          )
          const rosterSize = new Set([...hostRows.value, ...prevHostRows.value].map((h) => h.host)).size
          return [
            pointVital('Mean CPU', formatPct(cpu), cpu ?? 0, prevCpu ?? 0, 'up'),
            pointVital('Mean memory', formatPct(memory), memory ?? 0, prevMemory ?? 0, 'up'),
            countVital('Hosts reporting', `${hostRows.value.length}/${rosterSize}`, hostRows.value.length, rosterSize, 'down'),
            plain('Fullest disk', formatPct(fullestDisk)),
          ]
        }
        case 'ops': {
          const enabled = monitors.value.filter((m) => m.enabled !== false)
          const up = enabled.filter((m) => m.last_state === 'up').length
          const down = enabled.filter((m) => m.last_state === 'down').length
          const latency = mean(enabled.map((m) => m.last_latency_ms as number | undefined))
          return [
            plain('Monitors up', `${up}/${enabled.length}`),
            plain('Failing now', String(down)),
            plain('Mean latency', latency == null ? '—' : `${Math.round(latency)}ms`),
            plain('Paused', String(monitors.value.length - enabled.length)),
          ]
        }
        case 'sites': {
          const reporting = sites.value.filter((t) => t.status === 'up').length
          const ingest = sites.value.reduce((n, t) => n + (t.ingest_rows_per_sec ?? 0), 0)
          const open = sites.value.reduce((n, t) => n + (t.open_incidents ?? 0), 0)
          const oldest = sites.value.reduce<number | null>(
            (m, t) => (!t.last_seen_ms ? m : m == null ? t.last_seen_ms : Math.min(m, t.last_seen_ms)),
            null,
          )
          return [
            countVital('Sites reporting', `${reporting}/${sites.value.length}`, reporting, sites.value.length, 'down'),
            plain('Rows in', rateText(ingest)),
            plain('Open incidents there', String(open)),
            plain('Oldest push', oldest == null ? '—' : compactAge(watchNowMs.value - oldest) + ' ago'),
          ]
        }
        default:
          return [
            ratioVital('Traffic', rateText(cur.rate), cur.rate, prev.rate, 'down'),
            pointVital('Error rate', formatPct(cur.errorRate), cur.errorRate, prev.errorRate, 'up'),
            ratioVital('p99 latency', formatDuration(cur.worstP99), cur.worstP99, prev.worstP99, 'up'),
            hasSites.value
              ? countVital(
                  'Sites reporting',
                  `${sites.value.filter((t) => t.status === 'up').length}/${sites.value.length}`,
                  sites.value.filter((t) => t.status === 'up').length,
                  sites.value.length,
                  'down',
                )
              : ratioVital('Vital samples', formatCompact(pageviews), pageviews, prevPageviews, 'down'),
          ]
      }
    })

  const loading = computed(
    () =>
      services.isLoading.value ||
      hosts.isLoading.value ||
      monitorsQuery.isLoading.value ||
      rumAppsQuery.isLoading.value,
  )

  return { subjects, rollups, attention, unattributed, vitals, totalWatched, showSites, loading }
}

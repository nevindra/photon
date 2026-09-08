// The /infra fleet as ONE list. Pure.
//
// Two sources describe a host: the metrics the agent's sampler reports (`InfraHost`, CPU / memory
// / disk / GPU) and the managed-agent registry (`ManagedHost`, status / site / Docker facts). A
// page built from either alone lies — metrics hide the host enrolled a minute ago, the registry
// hides a box that only ships metrics — so the fleet is their union, one row per host name, ranked
// by how much attention it needs. The counts on the page are the counts in the list.
import { formatPct, hostStatus, utilAccent } from './hostStats'
import type { InfraHost, ManagedHost } from '@/lib/core/api'

/** How much a row needs you, worst first. `silent` = metrics stopped and no agent says why. */
export type FleetLevel = 'error' | 'offline' | 'warning' | 'pending' | 'ok'
export const LEVEL_RANK: Record<FleetLevel, number> = { error: 0, offline: 1, warning: 2, pending: 3, ok: 4 }
export const LEVEL_META: Record<FleetLevel, { label: string; tone: 'error' | 'warning' | 'neutral' | 'success' }> = {
  error: { label: 'critical', tone: 'error' },
  offline: { label: 'offline', tone: 'error' },
  warning: { label: 'warning', tone: 'warning' },
  pending: { label: 'waiting for first sync', tone: 'neutral' },
  ok: { label: 'ok', tone: 'success' },
}

export interface Verdict {
  /** The one resource that decides the level, as a short pill: "disk 90%". */
  label: string
  accent: 'error' | 'warning'
  /** The number behind the number, when there is one: "worst of 3 mounts · average 61%". */
  detail: string | null
}

const RESOURCE = { cpu: 'CPU', mem: 'memory', disk: 'disk', gpu: 'GPU' } as const

/** The single worst-degraded resource of a host, in words — or null when nothing is hot. */
export function hostVerdict(h: InfraHost): Verdict | null {
  const cands: { key: keyof typeof RESOURCE; value: number; mean: number | null; groups: number }[] = []
  if (h.cpuUtil != null) cands.push({ key: 'cpu', value: h.cpuUtil, mean: null, groups: 1 })
  if (h.memUtil != null) cands.push({ key: 'mem', value: h.memUtil, mean: null, groups: 1 })
  if (h.diskUtil != null) cands.push({ key: 'disk', value: h.diskUtil, mean: h.diskUtilAvg, groups: h.diskGroups })
  if (h.gpuUtil != null) cands.push({ key: 'gpu', value: h.gpuUtil, mean: h.gpuUtilAvg, groups: h.gpuGroups })
  let best: (typeof cands)[number] | null = null
  let bestAccent: 'error' | 'warning' | undefined
  for (const c of cands) {
    const accent = utilAccent(c.value)
    if (!accent) continue
    const rank = accent === 'error' ? 1 : 0
    const bestRank = bestAccent === 'error' ? 1 : bestAccent ? 0 : -1
    if (!best || rank > bestRank || (rank === bestRank && c.value > best.value)) {
      best = c
      bestAccent = accent
    }
  }
  if (!best || !bestAccent) return null
  const unit = best.key === 'disk' ? 'mount' : 'device'
  const detail = best.groups > 1 && best.mean != null ? `worst of ${best.groups} ${unit}s · average ${formatPct(best.mean)}` : null
  return { label: `${RESOURCE[best.key]} ${formatPct(best.value)}`, accent: bestAccent, detail }
}

export interface FleetRow {
  host: string
  level: FleetLevel
  metrics: InfraHost | null
  agent: ManagedHost | null
  verdict: Verdict | null
}

function levelOf(metrics: InfraHost | null, agent: ManagedHost | null): FleetLevel {
  if (agent?.status === 'offline') return 'offline'
  if (agent?.status === 'pending' && !metrics) return 'pending'
  if (!metrics) return 'ok'
  const s = hostStatus([metrics.cpuUtil, metrics.memUtil, metrics.diskUtil, metrics.gpuUtil])
  return s === 'error' ? 'error' : s === 'warning' ? 'warning' : 'ok'
}

/** The union of both sources, one row per host, ranked worst first then by name. */
export function fleetRows(hosts: InfraHost[], agents: ManagedHost[]): FleetRow[] {
  const byName = new Map<string, FleetRow>()
  for (const h of hosts) byName.set(h.host, { host: h.host, level: 'ok', metrics: h, agent: null, verdict: null })
  for (const a of agents) {
    const row = byName.get(a.host)
    if (row) row.agent = a
    else byName.set(a.host, { host: a.host, level: 'ok', metrics: null, agent: a, verdict: null })
  }
  const rows = Array.from(byName.values())
  for (const r of rows) {
    r.level = levelOf(r.metrics, r.agent)
    r.verdict = r.metrics ? hostVerdict(r.metrics) : null
  }
  return rows.sort((a, b) => LEVEL_RANK[a.level] - LEVEL_RANK[b.level] || a.host.localeCompare(b.host))
}

export interface FleetSummary {
  total: number
  counts: Record<FleetLevel, number>
  /** Mean of the hosts that report CPU; null when none does. */
  avgCpu: number | null
  gpuHosts: number
  containers: { running: number; total: number } | null
}

export function fleetSummary(rows: FleetRow[]): FleetSummary {
  const counts: Record<FleetLevel, number> = { error: 0, offline: 0, warning: 0, pending: 0, ok: 0 }
  const cpus: number[] = []
  let gpuHosts = 0
  let running = 0
  let total = 0
  let anyDocker = false
  for (const r of rows) {
    counts[r.level]++
    if (r.metrics?.cpuUtil != null) cpus.push(r.metrics.cpuUtil)
    if (r.metrics?.hasGpu) gpuHosts++
    if (r.agent?.docker) {
      anyDocker = true
      running += r.agent.docker.containers_running
      total += r.agent.docker.containers_total
    }
  }
  return {
    total: rows.length,
    counts,
    avgCpu: cpus.length ? cpus.reduce((a, b) => a + b, 0) / cpus.length : null,
    gpuHosts,
    containers: anyDocker ? { running, total } : null,
  }
}

/** The fleet filter a `?status=` param names; anything else means "all". */
export const FLEET_FILTERS: FleetLevel[] = ['error', 'offline', 'warning', 'pending', 'ok']
export function isFleetLevel(v: string): v is FleetLevel {
  return (FLEET_FILTERS as string[]).includes(v)
}

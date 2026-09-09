// Display logic for a host's Docker inventory. Pure.
//
// The one decision that matters here is grouping: containers group by their compose project
// (`com.docker.compose.project`), and a stack Photon deployed and one somebody `docker compose
// up`'d by hand look the same — the managed one just carries a chip. Standalone containers form
// a last group so nothing hides.
import type { Container, ContainerHealth, ContainerPort, ContainerState, DockerEvent, HostInventory } from '@/lib/core/api'

export type Tone = 'success' | 'error' | 'warning' | 'neutral' | 'info'

export interface StateMeta {
  label: string
  tone: Tone
}

export const STATE_META: Record<ContainerState, StateMeta> = {
  running: { label: 'Running', tone: 'success' },
  paused: { label: 'Paused', tone: 'warning' },
  restarting: { label: 'Restarting', tone: 'warning' },
  created: { label: 'Created', tone: 'neutral' },
  exited: { label: 'Exited', tone: 'neutral' },
  dead: { label: 'Dead', tone: 'error' },
  removing: { label: 'Removing', tone: 'neutral' },
}

export const HEALTH_META: Record<ContainerHealth, StateMeta | null> = {
  healthy: { label: 'Healthy', tone: 'success' },
  unhealthy: { label: 'Unhealthy', tone: 'error' },
  starting: { label: 'Starting', tone: 'info' },
  none: null,
}

const TONE_RANK: Record<Tone, number> = { error: 0, warning: 1, info: 2, neutral: 3, success: 4 }

/** The one colour a container row carries: the worse of its state and its health check. */
export function containerTone(c: Pick<Container, 'state' | 'health' | 'exit_code' | 'restart_count'>): Tone {
  let tone = STATE_META[c.state].tone
  if (c.state === 'exited' && c.exit_code != null && c.exit_code !== 0) tone = 'error'
  if (c.state === 'restarting') tone = 'error'
  const h = HEALTH_META[c.health]
  if (h && TONE_RANK[h.tone] < TONE_RANK[tone]) tone = h.tone
  return tone
}

export function worstTone(tones: Tone[]): Tone {
  return tones.reduce<Tone>((w, t) => (TONE_RANK[t] < TONE_RANK[w] ? t : w), 'success')
}

export function isRunning(c: Pick<Container, 'state'>): boolean {
  return c.state === 'running' || c.state === 'paused' || c.state === 'restarting'
}

export interface ProjectGroup {
  /** null = standalone containers (no compose label). */
  project: string | null
  managedStackId: string | null
  containers: Container[]
  running: number
  total: number
  tone: Tone
}

export function groupByProject(containers: Container[]): ProjectGroup[] {
  const map = new Map<string, Container[]>()
  for (const c of containers) {
    const k = c.compose_project ?? ''
    map.set(k, [...(map.get(k) ?? []), c])
  }
  const groups: ProjectGroup[] = Array.from(map.entries()).map(([k, list]) => {
    const sorted = [...list].sort((a, b) => (a.compose_service ?? a.name).localeCompare(b.compose_service ?? b.name))
    return {
      project: k || null,
      managedStackId: sorted.find((c) => c.managed_stack_id)?.managed_stack_id ?? null,
      containers: sorted,
      running: sorted.filter((c) => c.state === 'running').length,
      total: sorted.length,
      tone: worstTone(sorted.map(containerTone)),
    }
  })
  // Named projects alphabetically, the standalone bucket last.
  return groups.sort((a, b) => (a.project == null ? 1 : b.project == null ? -1 : a.project.localeCompare(b.project)))
}

/** "8080→80/tcp", "127.0.0.1:5432→5432/tcp", "9001/tcp" (unpublished). */
export function formatPort(p: ContainerPort): string {
  const proto = p.protocol === 'tcp' ? '' : '/' + p.protocol
  if (p.host_port == null) return `${p.container_port}${proto || '/tcp'}`
  const ip = p.host_ip && p.host_ip !== '0.0.0.0' && p.host_ip !== '::' ? p.host_ip + ':' : ''
  return `${ip}${p.host_port}→${p.container_port}${proto}`
}

/** Case-insensitive match on name, image, compose project/service, or a label value. */
/**
 * A container's ports with the IPv4 / IPv6 twins collapsed: Docker publishes `0.0.0.0:4030` and
 * `[::]:4030` as two bindings, and a row that shows the same chip twice reads as two ports.
 * Bindings that format the same are one; a port bound to a specific address stays its own.
 */
export function uniquePorts(ports: ContainerPort[]): ContainerPort[] {
  const seen = new Set<string>()
  const out: ContainerPort[] = []
  for (const p of ports) {
    const k = formatPort(p)
    if (seen.has(k)) continue
    seen.add(k)
    out.push(p)
  }
  return out
}

export function matchesFilter(c: Container, q: string): boolean {
  const needle = q.trim().toLowerCase()
  if (!needle) return true
  const hay = [c.name, c.image, c.compose_project ?? '', c.compose_service ?? '', c.state, c.id, ...Object.values(c.labels)].join('\n').toLowerCase()
  return hay.includes(needle)
}

export interface InventoryKpis {
  running: number
  total: number
  stopped: number
  unhealthy: number
  restarts: number
  projects: number
}

export function inventoryKpis(inv: Pick<HostInventory, 'containers'>): InventoryKpis {
  const cs = inv.containers
  return {
    running: cs.filter((c) => c.state === 'running').length,
    total: cs.length,
    stopped: cs.filter((c) => c.state === 'exited' || c.state === 'dead' || c.state === 'created').length,
    unhealthy: cs.filter((c) => c.health === 'unhealthy' || c.state === 'restarting' || c.state === 'dead').length,
    restarts: cs.reduce((n, c) => n + c.restart_count, 0),
    projects: new Set(cs.map((c) => c.compose_project).filter(Boolean)).size,
  }
}

/** The short image reference: strip the registry, keep `name:tag`. */
export function shortImage(image: string): string {
  const parts = image.split('/')
  return parts.length > 1 && (parts[0].includes('.') || parts[0].includes(':')) ? parts.slice(1).join('/') : image
}

/** `sha256:abcdef…` → `abcdef123456`. */
export function shortDigest(id: string): string {
  return id.replace(/^sha256:/, '').slice(0, 12)
}

/** A Docker event's tone: died / oom / unhealthy are the news; starts are calm. */
export function eventTone(action: string, attributes: Record<string, string> = {}): Tone {
  if (action === 'oom' || action.includes('unhealthy') || action === 'kill') return 'error'
  if (action === 'die') return attributes.exitCode && attributes.exitCode !== '0' ? 'error' : 'neutral'
  if (action === 'start' || action.includes('healthy')) return 'success'
  if (action === 'stop' || action === 'pause' || action === 'restart') return 'warning'
  return 'neutral'
}

export function eventLabel(action: string, attributes: Record<string, string> = {}): string {
  if (action === 'die') return attributes.exitCode && attributes.exitCode !== '0' ? `died (exit ${attributes.exitCode})` : 'exited'
  if (action === 'oom') return 'OOM-killed'
  if (action.startsWith('health_status:')) return action.replace('health_status:', 'health').trim()
  // Docker folds an exec's whole command line into the action itself ("exec_start: /bin/sh -c
  // pg_isready -U postgres -d vdashboard"). The pill keeps the verb; `eventDetail` carries the rest,
  // so a long health-check command cannot swell a fixed-width pill into a four-line blob.
  const colon = action.indexOf(': ')
  return colon > 0 ? action.slice(0, colon) : action
}

/** The part of an event the pill does not show: an exec's command line, or null. */
export function eventDetail(action: string): string | null {
  if (action.startsWith('health_status:')) return null
  const colon = action.indexOf(': ')
  if (colon < 0) return null
  const rest = action.slice(colon + 2).trim()
  return rest || null
}

/**
 * Where a published port opens in a browser, or null when it cannot: only TCP ports published
 * on the host qualify. A port bound to a specific address opens there; one bound to every
 * address (0.0.0.0 / ::) or to loopback opens on the host's name, since loopback is the host's
 * own, not the operator's. 443 is https, everything else http — a guess the browser corrects.
 */
export function portUrl(p: ContainerPort, host: string): string | null {
  if (p.protocol !== 'tcp' || p.host_port == null) return null
  const ip = p.host_ip
  const hostname = ip && ip !== '0.0.0.0' && ip !== '::' && ip !== '127.0.0.1' && ip !== '::1' ? (ip.includes(':') ? `[${ip}]` : ip) : host
  const scheme = p.host_port === 443 ? 'https' : 'http'
  return `${scheme}://${hostname}:${p.host_port}`
}

export interface EventFold {
  kind: 'fold'
  actor: string
  command: string | null
  image: string | null
  count: number
  /** Newest event in the fold. */
  ts_ms: number
  /** Typical gap between runs, or null with fewer than two. */
  every_ms: number | null
  events: DockerEvent[]
}
export type FeedItem = { kind: 'event'; event: DockerEvent } | EventFold

/**
 * The feed with a health check's chatter folded. Docker reports every exec as three events
 * (exec_create, exec_start, exec_die), and a health check execs every ten seconds — so on a
 * quiet host the feed is nothing but one command repeating. All of one actor's exec events
 * become ONE row carrying the command, how many, and how often; everything else stays a row
 * of its own in its place. Newest first, a fold sitting where its newest event was.
 */
export function foldEvents(events: DockerEvent[]): FeedItem[] {
  const folds = new Map<string, EventFold>()
  const out: FeedItem[] = []
  for (const e of events) {
    if (!e.action.startsWith('exec_')) {
      out.push({ kind: 'event', event: e })
      continue
    }
    let f = folds.get(e.actor)
    if (!f) {
      f = { kind: 'fold', actor: e.actor, command: null, image: e.attributes.image ?? null, count: 0, ts_ms: e.ts_ms, every_ms: null, events: [] }
      folds.set(e.actor, f)
      out.push(f)
    }
    f.count++
    f.events.push(e)
    f.ts_ms = Math.max(f.ts_ms, e.ts_ms)
    if (!f.command) f.command = eventDetail(e.action)
    if (!f.image && e.attributes.image) f.image = e.attributes.image
  }
  for (const f of folds.values()) {
    const starts = f.events.filter((e) => e.action.startsWith('exec_start')).map((e) => e.ts_ms).sort((a, b) => a - b)
    if (starts.length >= 2) {
      const gaps = starts.slice(1).map((t, i) => t - starts[i]).sort((a, b) => a - b)
      f.every_ms = gaps[Math.floor(gaps.length / 2)]
    }
  }
  return out.sort((a, b) => (b.kind === 'fold' ? b.ts_ms : b.event.ts_ms) - (a.kind === 'fold' ? a.ts_ms : a.event.ts_ms))
}

/** "every 10s" / "every 2m" / "every 1h" for a fold's cadence. */
export function everyLabel(ms: number): string {
  const s = Math.round(ms / 1000)
  if (s < 60) return `every ${s}s`
  const m = Math.round(s / 60)
  if (m < 60) return `every ${m}m`
  return `every ${Math.round(m / 60)}h`
}

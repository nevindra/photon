// The in-browser mock corpus for the Docker fleet-management surface (managed agents, audit log,
// containers, stacks, templates, rollouts). Split out of `mock.ts` because that file is the
// telemetry corpus and this one is control-plane state with its own mutable fixtures; `api.ts`
// imports both. Every exported function returns the exact `/api/*` JSON shape, so the offline UI
// and the wired UI render the same thing.
//
// Mutations here mutate module state (like `mockAlertRulesData` in mock.ts) so a demo session can
// enroll a host, watch a bootstrap run, and see the audit row it produced.
import type {
  AgentCapability,
  AuditEntry,
  AuditFilter,
  AuditResult,
  BootstrapJob,
  BootstrapHostProgress,
  BootstrapStep,
  BootstrapStepKey,
  DockerFacts,
  EnrollInput,
  EnrollResult,
  ManagedHost,
  ManagedHostsResult,
  MutationResult,
  SshBootstrapInput,
  TokenIssue,
} from '@/lib/core/api'

const MIN = 60_000
const HOUR = 3_600_000
const DAY = 24 * HOUR

// ---------------------------------------------------------------------------
// Managed hosts (agents)
// ---------------------------------------------------------------------------

const LATEST_AGENT = '1.8.0'

function facts(over: Partial<DockerFacts> = {}): DockerFacts {
  return {
    version: '27.3.1',
    api_version: '1.47',
    storage_driver: 'overlay2',
    os: 'Ubuntu 22.04.5 LTS',
    arch: 'x86_64',
    containers_running: 6,
    containers_total: 8,
    images: 14,
    disk: {
      images_bytes: 9.4 * 1024 ** 3,
      volumes_bytes: 21.7 * 1024 ** 3,
      build_cache_bytes: 1.2 * 1024 ** 3,
      reclaimable_bytes: 3.1 * 1024 ** 3,
      total_bytes: 200 * 1024 ** 3,
      used_bytes: 128 * 1024 ** 3,
    },
    ...over,
  }
}

let managedHosts: ManagedHost[] = [
  {
    id: 'mh_web1',
    host: 'web-1',
    site: null,
    status: 'online',
    agent_version: LATEST_AGENT,
    latest_agent_version: LATEST_AGENT,
    capabilities: ['read', 'lifecycle', 'apply'],
    poll_secs: 15,
    last_seen_ms: Date.now() - 9_000,
    enrolled_ms: Date.now() - 41 * DAY,
    docker: facts(),
    labels: { role: 'web', zone: 'dc1' },
  },
  {
    id: 'mh_web2',
    host: 'web-2',
    site: null,
    status: 'online',
    agent_version: '1.7.2',
    latest_agent_version: LATEST_AGENT,
    capabilities: ['read', 'lifecycle', 'apply'],
    poll_secs: 15,
    last_seen_ms: Date.now() - 4_000,
    enrolled_ms: Date.now() - 41 * DAY,
    docker: facts({ containers_running: 5, containers_total: 6, images: 11 }),
    labels: { role: 'web', zone: 'dc1' },
  },
  {
    id: 'mh_gpu1',
    host: 'gpu-node-1',
    site: null,
    status: 'online',
    agent_version: LATEST_AGENT,
    latest_agent_version: LATEST_AGENT,
    capabilities: ['read'],
    poll_secs: 30,
    last_seen_ms: Date.now() - 21_000,
    enrolled_ms: Date.now() - 12 * DAY,
    docker: facts({
      containers_running: 3,
      containers_total: 3,
      images: 6,
      disk: {
        images_bytes: 38.2 * 1024 ** 3,
        volumes_bytes: 410 * 1024 ** 3,
        build_cache_bytes: 0,
        reclaimable_bytes: 12.8 * 1024 ** 3,
        total_bytes: 2 * 1024 ** 4,
        used_bytes: 1.4 * 1024 ** 4,
      },
    }),
    labels: { role: 'gpu', zone: 'dc1' },
  },
  {
    id: 'mh_vas1',
    host: 'vas-edge-01',
    site: 'divtik',
    status: 'online',
    agent_version: '1.7.2',
    latest_agent_version: LATEST_AGENT,
    capabilities: ['read', 'lifecycle', 'apply', 'exec'],
    poll_secs: 15,
    last_seen_ms: Date.now() - 12_000,
    enrolled_ms: Date.now() - 88 * DAY,
    docker: facts({
      containers_running: 4,
      containers_total: 5,
      images: 9,
      os: 'Debian GNU/Linux 12',
      disk: {
        images_bytes: 6.1 * 1024 ** 3,
        volumes_bytes: 44.9 * 1024 ** 3,
        build_cache_bytes: 0.3 * 1024 ** 3,
        reclaimable_bytes: 2.2 * 1024 ** 3,
        total_bytes: 100 * 1024 ** 3,
        used_bytes: 91 * 1024 ** 3,
      },
    }),
    labels: { role: 'vas', customer: 'divtik' },
  },
  {
    id: 'mh_vas2',
    host: 'vas-edge-02',
    site: 'divtik',
    status: 'offline',
    agent_version: '1.7.2',
    latest_agent_version: LATEST_AGENT,
    capabilities: ['read', 'lifecycle', 'apply'],
    poll_secs: 15,
    last_seen_ms: Date.now() - 3 * DAY - 2 * HOUR,
    enrolled_ms: Date.now() - 88 * DAY,
    docker: facts({ containers_running: 4, containers_total: 5, images: 9, os: 'Debian GNU/Linux 12' }),
    labels: { role: 'vas', customer: 'divtik' },
  },
  {
    id: 'mh_kiosk',
    host: 'kiosk-gw',
    site: 'cpin',
    status: 'pending',
    agent_version: null,
    latest_agent_version: LATEST_AGENT,
    capabilities: ['read', 'lifecycle'],
    poll_secs: 15,
    last_seen_ms: null,
    enrolled_ms: Date.now() - 35 * MIN,
    docker: null,
    labels: { customer: 'cpin' },
  },
]

function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T
}

export function mockManagedHosts(): ManagedHostsResult {
  return { hosts: clone(managedHosts) }
}

export function mockManagedHost(host: string): ManagedHost | null {
  const h = managedHosts.find((x) => x.host === host)
  return h ? clone(h) : null
}

function mintToken(): string {
  const alphabet = 'abcdefghijklmnopqrstuvwxyz0123456789'
  let s = 'ak_'
  for (let i = 0; i < 28; i++) s += alphabet[Math.floor(Math.random() * alphabet.length)]
  return s
}

export function mockEnrollHost(input: EnrollInput): EnrollResult {
  const name = input.host.trim()
  if (!name) return { ok: false, error: 'host name is required' }
  if (managedHosts.some((h) => h.host === name)) return { ok: false, error: `${name} is already enrolled` }
  const host: ManagedHost = {
    id: 'mh_' + name.replace(/[^a-z0-9]/gi, '').toLowerCase(),
    host: name,
    site: input.site ?? null,
    status: 'pending',
    agent_version: null,
    latest_agent_version: LATEST_AGENT,
    capabilities: [...input.capabilities],
    poll_secs: input.poll_secs ?? 15,
    last_seen_ms: null,
    enrolled_ms: Date.now(),
    docker: null,
    labels: input.labels ?? {},
  }
  managedHosts = [...managedHosts, host]
  pushAudit({ action: 'agent.enroll', target: { host: name }, result: 'ok', detail: `capabilities: ${input.capabilities.join(', ')}` })
  return { ok: true, host: clone(host), token: mintToken() }
}

export function mockUpdateAgent(host: string, patch: { poll_secs?: number; labels?: Record<string, string>; site?: string | null }): MutationResult & { host?: ManagedHost } {
  const idx = managedHosts.findIndex((h) => h.host === host)
  if (idx < 0) return { ok: false, error: 'no such host' }
  if (patch.poll_secs != null && patch.poll_secs < 15) return { ok: false, error: 'poll interval must be at least 15 seconds' }
  const updated: ManagedHost = { ...managedHosts[idx], ...(patch.poll_secs != null ? { poll_secs: patch.poll_secs } : {}), ...(patch.labels ? { labels: patch.labels } : {}), ...(patch.site !== undefined ? { site: patch.site } : {}) }
  managedHosts = managedHosts.map((h, i) => (i === idx ? updated : h))
  pushAudit({ action: 'agent.update', target: { host }, result: 'ok', detail: JSON.stringify(patch) })
  return { ok: true, host: clone(updated) }
}

export function mockRotateAgentToken(host: string): MutationResult & TokenIssue {
  if (!managedHosts.some((h) => h.host === host)) return { ok: false, error: 'no such host' }
  pushAudit({ action: 'agent.rotate_token', target: { host }, result: 'ok' })
  return { ok: true, token: mintToken() }
}

export function mockDeleteAgent(host: string): MutationResult {
  if (!managedHosts.some((h) => h.host === host)) return { ok: false, error: 'no such host' }
  managedHosts = managedHosts.filter((h) => h.host !== host)
  pushAudit({ action: 'agent.remove', target: { host }, result: 'ok' })
  return { ok: true }
}

export function mockRequestAgentUpgrade(host: string): MutationResult {
  const h = managedHosts.find((x) => x.host === host)
  if (!h) return { ok: false, error: 'no such host' }
  pushAudit({ action: 'agent.upgrade', target: { host }, result: 'pending', detail: `→ ${h.latest_agent_version}` })
  return { ok: true }
}

// ---------------------------------------------------------------------------
// SSH bootstrap jobs — a time-driven simulation so the stepper has something to animate.
// Each step takes STEP_MS; an address containing "fail" refuses the SSH connection, an address
// containing "slow" never gets its first sync (the last step stays running, then times out).
// ---------------------------------------------------------------------------

const STEP_MS = 1_100
const STEPS: { key: BootstrapStepKey; label: string }[] = [
  { key: 'connect', label: 'Connect over SSH' },
  { key: 'detect', label: 'Detect OS and architecture' },
  { key: 'push', label: 'Push the agent binary' },
  { key: 'configure', label: 'Write config with a fresh token' },
  { key: 'start', label: 'Install and start the service' },
  { key: 'sync', label: 'Wait for the first sync' },
]

interface JobState {
  id: string
  started_ms: number
  input: SshBootstrapInput
  addresses: string[]
  enrolled: Set<string>
}
const jobs = new Map<string, JobState>()

export function mockStartBootstrap(input: SshBootstrapInput): { ok: boolean; error?: string; job?: BootstrapJob } {
  const addresses = input.addresses.map((a) => a.trim()).filter(Boolean)
  if (!addresses.length) return { ok: false, error: 'at least one address is required' }
  if (!input.username.trim()) return { ok: false, error: 'SSH user is required' }
  if (input.auth.type === 'password' && !input.auth.password) return { ok: false, error: 'password is required' }
  if (input.auth.type === 'key' && !input.auth.private_key.trim()) return { ok: false, error: 'private key is required' }
  const id = 'bj_' + Math.random().toString(36).slice(2, 10)
  jobs.set(id, { id, started_ms: Date.now(), input, addresses, enrolled: new Set() })
  pushAudit({
    action: 'agent.bootstrap',
    target: {},
    result: 'pending',
    detail: `${addresses.length} host${addresses.length === 1 ? '' : 's'} over SSH as ${input.username}${input.remember_credentials ? ' (credentials kept)' : ''}`,
  })
  return { ok: true, job: mockBootstrapJob(id)! }
}

function hostNameFor(address: string, input: SshBootstrapInput): string {
  if (input.addresses.length === 1 && input.host_name?.trim()) return input.host_name.trim()
  return address.replace(/^[^@]+@/, '').replace(/:\d+$/, '').replace(/\./g, '-')
}

export function mockBootstrapJob(id: string): BootstrapJob | null {
  const job = jobs.get(id)
  if (!job) return null
  const elapsed = Date.now() - job.started_ms
  const hosts: BootstrapHostProgress[] = job.addresses.map((address) => {
    const fails = /fail/i.test(address)
    const slow = /slow/i.test(address)
    const done = Math.floor(elapsed / STEP_MS)
    const steps: BootstrapStep[] = STEPS.map((s, i) => {
      let status: BootstrapStep['status'] = 'pending'
      let detail: string | undefined
      if (fails) {
        if (i === 0) {
          status = done >= 1 ? 'failed' : 'running'
          if (status === 'failed') detail = `ssh: connect to host ${address.replace(/^[^@]+@/, '')} port ${job.input.port}: Connection refused`
        } else status = done >= 1 ? 'skipped' : 'pending'
      } else if (slow && i === STEPS.length - 1) {
        status = done >= i ? (elapsed > STEP_MS * (STEPS.length + 8) ? 'failed' : 'running') : 'pending'
        if (status === 'failed') detail = 'No sync after 90s. The service is installed — check the host can reach this Photon.'
        if (status === 'running') detail = 'Installed and started. Waiting for the agent to call home…'
      } else {
        status = done > i ? 'done' : done === i ? 'running' : 'pending'
        if (status === 'done') {
          if (s.key === 'detect') detail = 'linux / x86_64 · Docker 27.3.1'
          if (s.key === 'push') detail = `photon-agent ${LATEST_AGENT} (12.4 MB)`
          if (s.key === 'configure') detail = 'token minted, never shown'
          if (s.key === 'start') detail = 'systemd unit photon-agent.service is active'
          if (s.key === 'sync') detail = 'first sync received'
        }
      }
      return { key: s.key, label: s.label, status, detail }
    })
    const failed = steps.some((s) => s.status === 'failed')
    const finished = steps.every((s) => s.status === 'done')
    const hostName = hostNameFor(address, job.input)
    if (finished && !job.enrolled.has(address)) {
      job.enrolled.add(address)
      if (!managedHosts.some((h) => h.host === hostName)) {
        managedHosts = [
          ...managedHosts,
          {
            id: 'mh_' + hostName.replace(/[^a-z0-9]/gi, '').toLowerCase(),
            host: hostName,
            site: job.input.site ?? null,
            status: 'online',
            agent_version: LATEST_AGENT,
            latest_agent_version: LATEST_AGENT,
            capabilities: [...job.input.capabilities],
            poll_secs: job.input.poll_secs ?? 15,
            last_seen_ms: Date.now(),
            enrolled_ms: Date.now(),
            docker: facts({ containers_running: 0, containers_total: 0, images: 0 }),
            labels: job.input.labels ?? {},
          },
        ]
      }
      pushAudit({ action: 'agent.enroll', target: { host: hostName }, result: 'ok', detail: `bootstrapped over SSH from ${address}` })
    }
    return {
      address,
      host: finished ? hostName : null,
      status: failed ? 'failed' : finished ? 'done' : 'running',
      steps,
      error: failed ? steps.find((s) => s.status === 'failed')?.detail ?? 'failed' : null,
    }
  })
  const status: BootstrapJob['status'] = hosts.some((h) => h.status === 'running')
    ? 'running'
    : hosts.some((h) => h.status === 'failed')
      ? 'failed'
      : 'done'
  return { id, status, started_ms: job.started_ms, credentials_kept: !!job.input.remember_credentials, hosts }
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

let auditSeq = 1000
let auditEntries: AuditEntry[] = []

export type AuditInput = Pick<AuditEntry, 'action' | 'target' | 'result'> & Partial<Pick<AuditEntry, 'actor' | 'role' | 'ts_ms' | 'detail' | 'payload_digest'>>

export function pushAudit(e: AuditInput): AuditEntry {
  const entry: AuditEntry = {
    id: 'au_' + (auditSeq += 1),
    ts_ms: e.ts_ms ?? Date.now(),
    actor: e.actor ?? 'demo',
    role: e.role ?? 'admin',
    action: e.action,
    target: e.target,
    result: e.result,
    detail: e.detail ?? null,
    payload_digest: e.payload_digest ?? null,
  }
  auditEntries = [entry, ...auditEntries]
  return entry
}

// Seed: a plausible fortnight of an ops team's activity, newest first once sorted.
;(function seedAudit() {
  const now = Date.now()
  const rows: Array<Omit<AuditEntry, 'id' | 'detail' | 'payload_digest'> & Partial<Pick<AuditEntry, 'detail' | 'payload_digest'>>> = [
    { ts_ms: now - 6 * MIN, actor: 'rina', role: 'operator', action: 'container.restart', target: { host: 'vas-edge-01', container: 'vas-inference' }, result: 'ok', detail: 'exit 0 in 4.2s', payload_digest: 'sha256:9f1c…a2e0' },
    { ts_ms: now - 22 * MIN, actor: 'rina', role: 'operator', action: 'stack.apply', target: { host: 'web-2', stack: 'photon-demo' }, result: 'ok', detail: 'generation 14 → 15 · pull=missing', payload_digest: 'sha256:44b1…07dd' },
    { ts_ms: now - 35 * MIN, actor: 'demo', role: 'admin', action: 'agent.enroll', target: { host: 'kiosk-gw' }, result: 'ok', detail: 'capabilities: read, lifecycle' },
    { ts_ms: now - 2 * HOUR, actor: 'bayu', role: 'viewer', action: 'container.stop', target: { host: 'web-1', container: 'worker' }, result: 'denied', detail: 'requires the Operator role' },
    { ts_ms: now - 3 * HOUR - 12 * MIN, actor: 'rina', role: 'operator', action: 'stack.apply', target: { host: 'vas-edge-02', stack: 'nodeflux-vas' }, result: 'pending', detail: 'host offline — will apply on next sync', payload_digest: 'sha256:c0de…1a9b' },
    { ts_ms: now - 5 * HOUR, actor: 'demo', role: 'admin', action: 'template.publish', target: { template: 'nodeflux-vas', version: '2.4.1' }, result: 'ok', detail: 'compose 3.1 KB · 9 variables' },
    { ts_ms: now - 7 * HOUR, actor: 'rina', role: 'operator', action: 'container.pull', target: { host: 'web-1', container: 'api' }, result: 'failed', detail: 'manifest unknown: ghcr.io/nodeflux/api:2.9.0-rc1' },
    { ts_ms: now - 1 * DAY - 40 * MIN, actor: 'rina', role: 'operator', action: 'rollout.promote', target: { rollout: 'ro_vas_241' }, result: 'ok', detail: 'stage 1 → 2 (canary healthy 18m)' },
    { ts_ms: now - 3 * DAY - 4 * HOUR, actor: 'rina', role: 'operator', action: 'rollout.pause', target: { rollout: 'ro_demo_284' }, result: 'ok', detail: 'stage 1 done — waiting at the gate' },
    { ts_ms: now - 3 * DAY - 4 * HOUR - 6 * MIN, actor: 'rina', role: 'operator', action: 'stack.apply', target: { host: 'web-1', stack: 'photon-demo', rollout: 'ro_demo_284' }, result: 'ok', detail: '2.8.3 → 2.8.4 (rollout stage 1)' },
    { ts_ms: now - 3 * DAY - 5 * HOUR, actor: 'rina', role: 'operator', action: 'rollout.create', target: { rollout: 'ro_demo_284', template: 'photon-demo', version: '2.8.4' }, result: 'ok', detail: '2 targets · 2 stages · threshold 1' },
    { ts_ms: now - 1 * DAY - 3 * HOUR, actor: 'rina', role: 'operator', action: 'rollout.create', target: { rollout: 'ro_vas_241', template: 'nodeflux-vas', version: '2.4.1' }, result: 'ok', detail: 'site divtik · canary 1 · threshold 1' },
    { ts_ms: now - 2 * DAY, actor: 'demo', role: 'admin', action: 'agent.rotate_token', target: { host: 'vas-edge-01' }, result: 'ok' },
    { ts_ms: now - 2 * DAY - 5 * HOUR, actor: 'rina', role: 'operator', action: 'stack.reconcile', target: { host: 'vas-edge-01', stack: 'nodeflux-vas' }, result: 'ok', detail: 'reverted 1 drifted service (vas-inference: image)' },
    { ts_ms: now - 3 * DAY, actor: 'demo', role: 'admin', action: 'user.role', target: { user: 'rina' }, result: 'ok', detail: 'viewer → operator' },
    { ts_ms: now - 3 * DAY - 6 * HOUR, actor: 'rina', role: 'operator', action: 'container.remove', target: { host: 'web-2', container: 'old-migrator' }, result: 'ok', detail: 'volumes kept' },
    { ts_ms: now - 4 * DAY, actor: 'rina', role: 'operator', action: 'prune', target: { host: 'web-2' }, result: 'ok', detail: 'freed 2.8 GB (images 2.1 GB, build cache 0.7 GB)' },
    { ts_ms: now - 5 * DAY, actor: 'demo', role: 'admin', action: 'stack.create', target: { host: 'web-2', stack: 'photon-demo' }, result: 'ok', detail: 'arbitrary compose · 2 acknowledged warnings', payload_digest: 'sha256:7e7e…b3c4' },
    { ts_ms: now - 6 * DAY, actor: 'demo', role: 'admin', action: 'template.import', target: { registry: 'portainer-community' }, result: 'ok', detail: '212 templates (portainer v3)' },
    { ts_ms: now - 8 * DAY, actor: 'rina', role: 'operator', action: 'rollout.rollback', target: { rollout: 'ro_vas_240' }, result: 'ok', detail: '2.4.0 → 2.3.7 on 3 hosts' },
    { ts_ms: now - 8 * DAY - 1 * HOUR, actor: 'rina', role: 'operator', action: 'rollout.pause', target: { rollout: 'ro_vas_240' }, result: 'ok', detail: 'threshold crossed: 2 failed in stage 2' },
    { ts_ms: now - 12 * DAY, actor: 'demo', role: 'admin', action: 'agent.enroll', target: { host: 'gpu-node-1' }, result: 'ok', detail: 'capabilities: read' },
    { ts_ms: now - 13 * DAY, actor: 'bayu', role: 'viewer', action: 'stack.apply', target: { host: 'web-1', stack: 'photon-demo' }, result: 'denied', detail: 'requires the Operator role' },
  ]
  auditEntries = rows.map((r) => ({ ...r, id: 'au_' + (auditSeq += 1), detail: r.detail ?? null, payload_digest: r.payload_digest ?? null }))
})()

export function mockAudit(filter: AuditFilter = {}): AuditResult {
  let rows = [...auditEntries].sort((a, b) => b.ts_ms - a.ts_ms)
  if (filter.since_ms != null) rows = rows.filter((r) => r.ts_ms >= filter.since_ms!)
  if (filter.actor) rows = rows.filter((r) => r.actor === filter.actor)
  if (filter.action) rows = rows.filter((r) => r.action === filter.action || r.action.startsWith(filter.action + '.'))
  if (filter.host) rows = rows.filter((r) => r.target.host === filter.host)
  if (filter.result) rows = rows.filter((r) => r.result === filter.result)
  if (filter.q) {
    const q = filter.q.toLowerCase()
    rows = rows.filter((r) => JSON.stringify(r).toLowerCase().includes(q))
  }
  const total = rows.length
  if (filter.limit) rows = rows.slice(0, filter.limit)
  return { entries: rows, total }
}

export const mockAgentCapabilities: AgentCapability[] = ['read', 'lifecycle', 'apply', 'exec']

// ---------------------------------------------------------------------------
// Per-host Docker inventory (containers / images / volumes / networks), container logs, stats
// and daemon events. Deterministic fixtures with a little time-driven life: running containers
// get fresh stats and log lines on every poll, so "follow" has something to follow.
// ---------------------------------------------------------------------------
import type {
  Container,
  ContainerLogLine,
  ContainerLogsQuery,
  ContainerLogsResult,
  ContainerStatPoint,
  ContainerStats,
  DockerEvent,
  DockerEventsResult,
  HostInventory,
  ImageInfo,
  NetworkInfo,
  VolumeInfo,
} from '@/lib/core/api'

const GB = 1024 ** 3
const MB = 1024 ** 2

function hash(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  return h >>> 0
}
// Deterministic 0..1 noise for (key, index) — stable across polls, different per container.
function noise(key: string, i: number): number {
  return (hash(key + ':' + i) % 1000) / 1000
}
function shortId(seed: string): string {
  return hash(seed).toString(16).padStart(8, '0') + hash(seed + 'x').toString(16).padStart(4, '0')
}

interface ContainerSpec {
  name: string
  image: string
  project?: string
  service?: string
  managed?: string
  state?: Container['state']
  health?: Container['health']
  exit_code?: number
  restarts?: number
  ports?: string[]
  mounts?: string[]
  cpu?: number
  mem?: number
  mem_limit?: number
  started_ago_ms?: number
  finished_ago_ms?: number
  labels?: Record<string, string>
  env?: string[]
  command?: string
  networks?: string[]
}

function parsePort(p: string): Container['ports'][number] {
  // "8080:80/tcp" | "80/tcp" | "0.0.0.0:443:8443"
  const [spec, proto = 'tcp'] = p.split('/')
  const parts = spec.split(':')
  if (parts.length === 1) return { host_ip: null, host_port: null, container_port: Number(parts[0]), protocol: proto as 'tcp' | 'udp' }
  if (parts.length === 2) return { host_ip: '0.0.0.0', host_port: Number(parts[0]), container_port: Number(parts[1]), protocol: proto as 'tcp' | 'udp' }
  return { host_ip: parts[0], host_port: Number(parts[1]), container_port: Number(parts[2]), protocol: proto as 'tcp' | 'udp' }
}
function parseMount(m: string): Container['mounts'][number] {
  const [src, dst, mode] = m.split(':')
  const type = src.startsWith('/') ? 'bind' : src === 'tmpfs' ? 'tmpfs' : 'volume'
  return { type, source: src, destination: dst, rw: mode !== 'ro' }
}

function statusText(c: Pick<Container, 'state' | 'health' | 'started_ms' | 'finished_ms' | 'exit_code'>, now: number): string {
  const ago = (ms: number | null) => {
    if (ms == null) return ''
    const s = Math.max(1, Math.round((now - ms) / 1000))
    if (s < 60) return `${s} seconds`
    const m = Math.round(s / 60)
    if (m < 60) return `${m} minute${m === 1 ? '' : 's'}`
    const h = Math.round(m / 60)
    if (h < 48) return `${h} hour${h === 1 ? '' : 's'}`
    return `${Math.round(h / 24)} days`
  }
  switch (c.state) {
    case 'running':
      return `Up ${ago(c.started_ms)}${c.health !== 'none' ? ` (${c.health})` : ''}`
    case 'exited':
      return `Exited (${c.exit_code ?? 0}) ${ago(c.finished_ms)} ago`
    case 'paused':
      return `Up ${ago(c.started_ms)} (Paused)`
    case 'restarting':
      return `Restarting (${c.exit_code ?? 1}) ${ago(c.finished_ms)} ago`
    case 'created':
      return 'Created'
    case 'dead':
      return 'Dead'
    case 'removing':
      return 'Removal in progress'
  }
}

function build(host: string, spec: ContainerSpec, now: number): Container {
  const state = spec.state ?? 'running'
  const startedAgo = spec.started_ago_ms ?? 3 * DAY
  const started_ms = state === 'created' ? null : now - startedAgo
  const finished_ms = state === 'exited' || state === 'restarting' || state === 'dead' ? now - (spec.finished_ago_ms ?? 2 * HOUR) : null
  const running = state === 'running' || state === 'paused'
  const key = host + '/' + spec.name
  const labels: Record<string, string> = { ...(spec.labels ?? {}) }
  if (spec.project) {
    labels['com.docker.compose.project'] = spec.project
    labels['com.docker.compose.service'] = spec.service ?? spec.name
    labels['com.docker.compose.version'] = '2.29.2'
  }
  const managedBy = spec.managed ?? (spec.project ? adoptedProjects.get(host + '/' + spec.project) : undefined)
  if (managedBy) labels['io.photon.stack'] = managedBy
  const c: Container = {
    id: shortId(key),
    name: spec.name,
    image: spec.image,
    image_id: 'sha256:' + shortId(spec.image) + shortId(spec.image + 'y'),
    command: spec.command ?? '/entrypoint.sh',
    state,
    health: running ? (spec.health ?? 'none') : 'none',
    status_text: '',
    created_ms: now - startedAgo - 5 * MIN,
    started_ms,
    finished_ms,
    exit_code: running ? null : (spec.exit_code ?? 0),
    restart_count: spec.restarts ?? 0,
    restart_policy: spec.project ? 'unless-stopped' : 'no',
    ports: (spec.ports ?? []).map(parsePort),
    mounts: (spec.mounts ?? []).map(parseMount),
    networks: spec.networks ?? (spec.project ? [`${spec.project}_default`] : ['bridge']),
    labels,
    env_keys: spec.env ?? ['PATH', 'TZ'],
    env: Object.fromEntries((spec.env ?? ['PATH', 'TZ']).map((k) => [k, /pass|secret|token|key|licen/i.test(k) ? '••••••••' : k === 'PATH' ? '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin' : k === 'TZ' ? 'Asia/Jakarta' : k.startsWith('OTEL_EXPORTER') ? 'http://photon:4318' : k === 'OTEL_SERVICE_NAME' ? (spec.service ?? spec.name) : k === 'OTEL_RESOURCE_ATTRIBUTES' ? `deployment.environment=prod,host.name=${host}` : k.endsWith('_URL') ? `${k.toLowerCase().replace('_url', '')}://…` : k === 'MODEL_PATH' ? '/models/lpr-id-v7' : 'value'])),
    compose_project: spec.project ?? null,
    compose_service: spec.project ? (spec.service ?? spec.name) : null,
    managed_stack_id: spec.managed ?? null,
    cpu_pct: running ? Math.round(((spec.cpu ?? 5) + (noise(key, Math.floor(now / 15_000)) - 0.5) * (spec.cpu ?? 5) * 0.4) * 10) / 10 : null,
    mem_bytes: running ? Math.round((spec.mem ?? 128 * MB) * (0.95 + noise(key + 'm', Math.floor(now / 15_000)) * 0.1)) : null,
    mem_limit_bytes: running ? (spec.mem_limit ?? null) : null,
    net_rx_bps: running ? Math.round(40_000 + noise(key + 'rx', Math.floor(now / 15_000)) * 900_000) : null,
    net_tx_bps: running ? Math.round(20_000 + noise(key + 'tx', Math.floor(now / 15_000)) * 300_000) : null,
    block_read_bps: running ? Math.round(noise(key + 'br', Math.floor(now / 15_000)) * 2_000_000) : null,
    block_write_bps: running ? Math.round(noise(key + 'bw', Math.floor(now / 15_000)) * 6_000_000) : null,
  }
  c.status_text = statusText(c, now)
  return c
}

const OTEL_ENV = ['OTEL_EXPORTER_OTLP_ENDPOINT', 'OTEL_SERVICE_NAME', 'OTEL_RESOURCE_ATTRIBUTES', 'PHOTON_INGEST_TOKEN']

const CONTAINER_SPECS: Record<string, ContainerSpec[]> = {
  'web-1': [
    { name: 'photon-demo-api-1', service: 'api', project: 'photon-demo', managed: 'st_web1_demo', image: 'ghcr.io/nodeflux/api:2.8.4', ports: ['8080:8080/tcp'], cpu: 42, mem: 512 * MB, mem_limit: 1 * GB, health: 'healthy', env: [...OTEL_ENV, 'DATABASE_URL', 'REDIS_URL'], mounts: ['api_data:/var/lib/api'], started_ago_ms: 3 * DAY + 4 * HOUR },
    { name: 'photon-demo-web-1', service: 'web', project: 'photon-demo', managed: 'st_web1_demo', image: 'ghcr.io/nodeflux/web:2.8.4', ports: ['80:80/tcp', '443:443/tcp'], cpu: 6, mem: 96 * MB, mem_limit: 256 * MB, health: 'healthy', env: [...OTEL_ENV], started_ago_ms: 3 * DAY + 4 * HOUR },
    { name: 'photon-demo-worker-1', service: 'worker', project: 'photon-demo', managed: 'st_web1_demo', image: 'ghcr.io/nodeflux/worker:2.8.4', cpu: 18, mem: 256 * MB, mem_limit: 512 * MB, restarts: 2, env: [...OTEL_ENV, 'QUEUE_URL'], started_ago_ms: 9 * HOUR },
    { name: 'photon-demo-postgres-1', service: 'postgres', project: 'photon-demo', managed: 'st_web1_demo', image: 'postgres:16.4-alpine', ports: ['127.0.0.1:5432:5432/tcp'], cpu: 9, mem: 380 * MB, mem_limit: 2 * GB, health: 'healthy', mounts: ['pgdata:/var/lib/postgresql/data'], env: ['POSTGRES_PASSWORD', 'POSTGRES_DB', 'PGDATA'], command: 'postgres -c shared_buffers=256MB', started_ago_ms: 3 * DAY + 4 * HOUR },
    { name: 'photon-demo-redis-1', service: 'redis', project: 'photon-demo', managed: 'st_web1_demo', image: 'redis:7.4-alpine', cpu: 2, mem: 24 * MB, mem_limit: 128 * MB, health: 'healthy', command: 'redis-server --save 60 1', started_ago_ms: 3 * DAY + 4 * HOUR },
    { name: 'node-exporter', image: 'prom/node-exporter:v1.8.2', ports: ['9100:9100/tcp'], cpu: 1, mem: 18 * MB, mounts: ['/:/host:ro'], started_ago_ms: 41 * DAY, labels: { 'org.opencontainers.image.source': 'https://github.com/prometheus/node_exporter' } },
    { name: 'old-migrator', image: 'ghcr.io/nodeflux/migrator:2.7.0', state: 'exited', exit_code: 0, finished_ago_ms: 6 * DAY, started_ago_ms: 6 * DAY + 2 * MIN, command: 'migrate up' },
    { name: 'debug-shell', image: 'alpine:3.20', state: 'exited', exit_code: 137, finished_ago_ms: 3 * HOUR, started_ago_ms: 4 * HOUR, command: 'sh' },
  ],
  'web-2': [
    { name: 'photon-demo-api-1', service: 'api', project: 'photon-demo', managed: 'st_web2_demo', image: 'ghcr.io/nodeflux/api:2.8.4', ports: ['8080:8080/tcp'], cpu: 51, mem: 640 * MB, mem_limit: 1 * GB, health: 'healthy', env: [...OTEL_ENV, 'DATABASE_URL', 'REDIS_URL'], mounts: ['api_data:/var/lib/api'], started_ago_ms: 22 * MIN },
    { name: 'photon-demo-web-1', service: 'web', project: 'photon-demo', managed: 'st_web2_demo', image: 'ghcr.io/nodeflux/web:2.8.4', ports: ['80:80/tcp', '443:443/tcp'], cpu: 5, mem: 88 * MB, mem_limit: 256 * MB, health: 'healthy', env: [...OTEL_ENV], started_ago_ms: 22 * MIN },
    { name: 'photon-demo-worker-1', service: 'worker', project: 'photon-demo', managed: 'st_web2_demo', image: 'ghcr.io/nodeflux/worker:2.8.4', cpu: 22, mem: 288 * MB, mem_limit: 512 * MB, health: 'unhealthy', restarts: 7, env: [...OTEL_ENV, 'QUEUE_URL'], started_ago_ms: 22 * MIN },
    { name: 'photon-demo-postgres-1', service: 'postgres', project: 'photon-demo', managed: 'st_web2_demo', image: 'postgres:16.4-alpine', ports: ['127.0.0.1:5432:5432/tcp'], cpu: 11, mem: 402 * MB, mem_limit: 2 * GB, health: 'healthy', mounts: ['pgdata:/var/lib/postgresql/data'], env: ['POSTGRES_PASSWORD', 'POSTGRES_DB'], started_ago_ms: 12 * DAY },
    { name: 'photon-demo-redis-1', service: 'redis', project: 'photon-demo', managed: 'st_web2_demo', image: 'redis:7.4-alpine', cpu: 2, mem: 26 * MB, mem_limit: 128 * MB, health: 'healthy', started_ago_ms: 12 * DAY },
    { name: 'node-exporter', image: 'prom/node-exporter:v1.8.2', ports: ['9100:9100/tcp'], cpu: 1, mem: 17 * MB, mounts: ['/:/host:ro'], started_ago_ms: 41 * DAY },
  ],
  'gpu-node-1': [
    { name: 'trainer', image: 'nvcr.io/nvidia/pytorch:24.08-py3', cpu: 88, mem: 24 * GB, mem_limit: 64 * GB, mounts: ['/data/datasets:/datasets:ro', 'checkpoints:/ckpt'], env: ['NVIDIA_VISIBLE_DEVICES', 'WANDB_API_KEY', 'RUN_ID'], command: 'python train.py --config cfg/a100.yaml', started_ago_ms: 26 * HOUR, labels: { team: 'ml' } },
    { name: 'ml-lab-jupyter-1', service: 'jupyter', project: 'ml-lab', image: 'jupyter/pytorch-notebook:2024-08-19', ports: ['8888:8888/tcp'], cpu: 3, mem: 1.2 * GB, mem_limit: 8 * GB, mounts: ['/home/ml/notebooks:/home/jovyan/work'], started_ago_ms: 12 * DAY },
    { name: 'ml-lab-dcgm-exporter-1', service: 'dcgm-exporter', project: 'ml-lab', image: 'nvcr.io/nvidia/k8s/dcgm-exporter:3.3.7-3.5.0-ubuntu22.04', ports: ['9400:9400/tcp'], cpu: 2, mem: 64 * MB, started_ago_ms: 12 * DAY },
  ],
  'vas-edge-01': [
    { name: 'nodeflux-vas-vas-inference-1', service: 'vas-inference', project: 'nodeflux-vas', managed: 'st_vas1', image: 'registry.divtik.local/nodeflux/vas-inference:2.4.1', cpu: 63, mem: 3.1 * GB, mem_limit: 4 * GB, health: 'healthy', env: [...OTEL_ENV, 'MODEL_PATH', 'LICENSE_KEY'], mounts: ['models:/models:ro', '/var/vas/frames:/frames'], started_ago_ms: 25 * HOUR },
    { name: 'nodeflux-vas-vas-api-1', service: 'vas-api', project: 'nodeflux-vas', managed: 'st_vas1', image: 'registry.divtik.local/nodeflux/vas-api:2.4.1', ports: ['8443:8443/tcp'], cpu: 8, mem: 210 * MB, mem_limit: 512 * MB, health: 'healthy', env: [...OTEL_ENV, 'DB_PASSWORD', 'JWT_SECRET'], started_ago_ms: 25 * HOUR },
    { name: 'nodeflux-vas-vas-db-1', service: 'vas-db', project: 'nodeflux-vas', managed: 'st_vas1', image: 'postgres:16.4-alpine', cpu: 4, mem: 260 * MB, mem_limit: 1 * GB, health: 'healthy', mounts: ['vasdb:/var/lib/postgresql/data'], env: ['POSTGRES_PASSWORD'], started_ago_ms: 25 * HOUR },
    { name: 'nodeflux-vas-mqtt-1', service: 'mqtt', project: 'nodeflux-vas', managed: 'st_vas1', image: 'eclipse-mosquitto:2.0.18', ports: ['1883:1883/tcp', '9001:9001/tcp'], cpu: 1, mem: 12 * MB, mem_limit: 64 * MB, started_ago_ms: 25 * HOUR },
    { name: 'watchtower', image: 'containrrr/watchtower:1.7.1', state: 'exited', exit_code: 0, finished_ago_ms: 25 * HOUR, started_ago_ms: 60 * DAY, mounts: ['/var/run/docker.sock:/var/run/docker.sock'], labels: { 'com.centurylinklabs.watchtower': 'true' } },
  ],
  'vas-edge-02': [
    { name: 'nodeflux-vas-vas-inference-1', service: 'vas-inference', project: 'nodeflux-vas', managed: 'st_vas2', image: 'registry.divtik.local/nodeflux/vas-inference:2.3.7', cpu: 58, mem: 2.9 * GB, mem_limit: 4 * GB, health: 'healthy', env: [...OTEL_ENV, 'MODEL_PATH', 'LICENSE_KEY'], started_ago_ms: 9 * DAY },
    { name: 'nodeflux-vas-vas-api-1', service: 'vas-api', project: 'nodeflux-vas', managed: 'st_vas2', image: 'registry.divtik.local/nodeflux/vas-api:2.3.7', ports: ['8443:8443/tcp'], cpu: 7, mem: 190 * MB, mem_limit: 512 * MB, health: 'healthy', started_ago_ms: 9 * DAY },
    { name: 'nodeflux-vas-vas-db-1', service: 'vas-db', project: 'nodeflux-vas', managed: 'st_vas2', image: 'postgres:16.4-alpine', cpu: 4, mem: 240 * MB, mem_limit: 1 * GB, health: 'healthy', started_ago_ms: 9 * DAY },
    { name: 'nodeflux-vas-mqtt-1', service: 'mqtt', project: 'nodeflux-vas', managed: 'st_vas2', image: 'eclipse-mosquitto:2.0.18', ports: ['1883:1883/tcp'], cpu: 1, mem: 11 * MB, mem_limit: 64 * MB, started_ago_ms: 9 * DAY },
    { name: 'legacy-recorder', image: 'divtik/recorder:0.9', state: 'restarting', exit_code: 1, restarts: 214, finished_ago_ms: 40_000, started_ago_ms: 9 * DAY },
  ],
}

function imagesFor(host: string, containers: Container[], now: number): ImageInfo[] {
  const byImage = new Map<string, number>()
  for (const c of containers) byImage.set(c.image, (byImage.get(c.image) ?? 0) + 1)
  const rows: ImageInfo[] = Array.from(byImage.entries()).map(([image, used]) => ({
    id: 'sha256:' + shortId(image) + shortId(image + 'y'),
    repo_tags: [image],
    size_bytes: Math.round((40 + noise(image, 1) * 900) * MB * (image.includes('pytorch') ? 12 : image.includes('inference') ? 4 : 1)),
    created_ms: now - Math.round(noise(image, 2) * 120) * DAY,
    used_by: used,
    dangling: false,
  }))
  // Leftovers from previous versions — what prune would reclaim.
  const olds = host.startsWith('vas') ? ['registry.divtik.local/nodeflux/vas-inference:2.3.5', 'registry.divtik.local/nodeflux/vas-api:2.3.5'] : host === 'gpu-node-1' ? [] : ['ghcr.io/nodeflux/api:2.8.3', 'ghcr.io/nodeflux/web:2.8.3', 'ghcr.io/nodeflux/worker:2.8.3']
  for (const image of olds) rows.push({ id: 'sha256:' + shortId(image) + shortId(image + 'y'), repo_tags: [image], size_bytes: Math.round((40 + noise(image, 1) * 900) * MB), created_ms: now - 30 * DAY, used_by: 0, dangling: false })
  if (host !== 'gpu-node-1') rows.push({ id: 'sha256:' + shortId(host + 'dangling') + shortId(host + 'dz'), repo_tags: [], size_bytes: 312 * MB, created_ms: now - 44 * DAY, used_by: 0, dangling: true })
  return rows.sort((a, b) => b.size_bytes - a.size_bytes)
}

function volumesFor(host: string, containers: Container[], now: number): VolumeInfo[] {
  const seen = new Map<string, string[]>()
  for (const c of containers) for (const m of c.mounts) if (m.type === 'volume') seen.set(m.source, [...(seen.get(m.source) ?? []), c.name])
  const rows: VolumeInfo[] = Array.from(seen.entries()).map(([name, used]) => ({
    name,
    driver: 'local',
    mountpoint: `/var/lib/docker/volumes/${name}/_data`,
    size_bytes: Math.round((name.includes('pg') || name.includes('db') ? 6 : name.includes('ckpt') ? 380 : name.includes('models') ? 9 : 0.4) * GB),
    used_by: used,
    created_ms: now - 40 * DAY,
  }))
  rows.push({ name: `${host.replace(/-/g, '')}_orphan_2f9a`, driver: 'local', mountpoint: `/var/lib/docker/volumes/${host}_orphan_2f9a/_data`, size_bytes: 1.1 * GB, used_by: [], created_ms: now - 90 * DAY })
  return rows
}

function networksFor(containers: Container[]): NetworkInfo[] {
  const counts = new Map<string, number>()
  for (const c of containers) for (const n of c.networks) counts.set(n, (counts.get(n) ?? 0) + 1)
  const rows: NetworkInfo[] = [
    { id: shortId('bridge'), name: 'bridge', driver: 'bridge', scope: 'local', internal: false, containers: counts.get('bridge') ?? 0, subnet: '172.17.0.0/16' },
    { id: shortId('host'), name: 'host', driver: 'host', scope: 'local', internal: false, containers: 0, subnet: null },
    { id: shortId('none'), name: 'none', driver: 'null', scope: 'local', internal: true, containers: 0, subnet: null },
  ]
  for (const [name, n] of counts) if (name !== 'bridge') rows.push({ id: shortId(name), name, driver: 'bridge', scope: 'local', internal: false, containers: n, subnet: `172.${18 + (hash(name) % 40)}.0.0/16` })
  return rows
}

const pruned = new Set<string>()

export function mockHostInventory(host: string): HostInventory | null {
  const managed = managedHosts.find((h) => h.host === host)
  if (!managed) return null
  const now = Date.now()
  const specs = CONTAINER_SPECS[host] ?? []
  // An offline host reports what the agent last saw, frozen at its last sync.
  const asOf = managed.status === 'offline' && managed.last_seen_ms ? managed.last_seen_ms : now
  const containers = specs.map((s) => build(host, s, asOf))
  const gone = removedImages.get(host) ?? new Set<string>()
  const images = imagesFor(host, containers, asOf).filter((i) => !gone.has(i.id) && !i.repo_tags.some((t) => gone.has(t)))
  // The daemon's `reclaimable` is what prune's default selection (images, stopped containers,
  // build cache — not volumes) would free, so the KPI and the dialog say the same number.
  const stopped = containers.filter((c) => c.state === 'exited' || c.state === 'dead' || c.state === 'created').length
  const reclaimable = pruned.has(host) ? 0 : images.filter((i) => !i.used_by).reduce((n, i) => n + i.size_bytes, 0) + stopped * 38 * MB + (managed.docker?.disk.build_cache_bytes ?? 0)
  const facts = managed.docker ? { ...managed.docker, disk: { ...managed.docker.disk, reclaimable_bytes: reclaimable } } : null
  return {
    host,
    agent_status: managed.status,
    fetched_ms: asOf,
    stale: managed.status !== 'online',
    containers,
    images,
    volumes: volumesFor(host, containers, asOf).filter((v) => !(removedVolumes.get(host)?.has(v.name) ?? false)),
    networks: networksFor(containers),
    facts,
  }
}

// --- Container logs: a per-container "program" of line templates, stamped by wall clock, so a
// follow poll can return "everything since T" without keeping a buffer. ---
const LOG_PROGRAMS: Record<string, string[]> = {
  api: [
    'INFO  http: GET /api/v1/frames 200 12ms trace=%t',
    'INFO  http: POST /api/v1/events 202 4ms trace=%t',
    'DEBUG pool: acquired conn 7/32 in 0.3ms',
    'INFO  http: GET /healthz 200 1ms',
    'WARN  cache: miss ratio 0.41 over 60s (threshold 0.30)',
    'INFO  http: GET /api/v1/frames 200 9ms trace=%t',
    'ERROR upstream: dial tcp 10.0.0.9:9000: i/o timeout (retry 1/3)',
    'INFO  http: PUT /api/v1/config 200 31ms trace=%t',
  ],
  worker: [
    'INFO  job=ingest.batch id=%t items=128 took=412ms',
    'INFO  job=ingest.batch id=%t items=96 took=380ms',
    'WARN  job=reindex id=%t retry=2 reason="lock held"',
    'ERROR job=ingest.batch id=%t panic: index out of range [3] with length 3',
    'INFO  worker: restarted by supervisor (restart_count=7)',
  ],
  postgres: ['LOG:  checkpoint starting: time', 'LOG:  checkpoint complete: wrote 412 buffers (0.3%); 0 WAL file(s) added', 'LOG:  automatic vacuum of table "public.events": index scans: 1'],
  redis: ['* 1 changes in 60 seconds. Saving...', '* Background saving started by pid 3021', '* DB saved on disk', '* Background saving terminated with success'],
  web: ['10.0.0.4 - - "GET / HTTP/1.1" 200 1843 "-" "Mozilla/5.0"', '10.0.0.4 - - "GET /assets/app.js HTTP/1.1" 304 0', '10.0.0.7 - - "GET /api/health HTTP/1.1" 200 15'],
  default: ['INFO  tick %t', 'INFO  heartbeat ok', 'DEBUG gc: freed 12 MB', 'WARN  slow poll 1203ms'],
  inference: ['INFO  stream=cam-04 fps=24.8 latency_ms=38 objects=3', 'INFO  stream=cam-11 fps=25.0 latency_ms=41 objects=0', 'WARN  stream=cam-07 dropped 3 frames (decoder backlog)', 'INFO  model: batch=8 gpu_ms=17.2', 'ERROR stream=cam-02 rtsp: connection reset by peer, reconnecting in 2s'],
  recorder: ['FATAL config: /etc/recorder/config.yml: no such file or directory', 'panic: nil pointer dereference', 'goroutine 1 [running]:', 'main.main()\n\t/src/main.go:42 +0x1f'],
}

function programFor(c: Container): string[] {
  const n = (c.compose_service ?? c.name).toLowerCase()
  for (const k of Object.keys(LOG_PROGRAMS)) if (k !== 'default' && n.includes(k)) return LOG_PROGRAMS[k]
  return LOG_PROGRAMS.default
}

function findContainer(host: string, id: string): Container | null {
  const inv = mockHostInventory(host)
  return inv?.containers.find((c) => c.id === id || c.name === id) ?? null
}

export function mockContainerLogs(host: string, id: string, q: ContainerLogsQuery = {}): ContainerLogsResult {
  const c = findContainer(host, id)
  if (!c) return { lines: [], truncated: false }
  const program = programFor(c)
  const key = host + '/' + c.name
  const running = c.state === 'running' || c.state === 'restarting'
  // A running container writes a line every ~1.4s; a stopped one stopped at `finished_ms`.
  const cadence = 1_400
  const end = running ? Date.now() : (c.finished_ms ?? Date.now())
  const tail = q.tail ?? 200
  const sinceMs = q.since_ms ?? end - tail * cadence
  const lines: ContainerLogLine[] = []
  const firstIdx = Math.floor(sinceMs / cadence) + 1
  const lastIdx = Math.floor(end / cadence)
  for (let i = firstIdx; i <= lastIdx; i++) {
    const tpl = program[(hash(key) + i) % program.length]
    // Error lines are rarer than their slot in the program suggests.
    if ((tpl.startsWith('ERROR') || tpl.startsWith('FATAL')) && noise(key + 'e', i) > 0.35) continue
    if (tpl.startsWith('WARN') && noise(key + 'w', i) > 0.6) continue
    lines.push({ ts_ms: i * cadence + (hash(key + i) % 900), stream: /^(ERROR|FATAL|panic|goroutine|\t)/.test(tpl) ? 'stderr' : 'stdout', text: tpl.replace('%t', shortId(key + i).slice(0, 12)) })
  }
  const truncated = q.since_ms == null && lines.length > tail
  return { lines: truncated ? lines.slice(-tail) : lines, truncated }
}

export function mockContainerStats(host: string, id: string, startNs: string, endNs: string): ContainerStats {
  const c = findContainer(host, id)
  const start = Number(BigInt(startNs) / 1_000_000n)
  const end = Number(BigInt(endNs) / 1_000_000n)
  const key = host + '/' + (c?.name ?? id)
  const n = 60
  const step = Math.max(1000, Math.floor((end - start) / n))
  const running = c?.state === 'running' || c?.state === 'paused'
  const series = (base: number, spread: number, k: string): ContainerStatPoint[] => {
    const out: ContainerStatPoint[] = []
    for (let i = 0; i <= n; i++) {
      const t = start + i * step
      if (!running || (c?.started_ms != null && t < c.started_ms)) out.push({ t, v: null })
      else {
        const j = Math.floor(t / step)
        const n3 = (noise(key + k, j) + noise(key + k, j - 1) + noise(key + k, j + 1)) / 3
        out.push({ t, v: Math.max(0, base + (n3 - 0.5) * spread * 0.7 + Math.sin(i / 9) * spread * 0.3) })
      }
    }
    return out
  }
  const cpuBase = c?.cpu_pct ?? 5
  const memBase = c?.mem_bytes ?? 128 * MB
  return {
    cpu_pct: series(cpuBase, cpuBase * 0.6, 'cpu'),
    mem_bytes: series(memBase, memBase * 0.08, 'mem'),
    net_rx_bps: series(300_000, 400_000, 'rx'),
    net_tx_bps: series(120_000, 150_000, 'tx'),
    block_read_bps: series(600_000, 900_000, 'br'),
    block_write_bps: series(2_000_000, 3_000_000, 'bw'),
    mem_limit_bytes: c?.mem_limit_bytes ?? null,
  }
}

export function mockDockerEvents(host: string, limit = 50): DockerEventsResult {
  const inv = mockHostInventory(host)
  if (!inv) return { events: [] }
  const now = inv.fetched_ms
  const events: DockerEvent[] = []
  let seq = 0
  const push = (ts_ms: number, type: DockerEvent['type'], action: string, actor: string, attributes: Record<string, string> = {}) =>
    events.push({ id: `ev_${host}_${seq++}`, ts_ms, type, action, actor, attributes })
  for (const c of inv.containers) {
    const attrs = { image: c.image, ...(c.compose_project ? { 'com.docker.compose.project': c.compose_project } : {}) }
    if (c.started_ms != null) {
      push(c.started_ms - 800, 'container', 'create', c.name, attrs)
      push(c.started_ms, 'container', 'start', c.name, attrs)
    }
    if (c.state === 'exited' && c.finished_ms != null) {
      push(c.finished_ms, 'container', 'die', c.name, { ...attrs, exitCode: String(c.exit_code ?? 0) })
      if (c.exit_code === 137) push(c.finished_ms - 20, 'container', 'oom', c.name, attrs)
    }
    if (c.state === 'restarting' && c.finished_ms != null) {
      for (let i = 0; i < 6; i++) {
        push(c.finished_ms - i * 45_000, 'container', 'die', c.name, { ...attrs, exitCode: '1' })
        push(c.finished_ms - i * 45_000 + 30_000, 'container', 'start', c.name, attrs)
      }
    }
    if (c.health === 'unhealthy' && c.started_ms != null) push(now - 4 * MIN, 'container', 'health_status: unhealthy', c.name, attrs)
    if (c.restart_count > 0 && c.state === 'running' && c.started_ms != null) push(c.started_ms - 2000, 'container', 'die', c.name, { ...attrs, exitCode: '2' })
  }
  for (const img of inv.images) if (img.used_by > 0) push(img.created_ms + 3 * HOUR, 'image', 'pull', img.repo_tags[0] ?? img.id.slice(7, 19))
  return { events: events.sort((a, b) => b.ts_ms - a.ts_ms).slice(0, limit) }
}

// ---------------------------------------------------------------------------
// One-shot actions. Time-driven: an action on an online host runs for RUN_MS then lands, and its
// effect is applied to the inventory through `overrides` (state, restart count, image). On an
// offline host it stays queued until its lease expires — that is the whole point of the model.
// ---------------------------------------------------------------------------
import type { ActionInput, ActionRecord, ActionResult, ActionsResult, PrunePreview } from '@/lib/core/api'

const RUN_MS = 2_600
const ACTION_TTL = 15 * MIN

interface Override {
  state?: Container['state']
  exit_code?: number | null
  started_ms?: number | null
  finished_ms?: number | null
  restart_delta?: number
  removed?: boolean
  image?: string
  health?: Container['health']
}
const overrides = new Map<string, Override>() // key: host/name
// Compose projects adopted through the UI: host/project → stack id, so their containers carry
// the `io.photon.stack` label from the moment they are adopted (nothing is recreated).
const adoptedProjects = new Map<string, string>()
// Images removed through an action: host → image ids/tags gone until the next pull.
const removedImages = new Map<string, Set<string>>()
const removedVolumes = new Map<string, Set<string>>()
let actionSeq = 0
let actions: ActionRecord[] = []

function applyOverride(host: string, c: Container, now: number): Container | null {
  const o = overrides.get(host + '/' + c.name)
  if (!o) return c
  if (o.removed) return null
  const next: Container = { ...c }
  if (o.state) next.state = o.state
  if (o.exit_code !== undefined) next.exit_code = o.exit_code
  if (o.started_ms !== undefined) next.started_ms = o.started_ms
  if (o.finished_ms !== undefined) next.finished_ms = o.finished_ms
  if (o.restart_delta) next.restart_count += o.restart_delta
  if (o.image) next.image = o.image
  if (o.health) next.health = o.health
  const running = next.state === 'running' || next.state === 'paused'
  if (!running) {
    next.cpu_pct = null
    next.mem_bytes = null
    next.mem_limit_bytes = null
    next.health = 'none'
  } else if (next.cpu_pct == null) {
    next.cpu_pct = 2.1
    next.mem_bytes = 64 * MB
  }
  next.status_text = statusText(next, now)
  return next
}

// Patch `mockHostInventory` so actions show. (Declared after it, so wrap the export.)
const baseInventory = mockHostInventory
export function mockHostInventoryWithActions(host: string): HostInventory | null {
  settleActions()
  const inv = baseInventory(host)
  if (!inv) return null
  inv.containers = inv.containers.map((c) => applyOverride(host, c, inv.fetched_ms)).filter((c): c is Container => c !== null)
  return inv
}

function settleActions(): void {
  const now = Date.now()
  for (const a of actions) {
    const managed = managedHosts.find((h) => h.host === a.host)
    if (a.status === 'queued') {
      if (managed?.status === 'online') {
        a.status = 'running'
        a.started_ms = now
      } else if (now > a.expires_ms) {
        a.status = 'expired'
        a.finished_ms = now
        a.result = 'the host did not pick this up before it expired'
        pushAudit({ action: `container.${a.kind}`, target: { host: a.host, container: a.container_name ?? undefined }, result: 'failed', detail: a.result })
      }
    }
    if (a.status === 'running' && a.started_ms != null && now - a.started_ms >= RUN_MS) {
      a.finished_ms = now
      const key = a.host + '/' + (a.container_name ?? '')
      const fails = a.kind === 'pull' && (a.container_name ?? '').includes('worker') && (a.host === 'web-1')
      if (fails) {
        a.status = 'failed'
        a.result = 'manifest unknown: ghcr.io/nodeflux/worker:2.8.4 (registry returned 404)'
      } else {
        a.status = 'done'
        const prev = overrides.get(key) ?? {}
        switch (a.kind) {
          case 'start':
            overrides.set(key, { ...prev, state: 'running', exit_code: null, started_ms: now, finished_ms: null })
            a.result = 'started in 0.8s'
            break
          case 'stop':
            overrides.set(key, { ...prev, state: 'exited', exit_code: 0, finished_ms: now })
            a.result = 'stopped in 1.4s (SIGTERM)'
            break
          case 'kill':
            overrides.set(key, { ...prev, state: 'exited', exit_code: 137, finished_ms: now })
            a.result = 'killed (SIGKILL)'
            break
          case 'restart':
            overrides.set(key, { ...prev, state: 'running', exit_code: null, started_ms: now, finished_ms: null, restart_delta: (prev.restart_delta ?? 0) + 1, health: 'healthy' })
            a.result = 'restarted in 2.1s'
            break
          case 'pull':
            a.result = 'image is up to date (digest unchanged)'
            break
          case 'recreate':
            overrides.set(key, { ...prev, state: 'running', exit_code: null, started_ms: now, finished_ms: null, restart_delta: 0 })
            a.result = a.args.pull ? 'pulled and recreated in 6.3s' : 'recreated in 2.4s'
            break
          case 'remove':
            overrides.set(key, { ...prev, removed: true })
            a.result = a.args.remove_volumes ? 'removed with its anonymous volumes' : 'removed; volumes kept'
            break
          case 'image_pull':
            a.result = 'image is up to date (digest unchanged)'
            break
          case 'image_remove': {
            const set = removedImages.get(a.host) ?? new Set<string>()
            if (a.image) set.add(a.image)
            removedImages.set(a.host, set)
            a.result = 'untagged and deleted'
            break
          }
          case 'volume_remove': {
            const set = removedVolumes.get(a.host) ?? new Set<string>()
            if (a.volume) set.add(a.volume)
            removedVolumes.set(a.host, set)
            a.result = `removed volume ${a.volume ?? ''}`.trim()
            break
          }
          case 'prune': {
            const p = mockPrunePreview(a.host)
            const freed = (a.args.images ? p.images.bytes : 0) + (a.args.containers ? p.containers.bytes : 0) + (a.args.volumes ? p.volumes.bytes : 0) + (a.args.build_cache ? p.build_cache.bytes : 0)
            a.result = `freed ${(freed / GB).toFixed(1)} GB`
            pruned.add(a.host)
            break
          }
        }
      }
      pushAudit({
        action: a.kind === 'prune' ? 'prune' : a.kind.startsWith('image_') ? `image.${a.kind.slice(6)}` : a.kind.startsWith('volume_') ? `volume.${a.kind.slice(7)}` : `container.${a.kind}`,
        target: { host: a.host, container: a.container_name ?? undefined, image: a.image ?? undefined, volume: a.volume ?? undefined },
        result: a.status === 'done' ? 'ok' : 'failed',
        detail: a.result,
        payload_digest: 'sha256:' + shortId(a.id).slice(0, 4) + '…' + shortId(a.id + 'z').slice(0, 4),
      })
    }
  }
}


export function mockCreateAction(host: string, input: ActionInput): ActionResult {
  const managed = managedHosts.find((h) => h.host === host)
  if (!managed) return { ok: false, error: 'no such host' }
  if (!managed.capabilities.includes('lifecycle')) return { ok: false, error: 'the agent on this host was installed without --docker-lifecycle' }
  let name: string | null = null
  if (input.kind === 'image_pull' || input.kind === 'image_remove') {
    const inv = mockHostInventoryWithActions(host)
    const img = inv?.images.find((i) => i.id === input.image || i.repo_tags.includes(input.image ?? ''))
    if (!img) return { ok: false, error: 'no such image' }
    if (input.kind === 'image_remove' && img.used_by > 0) return { ok: false, error: `${img.repo_tags[0] ?? 'image'} is used by ${img.used_by} container${img.used_by === 1 ? '' : 's'}` }
  } else if (input.kind === 'volume_remove') {
    if (!input.volume) return { ok: false, error: 'volume is required' }
    const inv = mockHostInventoryWithActions(host)
    const v = inv?.volumes.find((x) => x.name === input.volume)
    if (!v) return { ok: false, error: `no volume ${input.volume} on ${host} in its last inventory` }
    if (v.used_by.length) return { ok: false, error: `${v.name} is used by ${v.used_by.join(', ')}` }
  } else if (input.kind !== 'prune') {
    const inv = mockHostInventoryWithActions(host)
    const c = inv?.containers.find((x) => x.id === input.container_id || x.name === input.container_id)
    if (!c) return { ok: false, error: 'no such container' }
    name = c.name
  }
  const now = Date.now()
  const a: ActionRecord = {
    id: 'ac_' + (actionSeq += 1) + '_' + shortId(host + now).slice(0, 6),
    host,
    kind: input.kind,
    container_id: input.container_id ?? null,
    container_name: name,
    image: input.image ?? null,
    volume: input.volume ?? null,
    args: input.args ?? {},
    status: 'queued',
    actor: 'demo',
    created_ms: now,
    started_ms: null,
    finished_ms: null,
    expires_ms: now + ACTION_TTL,
    result: null,
  }
  actions = [a, ...actions].slice(0, 200)
  if (managed.status !== 'online') {
    pushAudit({ action: a.kind === 'prune' ? 'prune' : a.kind.startsWith('image_') ? `image.${a.kind.slice(6)}` : a.kind.startsWith('volume_') ? `volume.${a.kind.slice(7)}` : `container.${a.kind}`, target: { host, container: name ?? undefined, image: input.image, volume: input.volume }, result: 'pending', detail: 'host offline — waits for the next sync, expires in 15m' })
  }
  settleActions()
  return { ok: true, action: { ...a } }
}

export function mockHostActions(host: string, limit = 20): ActionsResult {
  settleActions()
  return { actions: actions.filter((a) => a.host === host).slice(0, limit).map((a) => ({ ...a })) }
}

export function mockPrunePreview(host: string): PrunePreview {
  const inv = baseInventory(host)
  const done = pruned.has(host)
  const unusedImages = inv?.images.filter((i) => !i.used_by) ?? []
  const stopped = inv?.containers.filter((c) => c.state === 'exited' || c.state === 'dead' || c.state === 'created') ?? []
  const unusedVolumes = inv?.volumes.filter((v) => !v.used_by.length) ?? []
  return {
    host,
    images: done ? { count: 0, bytes: 0 } : { count: unusedImages.length, bytes: unusedImages.reduce((n, i) => n + i.size_bytes, 0) },
    containers: done ? { count: 0, bytes: 0 } : { count: stopped.length, bytes: stopped.length * 38 * MB },
    volumes: done ? { count: 0, bytes: 0 } : { count: unusedVolumes.length, bytes: unusedVolumes.reduce((n, v) => n + (v.size_bytes ?? 0), 0) },
    build_cache: done ? { count: 0, bytes: 0 } : { count: 41, bytes: inv?.facts?.disk.build_cache_bytes ?? 0 },
  }
}

// ---------------------------------------------------------------------------
// Stacks. Desired state per (host, name): files + generation; observed state comes from the
// inventory; drift is the diff. Applies are time-driven like actions; a stack on an offline host
// sits at `pending` until the host is back (it never fails for being offline).
// ---------------------------------------------------------------------------
import type {
  DeployOptions,
  DiscoveredProject,
  DiscoveredProjectsResult,
  Stack,
  StackActionArgs,
  StackActionKind,
  StackFile,
  StackInput,
  StackResult,
  StackServiceState,
  StacksResult,
  StackValidation,
} from '@/lib/core/api'
import { lintCompose, composeServices, parseEnv, missingVariables, isComposeFile, isSafePath } from '@/lib/deploy/compose'

const APPLY_MS = 3_200
const DEFAULT_OPTS: DeployOptions = { pull: 'missing', force_recreate: false, recreate_volumes: false }

const VAS_COMPOSE = `services:
  vas-inference:
    image: registry.divtik.local/nodeflux/vas-inference:\${VAS_VERSION}
    restart: unless-stopped
    environment:
      MODEL_PATH: /models/\${MODEL}
      LICENSE_KEY: \${secret:LICENSE_KEY}
      OTEL_EXPORTER_OTLP_ENDPOINT: \${OTEL_EXPORTER_OTLP_ENDPOINT}
      OTEL_SERVICE_NAME: vas-inference
      OTEL_RESOURCE_ATTRIBUTES: deployment.environment=\${ENVIRONMENT}
    volumes:
      - models:/models:ro
      - /var/vas/frames:/frames
    deploy:
      resources:
        limits:
          memory: 4g
  vas-api:
    image: registry.divtik.local/nodeflux/vas-api:\${VAS_VERSION}
    restart: unless-stopped
    ports:
      - "\${API_PORT}:8443"
    environment:
      DB_PASSWORD: \${secret:DB_PASSWORD}
      JWT_SECRET: \${secret:JWT_SECRET}
      OTEL_EXPORTER_OTLP_ENDPOINT: \${OTEL_EXPORTER_OTLP_ENDPOINT}
      OTEL_SERVICE_NAME: vas-api
    depends_on: [vas-db, mqtt]
  vas-db:
    image: postgres:16.4-alpine
    restart: unless-stopped
    environment:
      POSTGRES_PASSWORD: \${secret:DB_PASSWORD}
    volumes:
      - vasdb:/var/lib/postgresql/data
  mqtt:
    image: eclipse-mosquitto:2.0.18
    restart: unless-stopped
    ports:
      - "1883:1883"

volumes:
  models:
  vasdb:
`
const DEMO_COMPOSE = `services:
  api:
    image: ghcr.io/nodeflux/api:\${VERSION}
    restart: unless-stopped
    ports: ["8080:8080"]
    environment:
      DATABASE_URL: postgres://photon:\${secret:DB_PASSWORD}@postgres:5432/photon
      REDIS_URL: redis://redis:6379
      OTEL_EXPORTER_OTLP_ENDPOINT: \${OTEL_EXPORTER_OTLP_ENDPOINT}
      OTEL_SERVICE_NAME: api
    volumes: [api_data:/var/lib/api]
    depends_on: [postgres, redis]
  web:
    image: ghcr.io/nodeflux/web:\${VERSION}
    restart: unless-stopped
    ports: ["80:80", "443:443"]
    environment:
      OTEL_EXPORTER_OTLP_ENDPOINT: \${OTEL_EXPORTER_OTLP_ENDPOINT}
      OTEL_SERVICE_NAME: web
  worker:
    image: ghcr.io/nodeflux/worker:\${VERSION}
    restart: unless-stopped
    environment:
      QUEUE_URL: redis://redis:6379/1
      OTEL_EXPORTER_OTLP_ENDPOINT: \${OTEL_EXPORTER_OTLP_ENDPOINT}
      OTEL_SERVICE_NAME: worker
  postgres:
    image: postgres:16.4-alpine
    restart: unless-stopped
    ports: ["127.0.0.1:5432:5432"]
    environment:
      POSTGRES_PASSWORD: \${secret:DB_PASSWORD}
    volumes: [pgdata:/var/lib/postgresql/data]
  redis:
    image: redis:7.4-alpine
    restart: unless-stopped
    command: redis-server --save 60 1

volumes:
  api_data:
  pgdata:
`
const SIGNAGE_COMPOSE = `services:
  signage:
    image: registry.divtik.local/nodeflux/kiosk-signage:1.2.0
    restart: unless-stopped
    ports:
      - "8443:8443"
    privileged: true
    volumes:
      - /dev/dri:/dev/dri
`

interface StackSeed {
  id: string
  name: string
  host: string
  site: string | null
  source: Stack['source']
  template?: { id: string; name: string; version: string }
  status: Stack['status']
  desired: number
  applied: number | null
  compose: string
  env: string
  variables: Record<string, string>
  secrets: string[]
  drift?: Stack['drift']
  apm: string[]
  last_error?: string
  created_ago: number
  updated_ago: number
  applied_ago?: number
  acked?: string[]
  opts?: Partial<DeployOptions>
}

const now0 = Date.now()
const SEEDS: StackSeed[] = [
  { id: 'st_web1_demo', name: 'photon-demo', host: 'web-1', site: null, source: 'template', template: { id: 'photon-demo', name: 'Photon demo app', version: '2.8.4' }, status: 'running', desired: 15, applied: 15, compose: DEMO_COMPOSE, env: 'VERSION=2.8.4\nOTEL_EXPORTER_OTLP_ENDPOINT=http://photon:4318\n', variables: { VERSION: '2.8.4', OTEL_EXPORTER_OTLP_ENDPOINT: 'http://photon:4318' }, secrets: ['DB_PASSWORD'], apm: ['api', 'web', 'worker'], created_ago: 40 * DAY, updated_ago: 3 * DAY + 4 * HOUR, applied_ago: 3 * DAY + 4 * HOUR },
  { id: 'st_web2_demo', name: 'photon-demo', host: 'web-2', site: null, source: 'template', template: { id: 'photon-demo', name: 'Photon demo app', version: '2.8.4' }, status: 'drifted', desired: 15, applied: 15, compose: DEMO_COMPOSE, env: 'VERSION=2.8.4\nOTEL_EXPORTER_OTLP_ENDPOINT=http://photon:4318\n', variables: { VERSION: '2.8.4', OTEL_EXPORTER_OTLP_ENDPOINT: 'http://photon:4318' }, secrets: ['DB_PASSWORD'], apm: ['api', 'web', 'worker'], created_ago: 40 * DAY, updated_ago: 22 * MIN, applied_ago: 22 * MIN,
    drift: [
      { kind: 'image_changed', service: 'worker', field: 'image', desired: 'ghcr.io/nodeflux/worker:2.8.4', observed: 'ghcr.io/nodeflux/worker:2.8.5-hotfix', since_ms: now0 - 3 * HOUR },
      { kind: 'env_changed', service: 'worker', field: 'QUEUE_CONCURRENCY', desired: '(unset)', observed: '2', since_ms: now0 - 3 * HOUR },
    ] },
  { id: 'st_vas1', name: 'nodeflux-vas', host: 'vas-edge-01', site: 'divtik', source: 'template', template: { id: 'nodeflux-vas', name: 'Nodeflux VAS', version: '2.4.1' }, status: 'running', desired: 9, applied: 9, compose: VAS_COMPOSE, env: 'VAS_VERSION=2.4.1\nMODEL=lpr-id-v7\nAPI_PORT=8443\nENVIRONMENT=prod\nOTEL_EXPORTER_OTLP_ENDPOINT=http://photon.divtik.local:4318\n', variables: { VAS_VERSION: '2.4.1', MODEL: 'lpr-id-v7', API_PORT: '8443', ENVIRONMENT: 'prod', OTEL_EXPORTER_OTLP_ENDPOINT: 'http://photon.divtik.local:4318' }, secrets: ['LICENSE_KEY', 'DB_PASSWORD', 'JWT_SECRET'], apm: ['vas-inference', 'vas-api'], created_ago: 88 * DAY, updated_ago: 25 * HOUR, applied_ago: 25 * HOUR },
  { id: 'st_vas2', name: 'nodeflux-vas', host: 'vas-edge-02', site: 'divtik', source: 'template', template: { id: 'nodeflux-vas', name: 'Nodeflux VAS', version: '2.4.1' }, status: 'pending', desired: 13, applied: 12, compose: VAS_COMPOSE, env: 'VAS_VERSION=2.4.1\nMODEL=lpr-id-v7\nAPI_PORT=8443\nENVIRONMENT=prod\nOTEL_EXPORTER_OTLP_ENDPOINT=http://photon.divtik.local:4318\n', variables: { VAS_VERSION: '2.4.1', MODEL: 'lpr-id-v7', API_PORT: '8443', ENVIRONMENT: 'prod', OTEL_EXPORTER_OTLP_ENDPOINT: 'http://photon.divtik.local:4318' }, secrets: ['LICENSE_KEY', 'DB_PASSWORD', 'JWT_SECRET'], apm: ['vas-inference', 'vas-api'], created_ago: 88 * DAY, updated_ago: 3 * HOUR + 12 * MIN, applied_ago: 9 * DAY },
  { id: 'st_signage', name: 'kiosk-signage', host: 'vas-edge-01', site: 'divtik', source: 'compose', status: 'failed', desired: 2, applied: 1, compose: SIGNAGE_COMPOSE, env: '', variables: {}, secrets: [], apm: [], created_ago: 6 * DAY, updated_ago: 5 * HOUR, applied_ago: 6 * DAY, last_error: 'service "signage": Bind for 0.0.0.0:8443 failed: port is already allocated (vas-api holds it)', acked: ['privileged:signage', 'bind_mount_outside:signage'] },
]

// A stack stores the RENDERED compose (variables substituted, secret refs kept) — that is what
// the host runs and what a later dry-run diffs against.
function renderCompose(compose: string, variables: Record<string, string>): string {
  return compose.replace(/\$\{(\w+)\}/g, (all, name) => (variables[name] != null && variables[name] !== '' ? variables[name] : all))
}

function seedFiles(s: StackSeed): StackFile[] {
  const files: StackFile[] = [{ path: 'compose.yaml', content: renderCompose(s.compose, s.variables) }]
  if (s.env) files.push({ path: '.env', content: s.env })
  return files
}

let stacks: Stack[] = SEEDS.map((s) => ({
  id: s.id,
  name: s.name,
  host: s.host,
  site: s.site,
  source: s.source,
  template_id: s.template?.id ?? null,
  template_name: s.template?.name ?? null,
  template_version: s.template?.version ?? null,
  status: s.status,
  desired_generation: s.desired,
  applied_generation: s.applied,
  files: seedFiles(s),
  variables: s.variables,
  secrets: s.secrets,
  deploy_options: { ...DEFAULT_OPTS, ...(s.opts ?? {}) },
  services: [],
  drift: s.drift ?? [],
  apm_services: s.apm,
  acknowledged_warnings: s.acked ?? [],
  last_error: s.last_error ?? null,
  created_by: 'rina',
  created_ms: now0 - s.created_ago,
  updated_ms: now0 - s.updated_ago,
  last_applied_ms: s.applied_ago != null ? now0 - s.applied_ago : null,
}))

// In-flight applies: stack id → when it started.
const applying = new Map<string, { since: number; kind: StackActionKind; args: StackActionArgs }>()

function observedServices(st: Stack): StackServiceState[] {
  const inv = mockHostInventoryWithActions(st.host)
  const names = composeServices(st.files.find((f) => isComposeFile(f.path))?.content ?? '')
  const compose = st.files.find((f) => isComposeFile(f.path))?.content ?? ''
  return names.map((name) => {
    const cs = (inv?.containers ?? []).filter((c) => c.compose_project === st.name && c.compose_service === name)
    const m = compose.match(new RegExp(`\\n\\s+${name.replace(/[-.]/g, '\\$&')}:\\n(?:.*\\n)*?\\s+image:\\s*(\\S+)`))
    const image = m ? m[1].replace(/\$\{(\w+)\}/g, (_, v) => st.variables[v] ?? `\${${v}}`) : cs[0]?.image ?? '?'
    return { name, image, containers: cs.map((c) => ({ id: c.id, name: c.name, state: c.state, health: c.health })) }
  })
}

function settleStacks(): void {
  const now = Date.now()
  for (const [id, a] of applying) {
    const st = stacks.find((s) => s.id === id)
    if (!st) {
      applying.delete(id)
      continue
    }
    const managed = managedHosts.find((h) => h.host === st.host)
    if (managed?.status !== 'online') continue // stays pending
    if (st.status === 'pending') {
      st.status = a.kind === 'destroy' ? 'destroying' : 'applying'
      a.since = now
      continue
    }
    if (now - a.since < APPLY_MS) continue
    applying.delete(id)
    st.last_applied_ms = now
    st.updated_ms = now
    if (a.kind === 'accept') {
      // What runs becomes what is desired: a new generation rendered from the observed state.
      st.status = 'running'
      st.desired_generation += 1
      st.applied_generation = st.desired_generation
      st.drift = []
      pushAudit({ action: 'stack.accept_drift', target: { host: st.host, stack: st.name }, result: 'ok', detail: `generation ${st.desired_generation} now matches what runs` })
      continue
    }
    if (a.kind === 'destroy') {
      st.status = 'gone'
      st.services = []
      st.drift = []
      setStackContainers(st, 'removed')
      pushAudit({ action: 'stack.destroy', target: { host: st.host, stack: st.name }, result: 'ok', detail: a.args.volumes ? 'containers and volumes removed' : 'containers removed; volumes kept' })
      continue
    }
    if (st.id === 'st_signage' && a.kind !== 'down') {
      st.status = 'failed'
      st.last_error = 'service "signage": Bind for 0.0.0.0:8443 failed: port is already allocated (vas-api holds it)'
      pushAudit({ action: 'stack.apply', target: { host: st.host, stack: st.name }, result: 'failed', detail: st.last_error })
      continue
    }
    st.status = a.kind === 'down' ? 'declared' : 'running'
    st.applied_generation = st.desired_generation
    st.last_error = null
    st.drift = []
    if (a.kind === 'down') setStackContainers(st, 'exited')
    else materialise(st)
    pushAudit({ action: a.kind === 'reconcile' ? 'stack.reconcile' : a.kind === 'down' ? 'stack.apply' : 'stack.apply', target: { host: st.host, stack: st.name }, result: 'ok', detail: `generation ${st.applied_generation} · ${a.kind}${a.args.pull ? ' (pull)' : ''}`, payload_digest: 'sha256:' + shortId(st.id + st.desired_generation).slice(0, 4) + '…' + shortId(st.id + 'q').slice(0, 4) })
  }
}

// After a successful apply, every compose service has a container on the host. Services the
// fixture never had get one synthesised from the compose file, so a stack created in the demo
// shows up in the host's Containers tab like any other.
function materialise(st: Stack): void {
  const compose = st.files.find((f) => isComposeFile(f.path))?.content ?? ''
  const specs = (CONTAINER_SPECS[st.host] ??= [])
  for (const name of composeServices(compose)) {
    const cname = `${st.name}-${name}-1`
    const m = compose.match(new RegExp(`\n\s+${name.replace(/[-.]/g, '\\$&')}:\n(?:.*\n)*?\s+image:\s*(\S+)`))
    const image = (m ? m[1] : 'unknown:latest').replace(/\$\{(\w+)\}/g, (_: string, v: string) => st.variables[v] ?? `\${${v}}`)
    if (!specs.some((sp) => sp.name === cname)) specs.push({ name: cname, project: st.name, service: name, managed: st.id, image, started_ago_ms: 1_000, cpu: 3, mem: 96 * MB, mem_limit: 512 * MB, health: 'healthy', env: OTEL_ENV })
    overrides.set(st.host + '/' + cname, { state: 'running', exit_code: null, started_ms: Date.now(), finished_ms: null, removed: false })
  }
}

function setStackContainers(st: Stack, state: 'exited' | 'removed'): void {
  for (const sp of CONTAINER_SPECS[st.host] ?? []) {
    if (sp.project !== st.name) continue
    const key = st.host + '/' + sp.name
    if (state === 'removed') overrides.set(key, { ...(overrides.get(key) ?? {}), removed: true })
    else overrides.set(key, { ...(overrides.get(key) ?? {}), state: 'exited', exit_code: 0, finished_ms: Date.now(), removed: false })
  }
}

function view(st: Stack): Stack {
  return { ...st, services: st.status === 'gone' ? [] : observedServices(st), files: st.files.map((f) => ({ ...f })), drift: [...st.drift] }
}

export function mockStacks(): StacksResult {
  settleStacks()
  return { stacks: stacks.map(view) }
}

export function mockStack(id: string): Stack | null {
  settleStacks()
  const st = stacks.find((s) => s.id === id)
  return st ? view(st) : null
}

export function mockValidateCompose(files: StackFile[]): StackValidation {
  const compose = files.find((f) => isComposeFile(f.path))
  if (!compose) return { ok: false, warnings: [], services: [], missing_variables: [], error: `No compose file. Expected one of compose.yaml, compose.yml, docker-compose.yaml, docker-compose.yml.` }
  const bad = files.find((f) => !isSafePath(f.path))
  if (bad) return { ok: false, warnings: [], services: [], missing_variables: [], error: `"${bad.path}" escapes the stack folder. Paths must be relative and stay inside it.` }
  const services = composeServices(compose.content)
  if (!services.length) return { ok: false, warnings: [], services: [], missing_variables: [], error: 'No services found under `services:`.' }
  const env = parseEnv(files.find((f) => f.path === '.env')?.content ?? '')
  const refs = missingVariables(compose.content, env, []).filter((v) => !v.startsWith('secret:'))
  return { ok: true, warnings: lintCompose(compose.content), services, missing_variables: refs, error: null }
}

export function mockCreateStack(input: StackInput): StackResult {
  const managed = managedHosts.find((h) => h.host === input.host)
  if (!managed) return { ok: false, error: 'no such host' }
  if (!managed.capabilities.includes('apply')) return { ok: false, error: 'the agent on this host was installed without --docker-apply' }
  const name = input.name.trim()
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(name)) return { ok: false, error: 'stack name must be lowercase letters, digits, - or _ (it becomes the compose project name)' }
  if (stacks.some((s) => s.host === input.host && s.name === name && s.status !== 'gone')) return { ok: false, error: `${name} already exists on ${input.host}` }
  const v = mockValidateCompose(input.files)
  if (!v.ok) return { ok: false, error: v.error ?? 'invalid compose' }
  const unacked = v.warnings.filter((w) => !input.acknowledged_warnings.includes(w.id))
  if (unacked.length) return { ok: false, error: `acknowledge ${unacked.length} warning${unacked.length === 1 ? '' : 's'} before deploying` }
  const now = Date.now()
  const st: Stack = {
    id: 'st_' + shortId(input.host + name + now).slice(0, 8),
    name,
    host: input.host,
    site: managed.site,
    source: 'compose',
    template_id: null,
    template_name: null,
    template_version: null,
    status: input.apply === false ? 'rendered' : 'pending',
    desired_generation: 1,
    applied_generation: null,
    files: input.files.map((f) => ({ ...f })),
    variables: parseEnv(input.files.find((f) => f.path === '.env')?.content ?? ''),
    secrets: [],
    deploy_options: { ...input.deploy_options },
    services: [],
    drift: [],
    apm_services: [],
    acknowledged_warnings: [...input.acknowledged_warnings],
    last_error: null,
    created_by: 'demo',
    created_ms: now,
    updated_ms: now,
    last_applied_ms: null,
  }
  stacks = [st, ...stacks]
  pushAudit({ action: 'stack.create', target: { host: st.host, stack: st.name }, result: 'ok', detail: `arbitrary compose · ${v.services.length} services${input.acknowledged_warnings.length ? ` · ${input.acknowledged_warnings.length} acknowledged warnings` : ''}`, payload_digest: 'sha256:' + shortId(st.id).slice(0, 4) + '…' + shortId(st.id + 'p').slice(0, 4) })
  if (input.apply !== false) {
    applying.set(st.id, { since: now, kind: 'up', args: { pull: st.deploy_options.pull === 'always' } })
    if (managed.status !== 'online') pushAudit({ action: 'stack.apply', target: { host: st.host, stack: st.name }, result: 'pending', detail: 'host offline — will apply on next sync' })
  }
  settleStacks()
  return { ok: true, stack: view(stacks.find((s) => s.id === st.id)!) }
}

export function mockUpdateStackFiles(id: string, files: StackFile[], acknowledged: string[], deployOptions?: DeployOptions): StackResult {
  const st = stacks.find((s) => s.id === id)
  if (!st) return { ok: false, error: 'no such stack' }
  const v = mockValidateCompose(files)
  if (!v.ok) return { ok: false, error: v.error ?? 'invalid compose' }
  const unacked = v.warnings.filter((w) => !acknowledged.includes(w.id))
  if (unacked.length) return { ok: false, error: `acknowledge ${unacked.length} warning${unacked.length === 1 ? '' : 's'} before saving` }
  const changed = JSON.stringify(files) !== JSON.stringify(st.files)
  st.files = files.map((f) => ({ ...f }))
  st.variables = parseEnv(files.find((f) => f.path === '.env')?.content ?? '')
  st.acknowledged_warnings = [...acknowledged]
  if (deployOptions) st.deploy_options = { ...deployOptions }
  if (changed) {
    st.desired_generation += 1
    st.updated_ms = Date.now()
    if (st.status === 'running' || st.status === 'drifted') st.status = 'rendered'
    pushAudit({ action: 'stack.edit', target: { host: st.host, stack: st.name }, result: 'ok', detail: `generation ${st.desired_generation} rendered (not applied yet)`, payload_digest: 'sha256:' + shortId(st.id + st.desired_generation).slice(0, 4) + '…' + shortId(st.id + 'q').slice(0, 4) })
  }
  return { ok: true, stack: view(st) }
}

export function mockStackAction(id: string, kind: StackActionKind, args: StackActionArgs = {}): StackResult {
  const st = stacks.find((s) => s.id === id)
  if (!st) return { ok: false, error: 'no such stack' }
  if (st.status === 'gone') return { ok: false, error: 'this stack was destroyed' }
  const managed = managedHosts.find((h) => h.host === st.host)
  if (!managed?.capabilities.includes('apply')) return { ok: false, error: 'the agent on this host was installed without --docker-apply' }
  if (applying.has(id)) return { ok: false, error: 'an apply is already in progress' }
  if (kind === 'accept' && !st.drift.length) return { ok: false, error: 'nothing has drifted' }
  const now = Date.now()
  st.updated_ms = now
  st.status = 'pending'
  if (kind === 'redeploy') args = { ...args, pull: true }
  applying.set(id, { since: now, kind, args })
  if (managed.status !== 'online') pushAudit({ action: kind === 'reconcile' ? 'stack.reconcile' : kind === 'destroy' ? 'stack.destroy' : 'stack.apply', target: { host: st.host, stack: st.name }, result: 'pending', detail: 'host offline — will apply on next sync' })
  settleStacks()
  return { ok: true, stack: view(st) }
}

const DISCOVERABLE: Record<string, DiscoveredProject[]> = {
  'gpu-node-1': [{ host: 'gpu-node-1', name: 'ml-lab', services: ['jupyter', 'dcgm-exporter'], containers: 2, running: 2, compose_file: '/home/ml/lab/docker-compose.yml', working_dir: '/home/ml/lab' }],
  'web-1': [{ host: 'web-1', name: 'legacy-monitoring', services: ['node-exporter'], containers: 1, running: 1, compose_file: '/opt/monitoring/docker-compose.yml', working_dir: '/opt/monitoring' }],
}

export function mockDiscoverProjects(host: string): DiscoveredProjectsResult {
  const managedNames = new Set(stacks.filter((s) => s.host === host && s.status !== 'gone').map((s) => s.name))
  return { projects: (DISCOVERABLE[host] ?? []).filter((p) => !managedNames.has(p.name)) }
}

export function mockAdoptProject(host: string, project: string): StackResult {
  const p = (DISCOVERABLE[host] ?? []).find((x) => x.name === project)
  if (!p) return { ok: false, error: 'no such compose project on that host' }
  const managed = managedHosts.find((h) => h.host === host)
  if (!managed) return { ok: false, error: 'no such host' }
  // Adopting recreates nothing: reading the project is enough. Applying changes later needs `apply`.
  const now = Date.now()
  const compose = `services:\n${p.services.map((s) => `  ${s}:\n    image: ${s === 'jupyter' ? 'jupyter/pytorch-notebook:2024-08-19' : s === 'dcgm-exporter' ? 'nvcr.io/nvidia/k8s/dcgm-exporter:3.3.7-3.5.0-ubuntu22.04' : 'prom/node-exporter:v1.8.2'}\n    restart: unless-stopped\n`).join('')}`
  const st: Stack = {
    id: 'st_' + shortId(host + project).slice(0, 8),
    name: project,
    host,
    site: managed.site,
    source: 'adopted',
    template_id: null,
    template_name: null,
    template_version: null,
    status: 'running',
    desired_generation: 1,
    applied_generation: 1,
    files: [{ path: 'compose.yaml', content: compose }],
    variables: {},
    secrets: [],
    deploy_options: { ...DEFAULT_OPTS },
    services: [],
    drift: [],
    apm_services: [],
    acknowledged_warnings: [],
    last_error: null,
    created_by: 'demo',
    created_ms: now,
    updated_ms: now,
    last_applied_ms: now,
  }
  stacks = [st, ...stacks]
  adoptedProjects.set(host + '/' + project, st.id)
  pushAudit({ action: 'stack.adopt', target: { host, stack: project }, result: 'ok', detail: `${p.containers} running containers taken under management without recreating them` })
  return { ok: true, stack: view(st) }
}

// ---------------------------------------------------------------------------
// Templates: the local catalog (immutable versions), a community registry, dry runs against a
// host's facts, and deploys that produce Stacks.
// ---------------------------------------------------------------------------
import type {
  DryRunResult,
  ImportResult,
  PreflightCheck,
  RegistriesResult,
  Registry,
  RegistryCredential,
  RegistryCredentialInput,
  RegistryCredentialResult,
  RegistryCredentialsResult,
  RegistryInput,
  RegistryResult,
  Template,
  TemplateDeployInput,
  TemplateManifest,
  TemplateVersion,
  TemplateVersionInput,
  TemplateVersionResult,
  TemplatesResult,
} from '@/lib/core/api'
import { renderEnv, semverCompare } from '@/lib/deploy/templates'
import { INFRA_HOSTS } from '@/lib/core/mock'
import { parsePortainerTemplates } from '@/lib/deploy/portainer'

const OBS_ON = { otlp_endpoint: true, ingest_token: true, service_name: null, environment: true }

function manifest(m: Partial<TemplateManifest> & Pick<TemplateManifest, 'id' | 'version' | 'name'>): TemplateManifest {
  return { description: '', icon: '', categories: [], variables: [], requires: {}, ports: [], volumes: [], health: null, observability: { otlp_endpoint: false, ingest_token: false, service_name: null, environment: false }, ...m }
}

const VAS_MANIFEST = (version: string): TemplateManifest =>
  manifest({
    id: 'nodeflux-vas',
    version,
    name: 'Nodeflux VAS',
    description: 'Video analytics: inference, API, database and MQTT broker for one site. Plates, faces, people counting.',
    icon: '🎥',
    categories: ['nodeflux', 'video-analytics', 'edge'],
    variables: [
      { name: 'VAS_VERSION', label: 'VAS version', type: 'string', required: true, default: version, preset: true, description: 'Pinned by this template version.' },
      { name: 'MODEL', label: 'Model pack', type: 'select', required: true, default: 'lpr-id-v7', options: ['lpr-id-v7', 'lpr-id-v6', 'face-v3', 'crowd-v2'], group: 'Analytics' },
      { name: 'API_PORT', label: 'API port', type: 'port', required: true, default: '8443', description: 'HTTPS port the site integrates against.', group: 'Network' },
      { name: 'ENVIRONMENT', label: 'Environment', type: 'select', required: true, default: 'prod', options: ['prod', 'staging'], group: 'General' },
      { name: 'MAX_STREAMS', label: 'Max camera streams', type: 'number', required: false, default: '16', pattern: '^[0-9]{1,3}$', group: 'Analytics' },
      { name: 'FRAMES_PATH', label: 'Frame spool path', type: 'path', required: false, default: '/var/vas/frames', description: 'Bind-mounted for the inference service.', group: 'Storage' },
      { name: 'LICENSE_KEY', label: 'License key', type: 'secret', required: true, description: 'From the customer order. Stored encrypted.' },
      { name: 'DB_PASSWORD', label: 'Database password', type: 'generated', required: true, description: 'Minted once at first deploy.' },
      { name: 'JWT_SECRET', label: 'JWT signing secret', type: 'generated', required: true },
    ],
    requires: { arch: ['x86_64'], min_docker: '24.0', min_ram_bytes: 8 * GB, min_disk_bytes: 40 * GB },
    ports: [{ name: 'API', container: 8443, default_host: 8443, protocol: 'tcp' }, { name: 'MQTT', container: 1883, default_host: 1883, protocol: 'tcp' }],
    volumes: [{ name: 'models', path: '/models', description: 'Model packs (~6 GB)' }, { name: 'vasdb', path: '/var/lib/postgresql/data' }],
    health: { kind: 'container_healthy', detail: 'vas-api and vas-inference report healthy', grace_secs: 180 },
    observability: { ...OBS_ON, service_name: null },
  })

const DEMO_MANIFEST = (version: string): TemplateManifest =>
  manifest({
    id: 'photon-demo',
    version,
    name: 'Photon demo app',
    description: 'The reference app: API, web, worker, Postgres and Redis, fully instrumented.',
    icon: '🧪',
    categories: ['nodeflux', 'demo'],
    variables: [
      { name: 'VERSION', label: 'App version', type: 'string', required: true, default: version, preset: true },
      { name: 'DB_PASSWORD', label: 'Database password', type: 'generated', required: true },
    ],
    requires: { arch: ['x86_64', 'aarch64'], min_docker: '24.0', min_ram_bytes: 2 * GB, min_disk_bytes: 5 * GB },
    ports: [{ name: 'HTTP', container: 80, default_host: 80, protocol: 'tcp' }, { name: 'HTTPS', container: 443, default_host: 443, protocol: 'tcp' }, { name: 'API', container: 8080, default_host: 8080, protocol: 'tcp' }],
    volumes: [{ name: 'pgdata', path: '/var/lib/postgresql/data' }, { name: 'api_data', path: '/var/lib/api' }],
    health: { kind: 'slo_burn', detail: 'api availability objective, 10 minutes', grace_secs: 600 },
    observability: { ...OBS_ON },
  })

const SIGNAGE_MANIFEST = (version: string): TemplateManifest =>
  manifest({
    id: 'kiosk-signage',
    version,
    name: 'Kiosk signage',
    description: 'Lobby signage player with GPU video decode.',
    icon: '🖥️',
    categories: ['nodeflux', 'edge'],
    variables: [
      { name: 'PLAYLIST_URL', label: 'Playlist URL', type: 'string', required: true, pattern: '^https?://' },
      { name: 'PORT', label: 'Admin port', type: 'port', required: true, default: '8443' },
    ],
    requires: { arch: ['x86_64'], min_docker: '24.0' },
    ports: [{ name: 'Admin', container: 8443, default_host: 8443, protocol: 'tcp' }],
    health: { kind: 'uptime_monitor', detail: 'HTTP 200 on /healthz', grace_secs: 120 },
    observability: { otlp_endpoint: true, ingest_token: true, service_name: 'kiosk-signage', environment: false },
  })

const UPTIME_KUMA_MANIFEST = manifest({
  id: 'uptime-kuma',
  version: '1.0.0',
  name: 'Uptime Kuma',
  description: 'A fancy self-hosted monitoring tool.',
  icon: 'https://raw.githubusercontent.com/portainer/portainer-icons/master/uptime-kuma.png',
  categories: ['monitoring', 'tools'],
  variables: [{ name: 'PORT', label: 'Port', type: 'port', required: true, default: '3001' }],
  requires: {},
  ports: [{ name: 'Web', container: 3001, default_host: 3001, protocol: 'tcp' }],
})
const NGINX_MANIFEST = manifest({ id: 'nginx', version: '1.0.0', name: 'Nginx', description: 'High performance web server.', icon: '', categories: ['webserver'], variables: [], requires: {}, ports: [{ name: 'HTTP', container: 80, default_host: 80, protocol: 'tcp' }] })

let registries: Registry[] = [
  { id: 'reg_portainer', name: 'Portainer community', url: 'https://raw.githubusercontent.com/portainer/templates/v3/templates.json', kind: 'portainer_v3', last_sync_ms: Date.now() - 6 * DAY, template_count: 2, status: 'ok', error: null },
]

interface TemplateRow {
  t: Template
  versions: TemplateVersion[]
}
const now1 = Date.now()
function tv(template_id: string, m: TemplateManifest, compose: string, created_ago: number, by: string, notes: string | null, deployed: number): TemplateVersion {
  return { id: `tv_${template_id}_${m.version.replace(/\./g, '_')}`, template_id, version: m.version, manifest: m, compose_text: compose, created_ms: now1 - created_ago, created_by: by, notes, deployed_count: deployed }
}
let catalog: TemplateRow[] = [
  {
    t: { id: 'nodeflux-vas', name: 'Nodeflux VAS', description: VAS_MANIFEST('2.4.1').description, icon: '🎥', categories: ['nodeflux', 'video-analytics', 'edge'], source: 'local', registry_id: null, latest_version: '2.4.2', versions: [], updated_ms: now1 - 40 * MIN, stacks: 2 },
    versions: [
      tv('nodeflux-vas', VAS_MANIFEST('2.4.2'), VAS_COMPOSE.replace('deploy:\n      resources:\n        limits:\n          memory: 4g\n', 'deploy:\n      resources:\n        limits:\n          memory: 6g\n    healthcheck:\n      test: ["CMD", "curl", "-f", "http://localhost:9090/healthz"]\n      interval: 30s\n'), 40 * MIN, 'demo', 'Inference memory limit 4g → 6g; explicit healthcheck for the health gate.', 0),
      tv('nodeflux-vas', VAS_MANIFEST('2.4.1'), VAS_COMPOSE, 5 * HOUR, 'demo', 'LPR model v7; MQTT exposed for site integrations.', 1),
      tv('nodeflux-vas', VAS_MANIFEST('2.4.0'), VAS_COMPOSE, 9 * DAY, 'demo', 'Rolled back at divtik — decoder regression.', 0),
      tv('nodeflux-vas', VAS_MANIFEST('2.3.7'), VAS_COMPOSE.replace('deploy:\n      resources:\n        limits:\n          memory: 4g\n', ''), 41 * DAY, 'rina', null, 1),
    ],
  },
  {
    t: { id: 'photon-demo', name: 'Photon demo app', description: DEMO_MANIFEST('2.8.4').description, icon: '🧪', categories: ['nodeflux', 'demo'], source: 'local', registry_id: null, latest_version: '2.8.4', versions: [], updated_ms: now1 - 3 * DAY, stacks: 2 },
    versions: [tv('photon-demo', DEMO_MANIFEST('2.8.4'), DEMO_COMPOSE, 3 * DAY, 'demo', null, 2), tv('photon-demo', DEMO_MANIFEST('2.8.3'), DEMO_COMPOSE, 20 * DAY, 'demo', null, 0)],
  },
  {
    t: { id: 'kiosk-signage', name: 'Kiosk signage', description: SIGNAGE_MANIFEST('1.2.0').description, icon: '🖥️', categories: ['nodeflux', 'edge'], source: 'local', registry_id: null, latest_version: '1.2.0', versions: [], updated_ms: now1 - 15 * DAY, stacks: 1 },
    versions: [tv('kiosk-signage', SIGNAGE_MANIFEST('1.2.0'), SIGNAGE_COMPOSE.replace('privileged: true\n    ', '').replace('8443:8443', '${PORT}:8443'), 15 * DAY, 'demo', 'GPU decode via /dev/dri.', 1)],
  },
  {
    t: { id: 'uptime-kuma', name: 'Uptime Kuma', description: UPTIME_KUMA_MANIFEST.description, icon: UPTIME_KUMA_MANIFEST.icon, categories: ['monitoring', 'tools'], source: 'portainer', registry_id: 'reg_portainer', latest_version: '1.0.0', versions: [], updated_ms: now1 - 6 * DAY, stacks: 0 },
    versions: [tv('uptime-kuma', UPTIME_KUMA_MANIFEST, 'services:\n  uptime-kuma:\n    image: louislam/uptime-kuma:1\n    restart: unless-stopped\n    ports:\n      - "${PORT}:3001"\n    volumes:\n      - uptime-kuma:/app/data\nvolumes:\n  uptime-kuma:\n', 6 * DAY, 'registry', 'Imported from Portainer community (v3).', 0)],
  },
  {
    t: { id: 'nginx', name: 'Nginx', description: NGINX_MANIFEST.description, icon: '', categories: ['webserver'], source: 'portainer', registry_id: 'reg_portainer', latest_version: '1.0.0', versions: [], updated_ms: now1 - 6 * DAY, stacks: 0 },
    versions: [tv('nginx', NGINX_MANIFEST, 'services:\n  nginx:\n    image: nginx:latest\n    restart: unless-stopped\n    ports:\n      - "80:80"\n', 6 * DAY, 'registry', 'Imported from Portainer community (v3).', 0)],
  },
]

function refresh(row: TemplateRow): Template {
  row.versions.sort((a, b) => semverCompare(b.version, a.version))
  row.t.versions = row.versions.map(({ id, version, created_ms, created_by, notes, deployed_count }) => ({ id, version, created_ms, created_by, notes, deployed_count }))
  row.t.latest_version = row.versions[0]?.version ?? row.t.latest_version
  row.t.stacks = stacks.filter((s) => s.template_id === row.t.id && s.status !== 'gone').length
  for (const v of row.versions) v.deployed_count = stacks.filter((s) => s.template_id === row.t.id && s.template_version === v.version && s.status !== 'gone').length
  return { ...row.t, versions: [...row.t.versions] }
}

export function mockTemplates(): TemplatesResult {
  return { templates: catalog.map(refresh).sort((a, b) => (a.source === 'local' ? 0 : 1) - (b.source === 'local' ? 0 : 1) || b.updated_ms - a.updated_ms) }
}
export function mockTemplate(id: string): Template | null {
  const row = catalog.find((r) => r.t.id === id)
  return row ? refresh(row) : null
}
export function mockTemplateVersion(id: string): TemplateVersion | null {
  for (const row of catalog) {
    const v = row.versions.find((x) => x.id === id)
    if (v) return { ...v, manifest: JSON.parse(JSON.stringify(v.manifest)) }
  }
  return null
}

export function mockPublishTemplateVersion(input: TemplateVersionInput): TemplateVersionResult {
  const m = input.manifest
  if (!m.id || !/^[a-z0-9][a-z0-9-]*$/.test(m.id)) return { ok: false, error: 'template id must be lowercase letters, digits and hyphens' }
  if (!m.version.trim()) return { ok: false, error: 'version is required' }
  if (!m.name.trim()) return { ok: false, error: 'name is required' }
  if (!composeServices(input.compose_text).length) return { ok: false, error: 'the compose file has no services' }
  let row = catalog.find((r) => r.t.id === (input.template_id ?? m.id))
  if (row && row.versions.some((v) => v.version === m.version)) return { ok: false, error: `version ${m.version} already exists — versions are immutable, publish a new one` }
  if (!row) {
    row = { t: { id: m.id, name: m.name, description: m.description, icon: m.icon, categories: m.categories, source: 'local', registry_id: null, latest_version: m.version, versions: [], updated_ms: Date.now(), stacks: 0 }, versions: [] }
    catalog = [row, ...catalog]
  }
  const v = tv(row.t.id, JSON.parse(JSON.stringify(m)), input.compose_text, 0, 'demo', input.notes ?? null, 0)
  row.versions = [v, ...row.versions]
  row.t.name = m.name
  row.t.description = m.description
  row.t.icon = m.icon
  row.t.categories = m.categories
  row.t.updated_ms = Date.now()
  pushAudit({ action: 'template.publish', target: { template: row.t.id, version: m.version }, result: 'ok', detail: `compose ${(input.compose_text.length / 1024).toFixed(1)} KB · ${m.variables.length} variables` })
  return { ok: true, version: v, template: refresh(row) }
}

export function mockRegistries(): RegistriesResult {
  return { registries: registries.map((r) => ({ ...r, template_count: catalog.filter((c) => c.t.registry_id === r.id).length })) }
}
export function mockAddRegistry(input: RegistryInput): RegistryResult {
  if (!input.name.trim() || !/^https?:\/\//.test(input.url)) return { ok: false, error: 'a name and an http(s) URL are required' }
  const r: Registry = { id: 'reg_' + shortId(input.url).slice(0, 6), name: input.name.trim(), url: input.url.trim(), kind: input.kind, last_sync_ms: null, template_count: 0, status: 'syncing', error: null }
  registries = [...registries, r]
  pushAudit({ action: 'template.import', target: { registry: r.name }, result: 'pending', detail: `added ${r.kind} registry, first sync queued` })
  setTimeout(() => {
    r.status = 'ok'
    r.last_sync_ms = Date.now()
  }, 1500)
  return { ok: true, registry: { ...r } }
}
export function mockSyncRegistry(id: string): RegistryResult {
  const r = registries.find((x) => x.id === id)
  if (!r) return { ok: false, error: 'no such registry' }
  r.status = 'syncing'
  setTimeout(() => {
    r.status = 'ok'
    r.last_sync_ms = Date.now()
  }, 1500)
  pushAudit({ action: 'template.import', target: { registry: r.name }, result: 'ok', detail: 'sync requested' })
  return { ok: true, registry: { ...r } }
}
export function mockDeleteRegistry(id: string): MutationResult {
  if (!registries.some((x) => x.id === id)) return { ok: false, error: 'no such registry' }
  registries = registries.filter((x) => x.id !== id)
  catalog = catalog.filter((c) => c.t.registry_id !== id || c.t.stacks > 0)
  return { ok: true }
}

let imageRegistries: RegistryCredential[] = [
  { id: 'rc_gitlab', server: 'registry.gitlab.com', username: 'nodeflux-deploy', label: 'GitLab deploy token (nodefluxio group)', scope: null, created_ms: Date.now() - 20 * DAY, created_by: 'rina', updated_ms: Date.now() - 20 * DAY },
]
export function mockImageRegistries(): RegistryCredentialsResult {
  return { credentials: imageRegistries.map((c) => ({ ...c })) }
}
function normalizeServer(s: string): string {
  const host = s.trim().replace(/^https?:\/\//, '').split('/')[0].toLowerCase()
  return ['index.docker.io', 'registry-1.docker.io', 'hub.docker.com', 'registry.hub.docker.com'].includes(host) ? 'docker.io' : host
}
export function mockAddImageRegistry(input: RegistryCredentialInput): RegistryCredentialResult {
  const server = normalizeServer(input.server ?? '')
  if (!server) return { ok: false, error: 'a registry is required — the host an image reference names, like registry.gitlab.com' }
  if (!input.username?.trim()) return { ok: false, error: 'a username is required' }
  if (!input.secret?.trim()) return { ok: false, error: 'a token or password is required' }
  const now = Date.now()
  const c: RegistryCredential = { id: 'rc_' + shortId(server + input.username + now).slice(0, 6), server, username: input.username.trim(), label: input.label?.trim() || null, scope: input.scope ?? null, created_ms: now, created_by: 'you', updated_ms: now }
  imageRegistries = [...imageRegistries, c]
  pushAudit({ action: 'registry_credential.create', target: { registry: server, user: c.username }, result: 'ok', detail: `scope: ${c.scope ? c.scope.kind : 'every host'}` })
  return { ok: true, credential: { ...c } }
}
export function mockUpdateImageRegistry(id: string, input: RegistryCredentialInput): RegistryCredentialResult {
  const c = imageRegistries.find((x) => x.id === id)
  if (!c) return { ok: false, error: 'no such registry credential' }
  if (input.server !== undefined) c.server = normalizeServer(input.server)
  if (input.username !== undefined) c.username = input.username.trim()
  if (input.label !== undefined) c.label = input.label?.trim() || null
  if (input.scope !== undefined) c.scope = input.scope
  c.updated_ms = Date.now()
  pushAudit({ action: 'registry_credential.update', target: { registry: c.server, user: c.username }, result: 'ok', detail: input.secret ? 'changed token' : 'changed details' })
  return { ok: true, credential: { ...c } }
}
export function mockDeleteImageRegistry(id: string): MutationResult {
  const c = imageRegistries.find((x) => x.id === id)
  if (!c) return { ok: false, error: 'no such registry credential' }
  imageRegistries = imageRegistries.filter((x) => x.id !== id)
  pushAudit({ action: 'registry_credential.delete', target: { registry: c.server, user: c.username }, result: 'ok' })
  return { ok: true }
}

export function mockImportPortainer(json: string, sourceName: string): ImportResult {
  let report
  try {
    report = parsePortainerTemplates(json, sourceName)
  } catch (e) {
    return { ok: false, error: (e as Error).message }
  }
  let imported = 0
  for (const t of report.imported) {
    if (catalog.some((c) => c.t.id === t.manifest.id)) continue
    const row: TemplateRow = { t: { id: t.manifest.id, name: t.manifest.name, description: t.manifest.description, icon: t.manifest.icon, categories: t.manifest.categories, source: 'portainer', registry_id: null, latest_version: t.manifest.version, versions: [], updated_ms: Date.now(), stacks: 0 }, versions: [] }
    row.versions = [tv(row.t.id, t.manifest, t.compose_text, 0, 'import', `Imported from ${sourceName} (Portainer v${report.version}).`, 0)]
    catalog = [...catalog, row]
    imported++
  }
  pushAudit({ action: 'template.import', target: { registry: sourceName }, result: 'ok', detail: `${imported} templates (portainer v${report.version}), ${report.skipped.length} skipped` })
  return { ok: true, imported, skipped: report.skipped.length }
}

function cmpVersion(a: string, b: string): number {
  return semverCompare(a, b)
}

export function mockDryRun(input: TemplateDeployInput): DryRunResult {
  const v = mockTemplateVersion(input.template_version_id)
  const fail = (error: string): DryRunResult => ({ ok: false, host: input.host, checks: [], compose: '', env: '', previous_compose: null, warnings: [], secrets: [], generated: [], error })
  if (!v) return fail('no such template version')
  const managed = managedHosts.find((h) => h.host === input.host)
  if (!managed) return fail('no such host')
  const m = v.manifest
  const checks: PreflightCheck[] = []
  const facts = managed.docker
  // agent
  checks.push(managed.capabilities.includes('apply') ? { key: 'capability', label: 'Agent can apply stacks', status: 'pass', detail: '--docker-apply granted' } : { key: 'capability', label: 'Agent can apply stacks', status: 'fail', detail: 'installed without --docker-apply; reinstall with it' })
  checks.push(managed.status === 'online' ? { key: 'online', label: 'Host is reachable', status: 'pass', detail: `synced ${Math.round((Date.now() - (managed.last_seen_ms ?? 0)) / 1000)}s ago` } : { key: 'online', label: 'Host is reachable', status: 'warn', detail: `${AGENT_STATUS_LABEL[managed.status]} — the deploy waits as pending and applies on the next sync` })
  // arch
  if (m.requires.arch?.length) checks.push(!facts ? { key: 'arch', label: 'CPU architecture', status: 'skip', detail: 'unknown until the first sync' } : m.requires.arch.includes(facts.arch) ? { key: 'arch', label: 'CPU architecture', status: 'pass', detail: facts.arch } : { key: 'arch', label: 'CPU architecture', status: 'fail', detail: `host is ${facts.arch}; template needs ${m.requires.arch.join(' or ')}` })
  // docker
  if (m.requires.min_docker) checks.push(!facts ? { key: 'docker', label: 'Docker version', status: 'skip', detail: 'unknown until the first sync' } : cmpVersion(facts.version, m.requires.min_docker) >= 0 ? { key: 'docker', label: 'Docker version', status: 'pass', detail: `${facts.version} ≥ ${m.requires.min_docker}` } : { key: 'docker', label: 'Docker version', status: 'fail', detail: `${facts.version} < ${m.requires.min_docker}` })
  // ram / disk
  const infra = INFRA_HOSTS.find((h) => h.host === input.host)
  if (m.requires.min_ram_bytes) checks.push(!infra ? { key: 'ram', label: 'Memory', status: 'skip', detail: 'no resource metrics from this host' } : infra.totalRamBytes >= m.requires.min_ram_bytes ? { key: 'ram', label: 'Memory', status: 'pass', detail: `${(infra.totalRamBytes / GB).toFixed(0)} GB ≥ ${(m.requires.min_ram_bytes / GB).toFixed(0)} GB` } : { key: 'ram', label: 'Memory', status: 'fail', detail: `${(infra.totalRamBytes / GB).toFixed(0)} GB < ${(m.requires.min_ram_bytes / GB).toFixed(0)} GB required` })
  if (m.requires.min_disk_bytes) {
    const free = facts?.disk.total_bytes != null && facts.disk.used_bytes != null ? facts.disk.total_bytes - facts.disk.used_bytes : null
    checks.push(free == null ? { key: 'disk', label: 'Free disk', status: 'skip', detail: 'unknown until the first sync' } : free >= m.requires.min_disk_bytes ? { key: 'disk', label: 'Free disk', status: 'pass', detail: `${(free / GB).toFixed(0)} GB free ≥ ${(m.requires.min_disk_bytes / GB).toFixed(0)} GB` } : free >= m.requires.min_disk_bytes * 0.5 ? { key: 'disk', label: 'Free disk', status: 'warn', detail: `${(free / GB).toFixed(0)} GB free — under the ${(m.requires.min_disk_bytes / GB).toFixed(0)} GB the template asks for; a prune frees ${(facts!.disk.reclaimable_bytes / GB).toFixed(1)} GB` } : { key: 'disk', label: 'Free disk', status: 'fail', detail: `${(free / GB).toFixed(0)} GB free < ${(m.requires.min_disk_bytes / GB).toFixed(0)} GB required` })
  }
  // ports
  const inv = mockHostInventoryWithActions(input.host)
  for (const p of m.ports) {
    const wanted = Number(input.variables[m.variables.find((x) => x.type === 'port' && Number(x.default) === p.default_host)?.name ?? ''] || p.default_host)
    const holder = inv?.containers.find((c) => c.state === 'running' && c.ports.some((cp) => cp.host_port === wanted) && !(input.stack_id && c.managed_stack_id === input.stack_id) && c.compose_project !== input.name)
    checks.push(holder ? { key: `port-${p.name}`, label: `Port ${wanted} (${p.name})`, status: 'fail', detail: `already published by ${holder.name}` } : { key: `port-${p.name}`, label: `Port ${wanted} (${p.name})`, status: 'pass', detail: 'free' })
  }
  checks.push({ key: 'images', label: 'Images reachable', status: 'skip', detail: 'checked by the agent at apply (pull policy: ' + input.deploy_options.pull + ')' })
  const secrets = m.variables.filter((x) => x.type === 'secret').map((x) => x.name)
  const generated = m.variables.filter((x) => x.type === 'generated').map((x) => x.name)
  const prev = input.stack_id ? stacks.find((s) => s.id === input.stack_id) : stacks.find((s) => s.host === input.host && s.name === input.name && s.status !== 'gone')
  // The Photon block is part of the render on BOTH sides of a diff: the endpoint a host already
  // uses is kept, a new host gets this Photon's.
  const otlp = prev?.variables.OTEL_EXPORTER_OTLP_ENDPOINT ?? 'http://photon:4318'
  const values: Record<string, string> = { ...input.variables }
  if (m.observability.otlp_endpoint) values.OTEL_EXPORTER_OTLP_ENDPOINT = otlp
  const compose = v.compose_text.replace(/\$\{(\w+)\}/g, (all, name) => (values[name] != null && values[name] !== '' ? values[name] : all))
  const env = renderEnv(m, values, { otlpEndpoint: otlp, environment: values.ENVIRONMENT ?? null })
  const warnings = lintCompose(compose)
  const ok = !checks.some((c) => c.status === 'fail')
  return { ok, host: input.host, checks, compose, env, previous_compose: prev ? (prev.files.find((f) => isComposeFile(f.path))?.content ?? null) : null, warnings, secrets, generated, error: null }
}
const AGENT_STATUS_LABEL: Record<string, string> = { online: 'online', offline: 'offline', pending: 'waiting for first sync' }

export function mockDeployTemplate(input: TemplateDeployInput): StackResult {
  const dry = mockDryRun(input)
  if (dry.error) return { ok: false, error: dry.error }
  if (!dry.ok) return { ok: false, error: 'preflight failed: ' + dry.checks.filter((c) => c.status === 'fail').map((c) => c.detail).join('; ') }
  const unacked = dry.warnings.filter((w) => !input.acknowledged_warnings.includes(w.id))
  if (unacked.length) return { ok: false, error: `acknowledge ${unacked.length} warning${unacked.length === 1 ? '' : 's'} first` }
  const v = mockTemplateVersion(input.template_version_id)!
  const row = catalog.find((r) => r.t.id === v.template_id)!
  const managed = managedHosts.find((h) => h.host === input.host)!
  const now = Date.now()
  const existing = input.stack_id ? stacks.find((s) => s.id === input.stack_id) : stacks.find((s) => s.host === input.host && s.name === input.name && s.status !== 'gone')
  const files = [{ path: 'compose.yaml', content: dry.compose }, { path: '.env', content: dry.env }]
  const variables: Record<string, string> = {}
  for (const x of v.manifest.variables) if (x.type !== 'secret' && x.type !== 'generated') variables[x.name] = input.variables[x.name] ?? x.default ?? ''
  if (v.manifest.observability.otlp_endpoint) variables.OTEL_EXPORTER_OTLP_ENDPOINT = existing?.variables.OTEL_EXPORTER_OTLP_ENDPOINT ?? 'http://photon:4318'
  if (existing) {
    const from = existing.template_version
    existing.files = files
    existing.variables = variables
    existing.secrets = [...dry.secrets, ...dry.generated]
    existing.template_id = row.t.id
    existing.template_name = row.t.name
    existing.template_version = v.version
    existing.source = 'template'
    existing.desired_generation += 1
    existing.deploy_options = { ...input.deploy_options }
    existing.acknowledged_warnings = [...input.acknowledged_warnings]
    existing.updated_ms = now
    existing.status = 'pending'
    existing.drift = []
    applying.set(existing.id, { since: now, kind: 'up', args: { pull: true } })
    pushAudit({ action: 'stack.apply', target: { host: existing.host, stack: existing.name, template: row.t.id, version: v.version }, result: managed.status === 'online' ? 'ok' : 'pending', detail: `${from ?? '?'} → ${v.version} · generation ${existing.desired_generation}`, payload_digest: 'sha256:' + shortId(existing.id + existing.desired_generation).slice(0, 4) + '…' + shortId(v.id).slice(0, 4) })
    settleStacks()
    return { ok: true, stack: view(existing) }
  }
  const st: Stack = {
    id: 'st_' + shortId(input.host + input.name + now).slice(0, 8),
    name: input.name,
    host: input.host,
    site: managed.site,
    source: 'template',
    template_id: row.t.id,
    template_name: row.t.name,
    template_version: v.version,
    status: 'pending',
    desired_generation: 1,
    applied_generation: null,
    files,
    variables,
    secrets: [...dry.secrets, ...dry.generated],
    deploy_options: { ...input.deploy_options },
    services: [],
    drift: [],
    apm_services: composeServices(v.compose_text).filter((s) => v.compose_text.includes(`OTEL_SERVICE_NAME: ${s}`)),
    acknowledged_warnings: [...input.acknowledged_warnings],
    last_error: null,
    created_by: 'demo',
    created_ms: now,
    updated_ms: now,
    last_applied_ms: null,
  }
  stacks = [st, ...stacks]
  applying.set(st.id, { since: now, kind: 'up', args: { pull: true } })
  pushAudit({ action: 'stack.create', target: { host: st.host, stack: st.name, template: row.t.id, version: v.version }, result: 'ok', detail: `from template ${row.t.name} ${v.version}${dry.generated.length ? ` · minted ${dry.generated.join(', ')}` : ''}`, payload_digest: 'sha256:' + shortId(st.id).slice(0, 4) + '…' + shortId(v.id).slice(0, 4) })
  if (managed.status !== 'online') pushAudit({ action: 'stack.apply', target: { host: st.host, stack: st.name }, result: 'pending', detail: 'host offline — will apply on next sync' })
  settleStacks()
  return { ok: true, stack: view(stacks.find((s) => s.id === st.id)!) }
}

// ---------------------------------------------------------------------------
// Rollouts. A time-driven supervisor: each tick, stage targets move pending → prepulling →
// applying → healthy / failed; an offline host's target stays pending; a stage whose failures
// cross the threshold pauses; a finished canary stage pauses at the gate. Failures are
// deterministic — a host whose name contains "fail" fails its apply — so the demo can show the
// threshold path.
// ---------------------------------------------------------------------------
import type { Rollout, RolloutActionKind, RolloutInput, RolloutResult, RolloutsResult, RolloutTarget } from '@/lib/core/api'
import { resolveTargets, planStages, targetCounts, stageCount } from '@/lib/deploy/rollouts'

const PREPULL_MS = 2_200
const APPLY_MS_R = 2_800

interface TargetClock {
  since: number
}
const clocks = new Map<string, TargetClock>() // rolloutId/host → when the current status began

let rollouts: Rollout[] = []

function seedRollouts(): void {
  const now = Date.now()
  const t = (host: string, site: string | null, stage: number, status: RolloutTarget['status'], over: Partial<RolloutTarget> = {}): RolloutTarget => ({
    host,
    site,
    stage,
    status,
    attempts: status === 'pending' ? 0 : 1,
    last_error: null,
    stack_id: stacks.find((s) => s.host === host && s.template_id === 'nodeflux-vas')?.id ?? null,
    previous_version: '2.3.7',
    agent_status: managedHosts.find((h) => h.host === host)?.status ?? 'pending',
    updated_ms: now - 25 * HOUR,
    ...over,
  })
  rollouts = [
    {
      id: 'ro_vas_241',
      name: 'Nodeflux VAS 2.4.1 → site divtik',
      template_id: 'nodeflux-vas',
      template_name: 'Nodeflux VAS',
      template_version_id: 'tv_nodeflux-vas_2_4_1',
      template_version: '2.4.1',
      previous_version: '2.3.7',
      stack_name: 'nodeflux-vas',
      selector: { kind: 'site', sites: ['divtik'] },
      strategy: { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 300 },
      state: 'running',
      stage: 2,
      stages: 2,
      targets: [t('vas-edge-01', 'divtik', 1, 'healthy'), t('vas-edge-02', 'divtik', 2, 'pending', { updated_ms: now - 3 * HOUR - 12 * MIN })],
      variables: { MODEL: 'lpr-id-v7', API_PORT: '8443', ENVIRONMENT: 'prod', MAX_STREAMS: '16', FRAMES_PATH: '/var/vas/frames' },
      created_by: 'rina',
      created_ms: now - 27 * HOUR,
      updated_ms: now - 3 * HOUR - 12 * MIN,
      started_ms: now - 27 * HOUR,
      finished_ms: null,
    },
    {
      id: 'ro_demo_284',
      name: 'Photon demo app 2.8.4 → 2 hosts',
      template_id: 'photon-demo',
      template_name: 'Photon demo app',
      template_version_id: 'tv_photon-demo_2_8_4',
      template_version: '2.8.4',
      previous_version: '2.8.3',
      stack_name: 'photon-demo',
      selector: { kind: 'hosts', hosts: ['web-1', 'web-2'] },
      strategy: { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 600 },
      state: 'paused_gate',
      stage: 1,
      stages: 2,
      targets: [
        { host: 'web-1', site: null, stage: 1, status: 'healthy', attempts: 1, last_error: null, stack_id: 'st_web1_demo', previous_version: '2.8.3', agent_status: 'online', updated_ms: now - 3 * DAY - 4 * HOUR },
        { host: 'web-2', site: null, stage: 2, status: 'queued', attempts: 0, last_error: null, stack_id: 'st_web2_demo', previous_version: '2.8.3', agent_status: 'online', updated_ms: now - 3 * DAY - 4 * HOUR },
      ],
      variables: {},
      created_by: 'rina',
      created_ms: now - 3 * DAY - 5 * HOUR,
      updated_ms: now - 3 * DAY - 4 * HOUR,
      started_ms: now - 3 * DAY - 5 * HOUR,
      finished_ms: null,
    },
    {
      id: 'ro_vas_240',
      name: 'Nodeflux VAS 2.4.0 → site divtik',
      template_id: 'nodeflux-vas',
      template_name: 'Nodeflux VAS',
      template_version_id: 'tv_nodeflux-vas_2_4_0',
      template_version: '2.4.0',
      previous_version: '2.3.7',
      stack_name: 'nodeflux-vas',
      selector: { kind: 'site', sites: ['divtik'] },
      strategy: { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 300 },
      state: 'rolled_back',
      stage: 2,
      stages: 2,
      targets: [
        { host: 'vas-edge-01', site: 'divtik', stage: 1, status: 'healthy', attempts: 1, last_error: null, stack_id: 'st_vas1', previous_version: '2.3.7', agent_status: 'online', updated_ms: now - 8 * DAY },
        { host: 'vas-edge-02', site: 'divtik', stage: 2, status: 'failed', attempts: 2, last_error: 'health gate: vas-inference unhealthy after 300s (decoder init failed: CUDA_ERROR_NO_DEVICE)', stack_id: 'st_vas2', previous_version: '2.3.7', agent_status: 'offline', updated_ms: now - 8 * DAY },
      ],
      variables: { MODEL: 'lpr-id-v7', API_PORT: '8443', ENVIRONMENT: 'prod' },
      created_by: 'rina',
      created_ms: now - 9 * DAY,
      updated_ms: now - 8 * DAY,
      started_ms: now - 9 * DAY,
      finished_ms: now - 8 * DAY,
    },
  ]
}
seedRollouts()

function tick(): void {
  const now = Date.now()
  for (const r of rollouts) {
    if (r.state !== 'running') continue
    const stageTargets = r.targets.filter((t) => t.stage === r.stage)
    for (const t of stageTargets) {
      const managed = managedHosts.find((h) => h.host === t.host)
      t.agent_status = managed?.status ?? 'pending'
      const key = r.id + '/' + t.host
      if (t.status === 'pending') {
        if (managed?.status !== 'online') continue // waits for the host
        t.status = r.strategy.prepull ? 'prepulling' : 'applying'
        t.attempts += 1
        t.updated_ms = now
        clocks.set(key, { since: now })
        continue
      }
      const clock = clocks.get(key)
      if (!clock) continue
      if (t.status === 'prepulling' && now - clock.since >= PREPULL_MS) {
        t.status = 'applying'
        t.updated_ms = now
        clocks.set(key, { since: now })
        continue
      }
      if (t.status === 'applying' && now - clock.since >= APPLY_MS_R) {
        clocks.delete(key)
        t.updated_ms = now
        if (/fail/i.test(t.host)) {
          t.status = 'failed'
          t.last_error = 'health gate: containers not healthy after grace period'
          pushAudit({ action: 'stack.apply', target: { host: t.host, stack: r.stack_name, rollout: r.id }, result: 'failed', detail: t.last_error })
        } else {
          t.status = 'healthy'
          t.last_error = null
          // The target's stack now runs the rollout's version.
          const st = stacks.find((s) => s.host === t.host && s.name === r.stack_name && s.status !== 'gone')
          if (st) {
            st.template_id = r.template_id
            st.template_name = r.template_name
            st.template_version = r.template_version
            st.source = 'template'
            st.desired_generation += 1
            st.applied_generation = st.desired_generation
            st.status = 'running'
            st.last_applied_ms = now
            st.updated_ms = now
            st.drift = []
            t.stack_id = st.id
          }
          pushAudit({ action: 'stack.apply', target: { host: t.host, stack: r.stack_name, rollout: r.id }, result: 'ok', detail: `${t.previous_version ?? '?'} → ${r.template_version} (rollout stage ${t.stage})` })
        }
      }
    }
    const c = targetCounts(stageTargets)
    if (c.failed >= r.strategy.failure_threshold) {
      r.state = 'paused_threshold'
      r.updated_ms = now
      pushAudit({ action: 'rollout.pause', target: { rollout: r.id }, result: 'ok', detail: `threshold crossed: ${c.failed} failed in stage ${r.stage}` })
      continue
    }
    const settled = stageTargets.every((t) => t.status === 'healthy' || t.status === 'failed' || t.status === 'skipped')
    if (settled && stageTargets.length) {
      if (r.stage < r.stages) {
        if (r.strategy.auto_promote) {
          r.stage += 1
          for (const t of r.targets) if (t.stage === r.stage && t.status === 'queued') t.status = 'pending'
          pushAudit({ action: 'rollout.promote', target: { rollout: r.id }, result: 'ok', detail: `auto-promoted to stage ${r.stage}` })
        } else {
          r.state = 'paused_gate'
          pushAudit({ action: 'rollout.pause', target: { rollout: r.id }, result: 'ok', detail: `stage ${r.stage} done — waiting at the gate` })
        }
      } else {
        r.state = 'completed'
        r.finished_ms = now
        pushAudit({ action: 'rollout.complete', target: { rollout: r.id }, result: 'ok', detail: `${targetCounts(r.targets).healthy} healthy, ${targetCounts(r.targets).failed} failed, ${targetCounts(r.targets).skipped} skipped` })
      }
      r.updated_ms = now
    }
  }
}

function viewRollout(r: Rollout): Rollout {
  return JSON.parse(JSON.stringify(r)) as Rollout
}

export function mockRollouts(): RolloutsResult {
  tick()
  return { rollouts: rollouts.map(viewRollout) }
}
export function mockRollout(id: string): Rollout | null {
  tick()
  const r = rollouts.find((x) => x.id === id)
  return r ? viewRollout(r) : null
}

export function mockCreateRollout(input: RolloutInput): RolloutResult {
  const v = mockTemplateVersion(input.template_version_id)
  if (!v) return { ok: false, error: 'no such template version' }
  const row = catalog.find((c) => c.t.id === v.template_id)!
  const resolved = resolveTargets(input.selector, managedHosts)
  if (!resolved.length) return { ok: false, error: 'the selector matches no managed host' }
  if (!resolved.some((t) => t.eligible)) return { ok: false, error: 'none of the selected hosts can apply stacks' }
  const now = Date.now()
  // Preflight first, then plan: a host that cannot take the version is not a canary candidate.
  const blockedBy = new Map<string, string>()
  for (const t of resolved) {
    if (!t.eligible) continue
    const dry = mockDryRun({ template_version_id: input.template_version_id, host: t.host, name: input.stack_name, variables: input.variables, deploy_options: input.deploy_options, acknowledged_warnings: input.acknowledged_warnings })
    if (!dry.ok) blockedBy.set(t.host, dry.checks.filter((c) => c.status === 'fail').map((c) => c.detail).join('; '))
  }
  const plannable = resolved.map((t) => (blockedBy.has(t.host) ? { ...t, eligible: false } : t))
  if (!plannable.some((t) => t.eligible)) return { ok: false, error: 'no host passes preflight — nothing to roll out to' }
  const plan = planStages(plannable, input.strategy)
  const targets: RolloutTarget[] = resolved.map((t) => {
    const stage = plan.find((p) => p.host === t.host)?.stage ?? 0
    const st = stacks.find((s) => s.host === t.host && s.name === input.stack_name && s.status !== 'gone')
    const blocked = blockedBy.get(t.host) ?? null
    return {
      host: t.host,
      site: t.site,
      stage: t.eligible && !blocked ? stage : 0,
      status: !t.eligible || blocked ? 'skipped' : stage === 1 ? 'pending' : 'queued',
      attempts: 0,
      last_error: blocked ?? (!t.eligible ? t.note : null),
      stack_id: st?.id ?? null,
      previous_version: st?.template_version ?? null,
      agent_status: t.agent_status,
      updated_ms: now,
    }
  })
  const r: Rollout = {
    id: 'ro_' + shortId(input.name + now).slice(0, 8),
    name: input.name.trim() || `${row.t.name} ${v.version}`,
    template_id: row.t.id,
    template_name: row.t.name,
    template_version_id: v.id,
    template_version: v.version,
    previous_version: targets.find((t) => t.previous_version)?.previous_version ?? null,
    stack_name: input.stack_name,
    selector: input.selector,
    strategy: { ...input.strategy },
    state: 'draft',
    stage: 1,
    stages: stageCount(targets),
    targets,
    variables: { ...input.variables },
    created_by: 'demo',
    created_ms: now,
    updated_ms: now,
    started_ms: null,
    finished_ms: null,
  }
  rollouts = [r, ...rollouts]
  pushAudit({ action: 'rollout.create', target: { rollout: r.id, template: row.t.id, version: v.version }, result: 'ok', detail: `${targets.length} targets · ${r.stages} stages · threshold ${input.strategy.failure_threshold}` })
  if (input.start) mockRolloutAction(r.id, 'start')
  return { ok: true, rollout: viewRollout(rollouts.find((x) => x.id === r.id)!) }
}

export function mockRolloutAction(id: string, kind: RolloutActionKind): RolloutResult {
  const r = rollouts.find((x) => x.id === id)
  if (!r) return { ok: false, error: 'no such rollout' }
  const now = Date.now()
  const audit = (detail: string, action = `rollout.${kind}`) => pushAudit({ action, target: { rollout: r.id }, result: 'ok', detail })
  switch (kind) {
    case 'start':
      if (r.state !== 'draft') return { ok: false, error: 'already started' }
      r.state = 'running'
      r.started_ms = now
      for (const t of r.targets) if (t.stage === 1 && t.status === 'queued') t.status = 'pending'
      audit(`stage 1: ${r.targets.filter((t) => t.stage === 1).length} canary target(s)`)
      break
    case 'promote':
      if (r.state !== 'paused_gate' || r.stage >= r.stages) return { ok: false, error: 'nothing to promote' }
      r.stage += 1
      r.state = 'running'
      for (const t of r.targets) if (t.stage === r.stage && t.status === 'queued') t.status = 'pending'
      audit(`stage ${r.stage - 1} → ${r.stage}`)
      break
    case 'pause':
      if (r.state !== 'running') return { ok: false, error: 'not running' }
      r.state = 'paused_gate'
      audit('paused by operator')
      break
    case 'resume':
      if (r.state !== 'paused_threshold' && r.state !== 'paused_gate') return { ok: false, error: 'not paused' }
      r.state = 'running'
      audit('resumed past the threshold by operator', 'rollout.resume')
      break
    case 'retry':
      for (const t of r.targets) if (t.stage === r.stage && t.status === 'failed') { t.status = 'pending'; t.last_error = null; t.updated_ms = now }
      r.state = 'running'
      audit(`retrying failed targets in stage ${r.stage}`, 'rollout.retry')
      break
    case 'abort':
      if (r.state === 'completed' || r.state === 'rolled_back' || r.state === 'aborted') return { ok: false, error: 'already finished' }
      r.state = 'aborted'
      r.finished_ms = now
      audit('aborted; applied targets keep the new version')
      break
    case 'rollback': {
      if (!r.previous_version) return { ok: false, error: 'no previous version recorded' }
      const touched = r.targets.filter((t) => t.status === 'healthy' || t.status === 'failed' || t.status === 'drifted')
      for (const t of touched) {
        const st = stacks.find((s) => s.id === t.stack_id)
        if (st && t.previous_version) {
          st.template_version = t.previous_version
          st.desired_generation += 1
          st.status = 'pending'
          applying.set(st.id, { since: now, kind: 'up', args: { pull: true } })
        }
        t.status = 'pending'
        t.updated_ms = now
      }
      r.state = 'rolled_back'
      r.finished_ms = now
      audit(`${r.template_version} → ${r.previous_version} on ${touched.length} host${touched.length === 1 ? '' : 's'}`)
      settleStacks()
      break
    }
  }
  r.updated_ms = now
  tick()
  return { ok: true, rollout: viewRollout(r) }
}

// ---------------------------------------------------------------------------
// Exec sessions: a tiny fake shell, enough to show the terminal, the masking of secret-looking
// environment values, and the audit trail a session leaves.
// ---------------------------------------------------------------------------
import type { ExecOutput, ExecSessionResult } from '@/lib/core/api'

interface ExecState {
  host: string
  container: Container
  opened_ms: number
  commands: number
  cwd: string
}
const execSessions = new Map<string, ExecState>()

export function mockOpenExec(host: string, containerId: string): ExecSessionResult {
  const managed = managedHosts.find((h) => h.host === host)
  if (!managed) return { ok: false, error: 'no such host' }
  if (!managed.capabilities.includes('exec')) return { ok: false, error: 'the agent on this host was installed without --docker-exec' }
  if (managed.status !== 'online') return { ok: false, error: 'the host is offline — a shell needs a live agent' }
  const c = findContainer(host, containerId)
  if (!c) return { ok: false, error: 'no such container' }
  if (c.state !== 'running') return { ok: false, error: `${c.name} is not running` }
  const id = 'ex_' + shortId(host + containerId + Date.now()).slice(0, 8)
  execSessions.set(id, { host, container: c, opened_ms: Date.now(), commands: 0, cwd: '/' })
  pushAudit({ action: 'container.exec', target: { host, container: c.name }, result: 'ok', detail: `shell session ${id} opened` })
  return { ok: true, session: { id, host, container_id: c.id, container_name: c.name, shell: '/bin/sh', opened_ms: Date.now() } }
}

const SECRET_RE = /pass|secret|token|key|licen/i

export function mockExecSend(sessionId: string, line: string): ExecOutput {
  const s = execSessions.get(sessionId)
  if (!s) return { lines: ['session closed'], exit_code: null, closed: true }
  s.commands += 1
  const [cmd, ...args] = line.trim().split(/\s+/)
  const c = s.container
  switch (cmd) {
    case '':
      return { lines: [], exit_code: 0, closed: false }
    case 'exit':
    case 'logout':
      return mockCloseExecOutput(sessionId)
    case 'whoami':
      return { lines: ['root'], exit_code: 0, closed: false }
    case 'hostname':
      return { lines: [c.id.slice(0, 12)], exit_code: 0, closed: false }
    case 'pwd':
      return { lines: [s.cwd], exit_code: 0, closed: false }
    case 'cd':
      s.cwd = args[0] ? (args[0].startsWith('/') ? args[0] : s.cwd.replace(/\/$/, '') + '/' + args[0]) : '/'
      return { lines: [], exit_code: 0, closed: false }
    case 'ls':
      return { lines: [s.cwd === '/' ? 'app  bin  dev  etc  lib  proc  sys  tmp  usr  var' : 'config.yaml  data  logs  run.sh'], exit_code: 0, closed: false }
    case 'env':
    case 'printenv':
      return { lines: c.env_keys.map((k) => `${k}=${SECRET_RE.test(k) ? '••••••••' : (c.env[k] ?? 'value')}`), exit_code: 0, closed: false }
    case 'ps':
      return { lines: ['PID   USER     TIME  COMMAND', `    1 root      0:${String(12 + (s.commands % 40)).padStart(2, '0')} ${c.command}`, `   ${20 + s.commands} root      0:00 /bin/sh`, `   ${21 + s.commands} root      0:00 ps`], exit_code: 0, closed: false }
    case 'df':
      return { lines: ['Filesystem      Size  Used Avail Use% Mounted on', 'overlay         200G  128G   72G  64% /', 'tmpfs            64M     0   64M   0% /dev'], exit_code: 0, closed: false }
    case 'cat':
      if (args[0] === '/etc/hostname') return { lines: [c.id.slice(0, 12)], exit_code: 0, closed: false }
      if (args[0] === '/etc/os-release') return { lines: ['PRETTY_NAME="Alpine Linux v3.20"', 'ID=alpine'], exit_code: 0, closed: false }
      return { lines: [`cat: ${args[0] ?? ''}: No such file or directory`], exit_code: 1, closed: false }
    case 'uptime':
      return { lines: [` ${new Date().toTimeString().slice(0, 8)} up ${c.status_text.replace(/^Up /, '').replace(/ \(.*\)$/, '')},  load average: 0.42, 0.37, 0.31`], exit_code: 0, closed: false }
    case 'echo':
      return { lines: [args.join(' ')], exit_code: 0, closed: false }
    default:
      return { lines: [`sh: ${cmd}: not found`], exit_code: 127, closed: false }
  }
}

function mockCloseExecOutput(sessionId: string): ExecOutput {
  mockCloseExec(sessionId)
  return { lines: ['exit'], exit_code: 0, closed: true }
}

export function mockCloseExec(sessionId: string): MutationResult {
  const s = execSessions.get(sessionId)
  if (!s) return { ok: false, error: 'no such session' }
  execSessions.delete(sessionId)
  const secs = Math.round((Date.now() - s.opened_ms) / 1000)
  pushAudit({ action: 'container.exec', target: { host: s.host, container: s.container.name }, result: 'ok', detail: `shell session ${sessionId} closed after ${secs}s · ${s.commands} command${s.commands === 1 ? '' : 's'}` })
  return { ok: true }
}

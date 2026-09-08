// Display vocabulary for Stacks. Pure.
//
// A Stack's status is the state machine from the proposal (Declared → Rendered → Pending →
// Applying → Running → Drifted / Failed, Destroying → Gone), and `pending` is the one that
// matters most on-prem: a host that has not answered is the normal case, not an error, so it is
// neutral, never red.
import type { DriftItem, Stack, StackStatus } from '@/lib/core/api'

export interface StackStatusMeta {
  label: string
  tone: 'success' | 'error' | 'warning' | 'neutral' | 'info'
  hint: string
  /** Counts toward the "needs attention" filter. */
  attention: boolean
}

export const STACK_STATUS_META: Record<StackStatus, StackStatusMeta> = {
  declared: { label: 'Declared', tone: 'neutral', hint: 'Saved, not rendered yet.', attention: false },
  rendered: { label: 'Rendered', tone: 'neutral', hint: 'Rendered and stored; nothing sent to the host yet.', attention: false },
  pending: { label: 'Pending', tone: 'neutral', hint: 'Waiting for the host: it applies on the next sync.', attention: false },
  applying: { label: 'Applying', tone: 'info', hint: 'The agent is bringing the stack up.', attention: false },
  running: { label: 'Running', tone: 'success', hint: 'Applied and matching what runs.', attention: false },
  drifted: { label: 'Drifted', tone: 'warning', hint: 'What runs differs from what was applied. Reconciling is a deliberate choice.', attention: true },
  failed: { label: 'Failed', tone: 'error', hint: 'The last apply did not succeed.', attention: true },
  destroying: { label: 'Destroying', tone: 'info', hint: 'The agent is taking it down.', attention: false },
  gone: { label: 'Gone', tone: 'neutral', hint: 'Destroyed. Kept for history.', attention: false },
}

export const STACK_STATUSES: StackStatus[] = ['running', 'drifted', 'failed', 'pending', 'applying', 'declared', 'rendered', 'destroying', 'gone']

/** "15", or "14 → 15" when a newer generation is waiting to be applied. */
export function generationLabel(s: Pick<Stack, 'applied_generation' | 'desired_generation'>): string {
  if (s.applied_generation == null) return `→ ${s.desired_generation}`
  return s.applied_generation === s.desired_generation ? String(s.applied_generation) : `${s.applied_generation} → ${s.desired_generation}`
}

export function hasPendingGeneration(s: Pick<Stack, 'applied_generation' | 'desired_generation'>): boolean {
  return s.applied_generation == null || s.applied_generation !== s.desired_generation
}

/** "template nodeflux-vas 2.4.1" / "compose" / "adopted". */
export function sourceLabel(s: Pick<Stack, 'source' | 'template_name' | 'template_version'>): string {
  if (s.source === 'template') return `${s.template_name ?? 'template'} ${s.template_version ?? ''}`.trim()
  return s.source === 'adopted' ? 'adopted' : 'compose'
}

const DRIFT_LABEL: Record<DriftItem['kind'], string> = {
  container_missing: 'container missing',
  extra_container: 'extra container',
  image_changed: 'image changed',
  config_changed: 'config changed',
  state_changed: 'state changed',
  env_changed: 'environment changed',
}

export function driftLabel(d: Pick<DriftItem, 'kind'>): string {
  return DRIFT_LABEL[d.kind]
}

/** "2 changes: image changed (vas-inference), extra container" */
export function driftSummary(items: DriftItem[]): string {
  if (!items.length) return 'no drift'
  const parts = items.slice(0, 3).map((d) => `${driftLabel(d)}${d.service ? ` (${d.service})` : ''}`)
  const more = items.length > 3 ? `, +${items.length - 3} more` : ''
  return `${items.length} ${items.length === 1 ? 'change' : 'changes'}: ${parts.join(', ')}${more}`
}

export interface StackCounts {
  total: number
  running: number
  drifted: number
  failed: number
  pending: number
}

export function stackCounts(stacks: Stack[]): StackCounts {
  const live = stacks.filter((s) => s.status !== 'gone')
  return {
    total: live.length,
    running: live.filter((s) => s.status === 'running').length,
    drifted: live.filter((s) => s.status === 'drifted').length,
    failed: live.filter((s) => s.status === 'failed').length,
    pending: live.filter((s) => s.status === 'pending' || s.status === 'applying').length,
  }
}

export interface StackFilter {
  q?: string | null
  host?: string | null
  site?: string | null
  attention?: boolean
  /** One of the chip strip's buckets: `pending` covers pending + applying, like `stackCounts`. */
  bucket?: 'running' | 'pending' | null
  includeGone?: boolean
}

export function filterStacks(stacks: Stack[], f: StackFilter): Stack[] {
  const q = (f.q ?? '').trim().toLowerCase()
  return stacks.filter((s) => {
    if (!f.includeGone && s.status === 'gone') return false
    if (f.host && s.host !== f.host) return false
    if (f.site && s.site !== f.site) return false
    if (f.attention && !STACK_STATUS_META[s.status].attention && !hasPendingGeneration(s)) return false
    if (f.bucket === 'running' && s.status !== 'running') return false
    if (f.bucket === 'pending' && s.status !== 'pending' && s.status !== 'applying') return false
    if (q && ![s.name, s.host, s.site ?? '', s.template_name ?? '', ...s.services.map((x) => x.name)].join('\n').toLowerCase().includes(q)) return false
    return true
  })
}

/** Attention first (failed, drifted), then pending, then by name. */
export function sortStacks(stacks: Stack[]): Stack[] {
  const rank: Record<StackStatus, number> = { failed: 0, drifted: 1, applying: 2, pending: 3, destroying: 4, running: 5, declared: 6, rendered: 6, gone: 7 }
  return [...stacks].sort((a, b) => rank[a.status] - rank[b.status] || a.name.localeCompare(b.name) || a.host.localeCompare(b.host))
}

export interface HostGroup {
  host: string
  site: string | null
  stacks: Stack[]
}

/**
 * The list grouped under its hosts — "what runs at each site" literally. Feed it `sortStacks`
 * output: a host's position is that of its first (worst) stack, so a host with a failed stack
 * leads the page, and inside the group the attention-first order is kept.
 */
export function groupStacksByHost(sorted: Stack[]): HostGroup[] {
  const groups: HostGroup[] = []
  const byHost = new Map<string, HostGroup>()
  for (const s of sorted) {
    let g = byHost.get(s.host)
    if (!g) {
      g = { host: s.host, site: s.site, stacks: [] }
      byHost.set(s.host, g)
      groups.push(g)
    }
    g.stacks.push(s)
  }
  return groups
}

/**
 * The "Last change" cell: what happened last and when, in the status's own verb, so the row
 * does not need a separate Note column to say why the time matters.
 */
export function lastChangeLabel(s: Pick<Stack, 'status' | 'updated_ms' | 'last_applied_ms' | 'created_by'>, nowMs: number = Date.now()): string {
  const age = (ms: number): string => {
    const secs = Math.max(0, Math.round((nowMs - ms) / 1000))
    if (secs < 60) return `${secs}s ago`
    const mins = Math.round(secs / 60)
    if (mins < 60) return `${mins}m ago`
    const hours = Math.round(mins / 60)
    if (hours < 48) return `${hours}h ago`
    return `${Math.round(hours / 24)}d ago`
  }
  switch (s.status) {
    case 'failed': return `apply failed ${age(s.updated_ms)}`
    case 'drifted': return `drift seen ${age(s.updated_ms)}`
    case 'running': return `applied ${age(s.last_applied_ms ?? s.updated_ms)}`
    case 'applying': return `applying since ${age(s.updated_ms)}`
    case 'destroying': return `destroying since ${age(s.updated_ms)}`
    case 'gone': return `destroyed ${age(s.updated_ms)}`
    default: return `saved ${age(s.updated_ms)} by ${s.created_by}`
  }
}

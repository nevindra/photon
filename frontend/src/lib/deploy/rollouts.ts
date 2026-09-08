// The Rollout vocabulary: states, per-target statuses, target selection over the managed-host
// list, and the staging plan (a canary subset first, then the rest, with an explicit gate between
// — R6.2). Pure; the supervisor that advances a rollout lives server-side.
import type { AgentStatus, ManagedHost, Rollout, RolloutState, RolloutStrategy, RolloutTarget, TargetSelector, TargetStatus } from '@/lib/core/api'

export interface RolloutStateMeta {
  label: string
  tone: 'success' | 'error' | 'warning' | 'neutral' | 'info'
  hint: string
  /** Waiting on an operator: the list's "needs a decision" filter. */
  decision: boolean
  active: boolean
}

export const ROLLOUT_STATE_META: Record<RolloutState, RolloutStateMeta> = {
  draft: { label: 'Draft', tone: 'neutral', hint: 'Planned, not started. Nothing has been sent to any host.', decision: false, active: false },
  running: { label: 'Running', tone: 'info', hint: 'Targets in the current stage are being applied.', decision: false, active: true },
  paused_gate: { label: 'At the gate', tone: 'warning', hint: 'The canary stage finished. Promote to continue, or roll back.', decision: true, active: true },
  paused_threshold: { label: 'Paused — failures', tone: 'error', hint: 'Too many targets failed in this stage. Nothing further is applied until you decide.', decision: true, active: true },
  completed: { label: 'Completed', tone: 'success', hint: 'Every target is healthy, failed, or skipped, and the last stage is done.', decision: false, active: false },
  aborted: { label: 'Aborted', tone: 'neutral', hint: 'Stopped by an operator. Targets already applied keep the new version.', decision: false, active: false },
  rolled_back: { label: 'Rolled back', tone: 'warning', hint: 'The previous version was re-applied to every target this rollout had touched.', decision: false, active: false },
}

export const ROLLOUT_STATES: RolloutState[] = ['running', 'paused_gate', 'paused_threshold', 'draft', 'completed', 'rolled_back', 'aborted']

export interface TargetStatusMeta {
  label: string
  tone: 'success' | 'error' | 'warning' | 'neutral' | 'info'
  hint: string
}

// `pending` deserves the emphasis: on-prem, a target that has not answered is the normal case.
export const TARGET_STATUS_META: Record<TargetStatus, TargetStatusMeta> = {
  queued: { label: 'Queued', tone: 'neutral', hint: 'Its stage has not started. Nothing has been sent to this host yet.' },
  pending: { label: 'Pending', tone: 'neutral', hint: 'Waiting for the host to sync. Not a failure — an offline site picks it up when it returns.' },
  prepulling: { label: 'Pre-pulling', tone: 'info', hint: 'Fetching images before the restart window, so a slow link costs seconds of downtime, not minutes.' },
  applying: { label: 'Applying', tone: 'info', hint: 'The agent is bringing the new generation up.' },
  healthy: { label: 'Healthy', tone: 'success', hint: 'Applied, and the template’s health gate passed.' },
  failed: { label: 'Failed', tone: 'error', hint: 'The apply or the health gate failed. Counts toward the stage threshold.' },
  drifted: { label: 'Drifted', tone: 'warning', hint: 'Applied, but what runs no longer matches. Reconcile from the stack.' },
  skipped: { label: 'Skipped', tone: 'neutral', hint: 'Requirements not met (preflight failed). Not counted as a failure.' },
}

export const TARGET_STATUSES: TargetStatus[] = ['healthy', 'applying', 'prepulling', 'pending', 'queued', 'failed', 'drifted', 'skipped']

export interface ResolvedTarget {
  host: string
  site: string | null
  agent_status: AgentStatus
  eligible: boolean
  /** Why not eligible, or a note ("offline — waits as pending"). */
  note: string | null
}

/** Which managed hosts a selector names. Site targeting reuses federation's tenant (R6.1). */
export function resolveTargets(selector: TargetSelector, hosts: ManagedHost[]): ResolvedTarget[] {
  const picked = hosts.filter((h) => {
    if (selector.kind === 'hosts') return selector.hosts.includes(h.host)
    if (selector.kind === 'site') return selector.sites.includes(h.site ?? '')
    return Object.entries(selector.labels).every(([k, v]) => h.labels[k] === v)
  })
  return picked
    .map<ResolvedTarget>((h) => {
      if (!h.capabilities.includes('apply')) return { host: h.host, site: h.site, agent_status: h.status, eligible: false, note: 'agent installed without --docker-apply' }
      if (h.status !== 'online') return { host: h.host, site: h.site, agent_status: h.status, eligible: true, note: h.status === 'offline' ? 'offline — waits as pending' : 'never synced — waits as pending' }
      return { host: h.host, site: h.site, agent_status: h.status, eligible: true, note: null }
    })
    .sort((a, b) => (a.site ?? '').localeCompare(b.site ?? '') || a.host.localeCompare(b.host))
}

/** Canary size for N eligible targets: at least 1 when there is anything at all. */
export function canarySize(eligibleCount: number, strategy: Pick<RolloutStrategy, 'canary' | 'canary_is_percent'>): number {
  if (eligibleCount <= 0) return 0
  const n = strategy.canary_is_percent ? Math.ceil((eligibleCount * strategy.canary) / 100) : strategy.canary
  return Math.max(1, Math.min(eligibleCount, n))
}

/**
 * Two stages: the canary subset, then the rest. Online hosts are preferred for the canary so the
 * gate can actually be judged instead of waiting on a box that is off for the weekend.
 */
export function planStages(targets: ResolvedTarget[], strategy: Pick<RolloutStrategy, 'canary' | 'canary_is_percent'>): { host: string; stage: number }[] {
  const eligible = targets.filter((t) => t.eligible)
  const n = canarySize(eligible.length, strategy)
  const ordered = [...eligible].sort((a, b) => (a.agent_status === 'online' ? 0 : 1) - (b.agent_status === 'online' ? 0 : 1))
  const canary = new Set(ordered.slice(0, n).map((t) => t.host))
  return targets.map((t) => ({ host: t.host, stage: !t.eligible ? 0 : canary.has(t.host) ? 1 : eligible.length > n ? 2 : 1 }))
}

export function stageCount(targets: Pick<RolloutTarget, 'stage'>[]): number {
  return Math.max(1, ...targets.map((t) => t.stage))
}

export interface TargetCounts {
  total: number
  healthy: number
  failed: number
  pending: number
  queued: number
  inflight: number
  skipped: number
  drifted: number
}

export function targetCounts(targets: Pick<RolloutTarget, 'status'>[]): TargetCounts {
  return {
    total: targets.length,
    healthy: targets.filter((t) => t.status === 'healthy').length,
    failed: targets.filter((t) => t.status === 'failed').length,
    pending: targets.filter((t) => t.status === 'pending').length,
    queued: targets.filter((t) => t.status === 'queued').length,
    inflight: targets.filter((t) => t.status === 'applying' || t.status === 'prepulling').length,
    skipped: targets.filter((t) => t.status === 'skipped').length,
    drifted: targets.filter((t) => t.status === 'drifted').length,
  }
}

export function selectorSummary(s: TargetSelector): string {
  if (s.kind === 'hosts') return s.hosts.length === 1 ? s.hosts[0] : `${s.hosts.length} hosts`
  if (s.kind === 'site') return s.sites.length === 1 ? `site ${s.sites[0]}` : `${s.sites.length} sites`
  const parts = Object.entries(s.labels).map(([k, v]) => `${k}=${v}`)
  return parts.length ? `labels ${parts.join(', ')}` : 'every managed host'
}

export function strategySummary(s: RolloutStrategy): string {
  const canary = s.canary_is_percent ? `${s.canary}% canary` : `${s.canary} canary`
  return `${canary} · gate before the rest · pause at ${s.failure_threshold} ${s.failure_threshold === 1 ? 'failure' : 'failures'}${s.prepull ? ' · pre-pull images' : ''}${s.auto_promote ? ' · auto-promote' : ''}`
}

export type RolloutActionKind = 'start' | 'promote' | 'pause' | 'resume' | 'abort' | 'rollback' | 'retry'

/** Which verbs make sense in a state, in the order the bar shows them. */
export function availableRolloutActions(r: Pick<Rollout, 'state' | 'stage' | 'stages' | 'previous_version'>): RolloutActionKind[] {
  switch (r.state) {
    case 'draft':
      return ['start', 'abort']
    case 'running':
      return ['pause', 'abort', ...(r.previous_version ? ['rollback' as const] : [])]
    case 'paused_gate':
      return [...(r.stage < r.stages ? ['promote' as const] : []), 'abort', ...(r.previous_version ? ['rollback' as const] : [])]
    case 'paused_threshold':
      return ['retry', 'resume', 'abort', ...(r.previous_version ? ['rollback' as const] : [])]
    case 'completed':
      return r.previous_version ? ['rollback'] : []
    case 'aborted':
      return r.previous_version ? ['rollback'] : []
    case 'rolled_back':
      return []
  }
}

export const DEFAULT_STRATEGY: RolloutStrategy = { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 300 }

/** "nodeflux-vas 2.4.2 → divtik" — the default rollout name. */
export function defaultRolloutName(templateName: string, version: string, selector: TargetSelector): string {
  return `${templateName} ${version} → ${selectorSummary(selector)}`
}

export function rolloutCounts(rollouts: Rollout[]): { active: number; decision: number; completed: number; rolledBack: number } {
  return {
    active: rollouts.filter((r) => ROLLOUT_STATE_META[r.state].active).length,
    decision: rollouts.filter((r) => ROLLOUT_STATE_META[r.state].decision).length,
    completed: rollouts.filter((r) => r.state === 'completed').length,
    rolledBack: rollouts.filter((r) => r.state === 'rolled_back').length,
  }
}

export function sortRollouts(rollouts: Rollout[]): Rollout[] {
  const rank: Record<RolloutState, number> = { paused_threshold: 0, paused_gate: 1, running: 2, draft: 3, completed: 4, rolled_back: 5, aborted: 6 }
  return [...rollouts].sort((a, b) => rank[a.state] - rank[b.state] || b.updated_ms - a.updated_ms)
}

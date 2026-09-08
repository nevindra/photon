// Display helpers for the audit log. Pure.
//
// An audit row's `action` is a dotted `noun.verb` (`container.restart`); the table shows it as a
// short human phrase ("Restart container") and groups the filter by noun, because "everything
// anyone did to stacks this week" is the question, not "every restart".
import type { AuditEntry, AuditResultKind } from '@/lib/core/api'

export interface ResultMeta {
  label: string
  tone: 'success' | 'error' | 'warning' | 'neutral'
  hint: string
}

export const RESULT_META: Record<AuditResultKind, ResultMeta> = {
  ok: { label: 'OK', tone: 'success', hint: 'Completed.' },
  failed: { label: 'Failed', tone: 'error', hint: 'The agent tried and could not.' },
  denied: { label: 'Denied', tone: 'warning', hint: "Refused by the role check — nothing ran on the host." },
  pending: { label: 'Pending', tone: 'neutral', hint: 'Recorded; the host will pick it up on its next sync.' },
}

export const RESULTS: AuditResultKind[] = ['ok', 'failed', 'denied', 'pending']

// Filterable noun families, in the order an operator would look for them.
export const ACTION_FAMILIES: { value: string; label: string }[] = [
  { value: 'container', label: 'Containers' },
  { value: 'stack', label: 'Stacks' },
  { value: 'rollout', label: 'Rollouts' },
  { value: 'template', label: 'Templates' },
  { value: 'agent', label: 'Agents' },
  { value: 'prune', label: 'Prune' },
  { value: 'user', label: 'Users' },
]

const VERB: Record<string, string> = {
  start: 'Start',
  stop: 'Stop',
  restart: 'Restart',
  kill: 'Kill',
  remove: 'Remove',
  pull: 'Pull image for',
  recreate: 'Recreate',
  apply: 'Apply',
  create: 'Create',
  destroy: 'Destroy',
  reconcile: 'Reconcile',
  accept_drift: 'Accept drift on',
  complete: 'Complete',
  edit: 'Edit',
  adopt: 'Adopt',
  promote: 'Promote',
  pause: 'Pause',
  abort: 'Abort',
  rollback: 'Roll back',
  publish: 'Publish',
  import: 'Import',
  enroll: 'Enroll',
  bootstrap: 'Bootstrap',
  rotate_token: 'Rotate token for',
  upgrade: 'Upgrade',
  update: 'Update',
  role: 'Change role of',
  exec: 'Open shell in',
  prepull: 'Pre-pull images for',
}

const NOUN: Record<string, string> = {
  image: 'image',
  container: 'container',
  volume: 'volume',
  stack: 'stack',
  rollout: 'rollout',
  template: 'template',
  agent: 'agent',
  user: 'user',
  registry: 'registry',
}

/** `container.restart` → "Restart container"; `prune` → "Prune"; unknown → the raw action. */
export function actionLabel(action: string): string {
  const [noun, verb] = action.split('.')
  if (!verb) return capitalize(VERB[noun] ?? noun)
  const v = VERB[verb]
  const n = NOUN[noun] ?? noun
  return v ? `${v} ${n}` : `${capitalize(verb)} ${n}`
}

function capitalize(s: string): string {
  return s ? s[0].toUpperCase() + s.slice(1) : s
}

/** The most specific name the row is about, and its host if it has one. */
export function targetSummary(e: AuditEntry): { primary: string | null; host: string | null } {
  const t = e.target
  const primary = t.container ?? t.image ?? t.volume ?? t.stack ?? t.rollout ?? (t.template ? `${t.template}${t.version ? ' ' + t.version : ''}` : null) ?? t.registry ?? t.user ?? null
  return { primary, host: t.host ?? null }
}

export interface SinceOption {
  value: string
  label: string
  ms: number | null
}
export const SINCE_OPTIONS: SinceOption[] = [
  { value: '24h', label: 'Last 24 hours', ms: 24 * 3_600_000 },
  { value: '7d', label: 'Last 7 days', ms: 7 * 24 * 3_600_000 },
  { value: '30d', label: 'Last 30 days', ms: 30 * 24 * 3_600_000 },
  { value: 'all', label: 'All time', ms: null },
]

export function sinceMs(value: string, nowMs: number = Date.now()): number | undefined {
  const opt = SINCE_OPTIONS.find((o) => o.value === value) ?? SINCE_OPTIONS[1]
  return opt.ms == null ? undefined : nowMs - opt.ms
}

/** Compact absolute stamp for a control-plane row: "Sep 4, 10:42". */
export function stampMs(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

/** Relative age of a ms timestamp: "9s ago", "3d ago". */
export function ageMs(ms: number, nowMs: number = Date.now()): string {
  const secs = Math.max(0, Math.round((nowMs - ms) / 1000))
  if (secs < 60) return `${secs}s ago`
  const mins = Math.round(secs / 60)
  if (mins < 60) return `${mins}m ago`
  const hours = Math.round(mins / 60)
  if (hours < 48) return `${hours}h ago`
  return `${Math.round(hours / 24)}d ago`
}

/** Time until a future ms timestamp: "14m", "3s"; "now" once it has passed. */
export function untilMs(ms: number, nowMs: number = Date.now()): string {
  const secs = Math.round((ms - nowMs) / 1000)
  if (secs <= 0) return 'now'
  if (secs < 60) return `${secs}s`
  const mins = Math.round(secs / 60)
  if (mins < 60) return `${mins}m`
  return `${Math.round(mins / 60)}h`
}

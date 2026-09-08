// The one-shot container Action vocabulary and its confirmation policy. Pure.
//
// Actions are NOT desired state (that is a Stack's job). They are "do this once, soon": a
// restart, a stop, a pull. Each carries an expiry so a restart queued while a site was offline for
// a week expires instead of firing on Monday. The UI's job is to size the confirmation to the
// blast radius — a start needs none, a stop a sentence, a remove the container's name typed —
// and to say what happens on an offline host (it waits as pending; it does not fail).
import type { ActionKind, Container } from '@/lib/core/api'

export type Risk = 'none' | 'confirm' | 'typed'

export interface ActionMeta {
  label: string
  /** Progressive form for toasts and row chips: "Restarting". */
  progressive: string
  /** Past form for the done toast: "Restarted". */
  done: string
  risk: Risk
  destructive: boolean
  /** What Docker will actually do, in one sentence an operator can act on. */
  explain: (c: Pick<Container, 'name' | 'image' | 'ports'>) => string
}

export const ACTION_META: Record<ActionKind, ActionMeta> = {
  start: { label: 'Start', progressive: 'Starting', done: 'Started', risk: 'none', destructive: false, explain: (c) => `Starts ${c.name} with its existing configuration.` },
  stop: {
    label: 'Stop',
    progressive: 'Stopping',
    done: 'Stopped',
    risk: 'confirm',
    destructive: false,
    explain: (c) => `Sends SIGTERM to ${c.name}, waits 10 seconds, then SIGKILL.${c.ports.length ? ` Anything on ${c.ports.map((p) => p.host_port ?? p.container_port).join(', ')} stops answering until it is started again.` : ''}`,
  },
  restart: {
    label: 'Restart',
    progressive: 'Restarting',
    done: 'Restarted',
    risk: 'confirm',
    destructive: false,
    explain: (c) => `Stops and starts ${c.name} with the same image and configuration.${c.ports.length ? ` A short gap on ${c.ports.map((p) => p.host_port ?? p.container_port).join(', ')} while it comes back.` : ''}`,
  },
  kill: {
    label: 'Kill',
    progressive: 'Killing',
    done: 'Killed',
    risk: 'confirm',
    destructive: true,
    explain: (c) => `SIGKILL to ${c.name} immediately — no graceful shutdown, no flush. For a container that will not stop.`,
  },
  pull: {
    label: 'Pull image',
    progressive: 'Pulling image for',
    done: 'Pulled image for',
    risk: 'confirm',
    destructive: false,
    explain: (c) => `Pulls ${c.image} from its registry. The running container is untouched; recreate it to use the new image.`,
  },
  recreate: {
    label: 'Recreate',
    progressive: 'Recreating',
    done: 'Recreated',
    risk: 'confirm',
    destructive: false,
    explain: (c) => `Removes ${c.name} and creates it again from ${c.image} with the same configuration. Named volumes are kept; anything written inside the container's own filesystem is lost.`,
  },
  remove: {
    label: 'Remove',
    progressive: 'Removing',
    done: 'Removed',
    risk: 'typed',
    destructive: true,
    explain: (c) => `Deletes ${c.name}. Its named volumes stay unless you say otherwise; its own filesystem and logs are gone.`,
  },
  prune: { label: 'Prune', progressive: 'Pruning', done: 'Pruned', risk: 'typed', destructive: true, explain: () => 'Removes what nothing uses.' },
  image_pull: { label: 'Pull image', progressive: 'Pulling', done: 'Pulled', risk: 'confirm', destructive: false, explain: () => 'Pulls the tag from its registry.' },
  image_remove: { label: 'Remove image', progressive: 'Removing image', done: 'Removed image', risk: 'confirm', destructive: true, explain: () => 'Frees the image on the host.' },
  volume_remove: { label: 'Remove volume', progressive: 'Removing volume', done: 'Removed volume', risk: 'typed', destructive: true, explain: () => 'Deletes the volume and everything in it.' },
}

/** Which actions make sense for a container in this state, in menu order. */
export function availableActions(c: Pick<Container, 'state'>): ActionKind[] {
  switch (c.state) {
    case 'running':
      return ['restart', 'stop', 'kill', 'pull', 'recreate', 'remove']
    case 'paused':
      return ['restart', 'stop', 'kill', 'remove']
    case 'restarting':
      return ['stop', 'kill', 'pull', 'recreate', 'remove']
    case 'exited':
    case 'created':
    case 'dead':
      return ['start', 'pull', 'recreate', 'remove']
    case 'removing':
      return []
  }
}

/** The one primary action a row offers before the menu: start when stopped, restart when running. */
export function primaryAction(c: Pick<Container, 'state'>): ActionKind | null {
  if (c.state === 'running' || c.state === 'paused') return 'restart'
  if (c.state === 'restarting') return 'stop' // a crash loop needs stopping, not another restart
  if (c.state === 'exited' || c.state === 'created' || c.state === 'dead') return 'start'
  return null
}

// Default lease: an action nobody has picked up in 15 minutes is stale. Long enough for a site's
// flaky uplink, short enough that "restart it" cannot land during tomorrow's peak.
export const ACTION_TTL_MS = 15 * 60_000

export interface ActionStatusMeta {
  label: string
  tone: 'success' | 'error' | 'warning' | 'neutral' | 'info'
}
export const ACTION_STATUS_META: Record<'queued' | 'running' | 'done' | 'failed' | 'expired', ActionStatusMeta> = {
  queued: { label: 'Queued', tone: 'neutral' },
  running: { label: 'Running', tone: 'info' },
  done: { label: 'Done', tone: 'success' },
  failed: { label: 'Failed', tone: 'error' },
  expired: { label: 'Expired', tone: 'warning' },
}

/** A prune's human summary: "Would free 2.8 GB (images 2.1 GB, build cache 0.7 GB)". */
export function pruneSummary(parts: { label: string; bytes: number; count: number; selected: boolean }[], fmt: (b: number) => string): string {
  const chosen = parts.filter((p) => p.selected && (p.bytes > 0 || p.count > 0))
  if (!chosen.length) return 'Nothing selected.'
  const total = chosen.reduce((n, p) => n + p.bytes, 0)
  return `Would free ${fmt(total)} (${chosen.map((p) => `${p.label.toLowerCase()} ${fmt(p.bytes)}`).join(', ')})`
}

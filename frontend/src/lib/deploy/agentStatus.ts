// Display vocabulary for a managed agent (the `photon-agent` with its Docker capability on a Host).
// Pure: the api types are the input, the words and tones are the output.
//
// Three states, and the middle one is the load-bearing one. On-prem a host that has not answered
// is the normal case — a box switched off for the weekend, a site whose uplink is down — so
// "Offline" is stated plainly and never dressed as a failure of anything Photon queued for it:
// desired state waits as `pending` and applies when the agent returns. "Waiting for first sync" is
// the enrolment gap: a token exists, the agent has never called home, and the honest thing to say
// is that Photon does not know what is on that box yet.
import type { AgentCapability, AgentStatus, ManagedHost } from '@/lib/core/api'

export interface AgentStatusMeta {
  label: string
  tone: 'success' | 'error' | 'neutral'
  /** Why the agent is in this state, for the tooltip / detail header. */
  hint: string
}

export const AGENT_STATUS_META: Record<AgentStatus, AgentStatusMeta> = {
  online: { label: 'Online', tone: 'success', hint: 'Synced within the last two poll intervals.' },
  offline: {
    label: 'Offline',
    tone: 'error',
    hint: 'Reported, then stopped. Anything queued for this host waits as pending and applies when it returns.',
  },
  pending: {
    label: 'Waiting for first sync',
    tone: 'neutral',
    hint: 'Enrolled, but the agent has not called home yet. Photon does not know what runs here.',
  },
}

export interface CapabilityMeta {
  label: string
  summary: string
  /** The agent-side flag that grants it — capabilities are opt-in on the host, never assigned by the server. */
  flag: string
  /** Rendered as a caution on the pickers; exec is the one feature a customer's security team will ask about. */
  risk?: string
}

export const CAPABILITIES: AgentCapability[] = ['read', 'lifecycle', 'apply', 'exec']

export const CAPABILITY_META: Record<AgentCapability, CapabilityMeta> = {
  read: { label: 'Read', summary: 'Inventory, container logs and live stats.', flag: '--docker-read' },
  lifecycle: {
    label: 'Lifecycle',
    summary: 'Start, stop, restart, kill, remove, pull and recreate containers.',
    flag: '--docker-lifecycle',
  },
  apply: { label: 'Apply', summary: 'Create, update and destroy compose stacks.', flag: '--docker-apply' },
  exec: {
    label: 'Exec',
    summary: 'Open a shell inside a container.',
    flag: '--docker-exec',
    risk: 'Root-equivalent on the host. Grant it only where someone has asked for it.',
  },
}

/** Sort capabilities into their canonical order regardless of how the agent reported them. */
export function orderedCapabilities(caps: readonly AgentCapability[]): AgentCapability[] {
  return CAPABILITIES.filter((c) => caps.includes(c))
}

export function hasCapability(host: Pick<ManagedHost, 'capabilities'> | null | undefined, cap: AgentCapability): boolean {
  return !!host && host.capabilities.includes(cap)
}

/** An agent has an upgrade waiting when the server advertises a newer version than the one it reports. */
export function upgradeAvailable(host: Pick<ManagedHost, 'agent_version' | 'latest_agent_version'>): boolean {
  return !!host.agent_version && !!host.latest_agent_version && host.agent_version !== host.latest_agent_version
}

// The copy-pasteable install for the manual path. The binary comes from THIS Photon (`/api/agent/
// download`), never from the internet — an air-gapped site has no internet to download from. The
// server URL is this UI's origin: the sync endpoint lives on the API surface, unlike the federation
// snippet, which has to point at the OTLP ingest port.
export type InstallFormat = 'systemd' | 'docker'

export function installSnippet(
  opts: { token: string; host: string; capabilities: readonly AgentCapability[]; pollSecs: number; origin: string },
  format: InstallFormat,
): string {
  const flags = orderedCapabilities(opts.capabilities)
    .map((c) => CAPABILITY_META[c].flag)
    .join(' ')
  if (format === 'docker') {
    return [
      `docker run -d --name photon-agent --restart unless-stopped \\`,
      `  -v /var/run/docker.sock:/var/run/docker.sock:ro \\`,
      `  -e PHOTON_SERVER=${opts.origin} \\`,
      `  -e PHOTON_AGENT_TOKEN=${opts.token} \\`,
      `  -e PHOTON_AGENT_HOST_NAME=${opts.host} \\`,
      `  ghcr.io/nodeflux/photon-agent:latest ${flags} --poll ${opts.pollSecs}s`,
    ].join('\n')
  }
  return [
    `# 1. fetch the agent from this Photon (no internet needed)`,
    `curl -fsSL -o /usr/local/bin/photon-agent "${opts.origin}/api/agent/download?arch=$(uname -m)"`,
    `chmod +x /usr/local/bin/photon-agent`,
    ``,
    `# 2. enroll — the token is shown once, keep it out of shell history`,
    `photon-agent install \\`,
    `  --server ${opts.origin} \\`,
    `  --token ${opts.token} \\`,
    `  --host-name ${opts.host} \\`,
    `  ${flags} --poll ${opts.pollSecs}s`,
    ``,
    `# 3. installs and starts the systemd unit; the host appears once the first sync lands`,
  ].join('\n')
}

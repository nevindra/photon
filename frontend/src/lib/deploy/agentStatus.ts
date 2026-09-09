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
// download`), never from the internet — an air-gapped site has no internet to download from.
//
// Three things the first version of this got wrong, all of which failed silently on the host:
// it ran root-only steps without `sudo`; `curl -s` without `-S` swallowed the "permission
// denied" that followed; and it pasted the token into the command line under a comment that
// said to keep it out of shell history. Now every privileged line carries `sudo`, curl reports
// its errors, and the token is left OUT of the command by default — `photon-agent` prompts for
// it (echo off) when stdin is a terminal. `embedToken` puts it back for an operator pasting
// into an automation, and the snippet then says so.
export type InstallFormat = 'systemd' | 'foreground'

export interface SnippetOpts {
  token: string
  host: string
  capabilities: readonly AgentCapability[]
  pollSecs: number
  /** The URL the host dials — the server's own idea of it, editable in the panel. */
  origin: string
  /** Put `--token ak_…` in the command instead of prompting for it. Off by default. */
  embedToken?: boolean
}

export const INSTALL_FORMAT_META: Record<InstallFormat, { label: string; blurb: string }> = {
  systemd: { label: 'Install as a service', blurb: 'systemd unit, starts on boot. Needs root.' },
  foreground: { label: 'Run in the foreground', blurb: 'Try it first, no root needed. Ctrl-C stops it.' },
}

/** `http://host:8080/` → `http://host:8080`. */
export function normalizeOrigin(url: string): string {
  return url.trim().replace(/\/+$/, '')
}

export function installSnippet(opts: SnippetOpts, format: InstallFormat): string {
  const origin = normalizeOrigin(opts.origin)
  const flags = [...orderedCapabilities(opts.capabilities).map((c) => CAPABILITY_META[c].flag), `--poll ${opts.pollSecs}s`]
  const download = `"${origin}/api/agent/download?arch=$(uname -m)"`
  const tokenArg = opts.embedToken ? ` --token ${opts.token}` : ''
  const tokenNote = opts.embedToken
    ? '# the token is embedded in this command — clear it from your shell history afterwards'
    : "# you'll be asked for the token; paste it (the input is hidden)"
  if (format === 'foreground') {
    return [
      `# 1. Fetch the agent from this Photon into the current directory (no internet needed)`,
      `curl -fSsL -o photon-agent ${download} && chmod +x photon-agent`,
      ``,
      `# 2. Run it — Ctrl-C stops it, nothing is installed`,
      tokenNote,
      `./photon-agent --server ${origin} --host-name ${opts.host}${tokenArg} \\`,
      `  ${flags.join(' ')}`,
    ].join('\n')
  }
  return [
    `# 1. Fetch the agent from this Photon (no internet needed)`,
    `sudo curl -fSsL -o /usr/local/bin/photon-agent ${download}`,
    `sudo chmod +x /usr/local/bin/photon-agent`,
    ``,
    `# 2. Enroll and start the service`,
    tokenNote,
    `sudo photon-agent install --server ${origin} --host-name ${opts.host}${tokenArg} \\`,
    `  ${flags.join(' ')}`,
    ``,
    `# 3. Check it — the host appears under Hosts once the first sync lands`,
    `systemctl status photon-agent --no-pager`,
  ].join('\n')
}

/** What can go wrong with the URL the host will dial, said before the operator pastes anything. */
export type OriginIssue = 'invalid' | 'loopback' | 'plain-http' | null

export function originIssue(url: string): OriginIssue {
  const o = normalizeOrigin(url)
  let u: URL
  try {
    u = new URL(o)
  } catch {
    return 'invalid'
  }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') return 'invalid'
  const h = u.hostname.replace(/^\[|\]$/g, '')
  if (h === 'localhost' || h === '::1' || /^127\./.test(h) || h === '0.0.0.0') return 'loopback'
  if (u.protocol === 'http:') return 'plain-http'
  return null
}

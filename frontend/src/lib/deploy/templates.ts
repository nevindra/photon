// The Template vocabulary: typed variables (Portainer's `env` object, taken further), what each
// type means for the form, validation of an operator's values against a manifest, and the
// manifest's YAML rendering for the publish preview. Pure.
import type { DockerFacts, TargetSelector, TemplateManifest, TemplateVariable, VariableType } from '@/lib/core/api'

export interface VariableTypeMeta {
  label: string
  /** One line for the publish form's type picker. */
  summary: string
}

export const VARIABLE_TYPES: VariableType[] = ['string', 'number', 'bool', 'select', 'port', 'path', 'secret', 'generated']

export const VARIABLE_TYPE_META: Record<VariableType, VariableTypeMeta> = {
  string: { label: 'Text', summary: 'Free text.' },
  number: { label: 'Number', summary: 'A number; the form rejects anything else.' },
  bool: { label: 'Yes / no', summary: 'A switch; rendered as true / false.' },
  select: { label: 'Choice', summary: 'One of a fixed list of options.' },
  port: { label: 'Port', summary: 'A host port; preflight checks it is free on the target.' },
  path: { label: 'Path', summary: 'A path on the host, for a bind mount.' },
  secret: { label: 'Secret', summary: 'Stored encrypted, write-only, resolved on the agent at apply time. Never rendered into a file Photon keeps.' },
  generated: { label: 'Generated', summary: 'Photon mints a random value once at first deploy (a database password) and keeps it as a secret.' },
}

/** The starting values for a deploy form: defaults, presets, and nothing for secrets. */
export function defaultVariables(manifest: Pick<TemplateManifest, 'variables'>, existing: Record<string, string> = {}): Record<string, string> {
  const out: Record<string, string> = {}
  for (const v of manifest.variables) {
    if (v.type === 'generated' || v.type === 'secret') continue
    // A preset is pinned by THIS version: on an upgrade the stack's stored value (the old
    // version's pin) must not beat it, or the diff shows nothing and the old images ship.
    if (v.preset) out[v.name] = v.default ?? ''
    else out[v.name] = existing[v.name] ?? v.default ?? (v.type === 'bool' ? 'false' : v.type === 'select' ? (v.options?.[0] ?? '') : '')
  }
  return out
}

export function validateVariable(v: TemplateVariable, raw: string | undefined): string | null {
  const value = (raw ?? '').trim() || (v.preset ? (v.default ?? '') : '')
  if (v.type === 'generated') return null
  if (v.type === 'secret') return v.required && !value ? 'Required — enter it once; it is stored encrypted.' : null
  if (!value) return v.required ? 'Required.' : null
  switch (v.type) {
    case 'number':
      if (!/^-?\d+(\.\d+)?$/.test(value)) return 'Must be a number.'
      break
    case 'port': {
      const n = Number(value)
      if (!/^\d+$/.test(value) || n < 1 || n > 65535) return 'Must be a port between 1 and 65535.'
      break
    }
    case 'bool':
      if (value !== 'true' && value !== 'false') return 'Must be true or false.'
      break
    case 'select':
      if (v.options && !v.options.includes(value)) return `Must be one of ${v.options.join(', ')}.`
      break
    case 'path':
      if (!value.startsWith('/')) return 'Must be an absolute path on the host.'
      break
  }
  if (v.pattern) {
    try {
      if (!new RegExp(v.pattern).test(value)) return `Must match ${v.pattern}.`
    } catch {
      /* a broken pattern in a manifest must not block deploys */
    }
  }
  return null
}

/** name → error for every variable that fails; an empty object means the form may submit. */
export function validateVariables(manifest: Pick<TemplateManifest, 'variables'>, values: Record<string, string>, secretsProvided: Set<string> = new Set()): Record<string, string> {
  const errors: Record<string, string> = {}
  for (const v of manifest.variables) {
    if (v.type === 'secret' && secretsProvided.has(v.name)) continue
    const err = validateVariable(v, values[v.name])
    if (err) errors[v.name] = err
  }
  return errors
}

/** Variables grouped for the form: `group` from the manifest, "General" for the rest, secrets last. */
export function groupVariables(manifest: Pick<TemplateManifest, 'variables'>): { group: string; variables: TemplateVariable[] }[] {
  const map = new Map<string, TemplateVariable[]>()
  for (const v of manifest.variables) {
    const g = v.type === 'secret' || v.type === 'generated' ? 'Secrets' : (v.group ?? 'General')
    map.set(g, [...(map.get(g) ?? []), v])
  }
  const groups = Array.from(map.entries()).map(([group, variables]) => ({ group, variables }))
  return groups.sort((a, b) => (a.group === 'Secrets' ? 1 : b.group === 'Secrets' ? -1 : a.group === 'General' ? -1 : b.group === 'General' ? 1 : a.group.localeCompare(b.group)))
}

/** The `.env` a template deploy renders — non-secret variables only, plus the observability block. */
export function renderEnv(manifest: Pick<TemplateManifest, 'variables' | 'observability'>, values: Record<string, string>, ctx: { otlpEndpoint: string; environment: string | null }): string {
  const lines: string[] = []
  for (const v of manifest.variables) {
    if (v.type === 'secret' || v.type === 'generated') continue
    lines.push(`${v.name}=${values[v.name] ?? v.default ?? ''}`)
  }
  if (manifest.observability.otlp_endpoint) lines.push(`OTEL_EXPORTER_OTLP_ENDPOINT=${ctx.otlpEndpoint}`)
  if (manifest.observability.environment && ctx.environment) lines.push(`DEPLOYMENT_ENVIRONMENT=${ctx.environment}`)
  return lines.join('\n') + '\n'
}

/** "1.10.0" > "1.9.2"; non-numeric parts compare as strings. */
export function semverCompare(a: string, b: string): number {
  const pa = a.split(/[.-]/)
  const pb = b.split(/[.-]/)
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? '0'
    const y = pb[i] ?? '0'
    const nx = Number(x)
    const ny = Number(y)
    const cmp = Number.isFinite(nx) && Number.isFinite(ny) ? nx - ny : x.localeCompare(y)
    if (cmp !== 0) return cmp
  }
  return 0
}

export type RequirementState = 'ok' | 'fail' | 'unknown'
export interface RequirementCheck {
  key: string
  /** What the template asks for, in its own words: "Docker ≥ 24.0". */
  need: string
  /** What the host has, when the agent reported it; otherwise what will establish it. */
  have: string
  state: RequirementState
}

const GB = 1024 ** 3
const ARCH_ALIASES: Record<string, string> = { amd64: 'x86_64', x64: 'x86_64', arm64: 'aarch64' }
function canonArch(a: string): string {
  const l = a.trim().toLowerCase()
  return ARCH_ALIASES[l] ?? l
}

/**
 * The Target step's answer to "what this template needs": each requirement marked against the
 * host the agent describes. Only what the agent actually reports gets a verdict — arch, Docker
 * version, free disk — and everything else (RAM, ports, volumes) is `unknown` and says so, because
 * Preflight is where the host itself renders and validates. A missing host or missing facts is
 * likewise `unknown`, never a fail: this list guides the choice, it does not gate it.
 */
export function requirementChecks(manifest: Pick<TemplateManifest, 'requires' | 'ports' | 'volumes'>, facts: DockerFacts | null): RequirementCheck[] {
  const r = manifest.requires
  const out: RequirementCheck[] = []
  const later = 'checked at preflight'
  if (r.arch?.length) {
    const wanted = r.arch.map(canonArch)
    const have = facts?.arch ? canonArch(facts.arch) : null
    out.push({ key: 'arch', need: `arch ${r.arch.join(' / ')}`, have: have ?? later, state: have ? (wanted.includes(have) ? 'ok' : 'fail') : 'unknown' })
  }
  if (r.min_docker) {
    const have = facts?.version ?? null
    out.push({ key: 'docker', need: `Docker ≥ ${r.min_docker}`, have: have ?? later, state: have ? (semverCompare(have, r.min_docker) >= 0 ? 'ok' : 'fail') : 'unknown' })
  }
  if (r.min_ram_bytes) out.push({ key: 'ram', need: `≥ ${Math.round(r.min_ram_bytes / GB)} GB RAM`, have: later, state: 'unknown' })
  if (r.min_disk_bytes) {
    const d = facts?.disk
    const free = d && d.total_bytes != null && d.used_bytes != null ? d.total_bytes - d.used_bytes : null
    out.push({ key: 'disk', need: `≥ ${Math.round(r.min_disk_bytes / GB)} GB free disk`, have: free != null ? `${(free / GB).toFixed(free < 10 * GB ? 1 : 0)} GB free` : later, state: free != null ? (free >= r.min_disk_bytes ? 'ok' : 'fail') : 'unknown' })
  }
  for (const p of manifest.ports) out.push({ key: `port:${p.name}`, need: `port ${p.default_host} (${p.name})`, have: later, state: 'unknown' })
  for (const v of manifest.volumes) out.push({ key: `volume:${v.name}`, need: `volume ${v.name}`, have: later, state: 'unknown' })
  return out
}

/** Bump the last numeric part: "2.4.1" → "2.4.2". */
export function nextVersion(v: string): string {
  const m = v.match(/^(.*?)(\d+)([^\d]*)$/)
  return m ? `${m[1]}${Number(m[2]) + 1}${m[3]}` : v + '.1'
}

function yamlStr(s: string): string {
  return /^[A-Za-z0-9_./:-]+$/.test(s) && !/^(true|false|null|yes|no)$/i.test(s) && !/^\d/.test(s) ? s : JSON.stringify(s)
}

/** `photon.template.yaml` for the publish preview and the Compose tab. */
export function manifestToYaml(m: TemplateManifest): string {
  const out: string[] = []
  out.push(`id: ${yamlStr(m.id)}`, `version: ${yamlStr(m.version)}`, `name: ${yamlStr(m.name)}`, `description: ${yamlStr(m.description)}`, `icon: ${yamlStr(m.icon)}`)
  out.push(`categories: [${m.categories.map(yamlStr).join(', ')}]`)
  out.push('variables:')
  for (const v of m.variables) {
    out.push(`  - name: ${v.name}`, `    type: ${v.type}`, `    label: ${yamlStr(v.label)}`)
    if (v.description) out.push(`    description: ${yamlStr(v.description)}`)
    if (v.required) out.push('    required: true')
    if (v.default != null && v.default !== '') out.push(`    default: ${yamlStr(v.default)}`)
    if (v.preset) out.push('    preset: true')
    if (v.options?.length) out.push(`    options: [${v.options.map(yamlStr).join(', ')}]`)
    if (v.pattern) out.push(`    pattern: ${JSON.stringify(v.pattern)}`)
    if (v.group) out.push(`    group: ${yamlStr(v.group)}`)
  }
  const r = m.requires
  if (r.arch?.length || r.min_docker || r.min_ram_bytes || r.min_disk_bytes) {
    out.push('requires:')
    if (r.arch?.length) out.push(`  arch: [${r.arch.join(', ')}]`)
    if (r.min_docker) out.push(`  min_docker: ${yamlStr(r.min_docker)}`)
    if (r.min_ram_bytes) out.push(`  min_ram_bytes: ${r.min_ram_bytes}`)
    if (r.min_disk_bytes) out.push(`  min_disk_bytes: ${r.min_disk_bytes}`)
  }
  if (m.ports.length) {
    out.push('ports:')
    for (const p of m.ports) out.push(`  - name: ${p.name}`, `    container: ${p.container}`, `    default_host: ${p.default_host}`, `    protocol: ${p.protocol}`)
  }
  if (m.volumes.length) {
    out.push('volumes:')
    for (const v of m.volumes) out.push(`  - name: ${v.name}`, `    path: ${yamlStr(v.path)}`)
  }
  if (m.health) out.push('health:', `  kind: ${m.health.kind}`, `  detail: ${yamlStr(m.health.detail)}`, `  grace_secs: ${m.health.grace_secs}`)
  out.push('photon:', `  otlp_endpoint: ${m.observability.otlp_endpoint}`, `  ingest_token: ${m.observability.ingest_token}`, `  service_name: ${m.observability.service_name ? yamlStr(m.observability.service_name) : 'null'}`, `  environment: ${m.observability.environment}`)
  return out.join('\n') + '\n'
}

export interface PreflightMeta {
  label: string
  tone: 'success' | 'error' | 'warning' | 'neutral'
}
export const PREFLIGHT_META: Record<'pass' | 'warn' | 'fail' | 'skip', PreflightMeta> = {
  pass: { label: 'OK', tone: 'success' },
  warn: { label: 'Warning', tone: 'warning' },
  fail: { label: 'Blocked', tone: 'error' },
  skip: { label: 'Not checked', tone: 'neutral' },
}

// A registry credential's scope (or a rollout's selector) in one phrase: `null` is every host.
export function scopeText(scope: TargetSelector | null): string {
  if (!scope) return 'every host'
  if (scope.kind === 'site') return `site ${scope.sites.join(', ')}`
  if (scope.kind === 'hosts') return `hosts ${scope.hosts.join(', ')}`
  return `labels ${Object.entries(scope.labels).map(([k, v]) => `${k}=${v}`).join(',')}`
}

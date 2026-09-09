// Portainer app-template JSON (v2 / v3) → Photon templates. Pure. Gives us the community catalog
// for free (R5.7): compose stacks (type 3) become a template with a compose file; the `env`
// array becomes typed variables (Portainer's `select` and `preset` survive; everything else is
// text). Container templates (type 1) are converted into a one-service compose file; Swarm
// stacks (type 2) are skipped, Photon does not cluster.
import type { TemplateManifest, TemplateVariable } from '@/lib/core/api'

export interface PortainerEnv {
  name: string
  label?: string
  description?: string
  default?: string
  preset?: boolean
  select?: { text: string; value: string; default?: boolean }[]
}
export interface PortainerTemplate {
  type: number
  title: string
  name?: string
  description?: string
  logo?: string
  categories?: string[]
  platform?: string
  image?: string
  repository?: { url: string; stackfile: string }
  env?: PortainerEnv[]
  ports?: string[]
  volumes?: { container: string; bind?: string; readonly?: boolean }[]
  restart_policy?: string
  network?: string
  command?: string
}

export interface ImportedTemplate {
  manifest: TemplateManifest
  compose_text: string
  source_kind: 'compose_stack' | 'container'
}

export interface ImportReport {
  imported: ImportedTemplate[]
  skipped: { title: string; reason: string }[]
  version: 2 | 3
}

function slug(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 48) || 'template'
}

function toVariable(e: PortainerEnv): TemplateVariable {
  const sel = e.select?.length ? e.select : null
  return {
    name: e.name,
    label: e.label ?? e.name,
    description: e.description ?? null,
    type: sel ? 'select' : /pass|secret|token|key/i.test(e.name) ? 'secret' : /port/i.test(e.name) ? 'port' : /path|dir/i.test(e.name) ? 'path' : 'string',
    required: !e.default && !sel,
    default: sel ? (sel.find((o) => o.default)?.value ?? sel[0].value) : (e.default ?? null),
    preset: !!e.preset,
    options: sel ? sel.map((o) => o.value) : undefined,
    pattern: null,
    group: null,
  }
}

function containerCompose(t: PortainerTemplate): string {
  const svc = slug(t.name ?? t.title)
  const lines = ['services:', `  ${svc}:`, `    image: ${t.image ?? 'unknown'}`, `    restart: ${t.restart_policy ?? 'unless-stopped'}`]
  if (t.ports?.length) {
    lines.push('    ports:')
    for (const p of t.ports) lines.push(`      - "${p.replace(/^[^:]*:(?=\d)/, (m) => m)}"`)
  }
  if (t.volumes?.length) {
    lines.push('    volumes:')
    for (const v of t.volumes) lines.push(`      - ${v.bind ?? slug(v.container) + '_data'}:${v.container}${v.readonly ? ':ro' : ''}`)
  }
  if (t.env?.length) {
    lines.push('    environment:')
    for (const e of t.env) lines.push(`      ${e.name}: \${${e.name}}`)
  }
  if (t.command) lines.push(`    command: ${t.command}`)
  if (t.network) lines.push(`    network_mode: ${t.network}`)
  const named = (t.volumes ?? []).filter((v) => !v.bind)
  if (named.length) {
    lines.push('volumes:')
    for (const v of named) lines.push(`  ${slug(v.container) + '_data'}:`)
  }
  return lines.join('\n') + '\n'
}

export function parsePortainerTemplates(json: string, sourceName = 'portainer'): ImportReport {
  let doc: unknown
  try {
    doc = JSON.parse(json)
  } catch (e) {
    throw new Error(`Not valid JSON: ${(e as Error).message}`)
  }
  const root = doc as { version?: string; templates?: PortainerTemplate[] }
  const list: PortainerTemplate[] = Array.isArray(root.templates) ? root.templates : Array.isArray(doc) ? (doc as PortainerTemplate[]) : []
  if (!list.length) throw new Error('No `templates` array found. Expected Portainer app-template JSON v2 or v3.')
  const version: 2 | 3 = root.version === '3' ? 3 : 2
  const report: ImportReport = { imported: [], skipped: [], version }
  const seen = new Set<string>()
  for (const t of list) {
    if (!t?.title) {
      report.skipped.push({ title: '(untitled)', reason: 'no title' })
      continue
    }
    if (t.type === 2) {
      report.skipped.push({ title: t.title, reason: 'Swarm stack — Photon does not cluster' })
      continue
    }
    if (t.type !== 1 && t.type !== 3) {
      report.skipped.push({ title: t.title, reason: `unsupported type ${t.type}` })
      continue
    }
    let id = slug(t.name ?? t.title)
    while (seen.has(id)) id += '-2'
    seen.add(id)
    const manifest: TemplateManifest = {
      id,
      version: '1.0.0',
      name: t.title,
      description: t.description ?? '',
      icon: t.logo ?? '',
      categories: (t.categories ?? []).map((c) => c.toLowerCase()),
      variables: (t.env ?? []).map(toVariable),
      requires: { arch: t.platform ? [t.platform] : undefined, min_docker: null, min_ram_bytes: null, min_disk_bytes: null },
      ports: [],
      volumes: [],
      health: null,
      observability: { otlp_endpoint: false, ingest_token: false, service_name: null, environment: false },
    }
    const compose = t.type === 3 ? `# from ${sourceName}: ${t.repository?.url ?? '?'} ${t.repository?.stackfile ?? ''}\n# (fetched on sync)\n` : containerCompose(t)
    report.imported.push({ manifest, compose_text: compose, source_kind: t.type === 3 ? 'compose_stack' : 'container' })
  }
  return report
}
